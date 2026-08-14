//! Native storage authority for `weakref` weak containers.
//!
//! A Python wrapper owns exactly one `TYPE_ID_WEAK_CONTAINER_STATE` object.
//! The state owns every strong entry edge and stores cached hashes in one
//! open-addressed table.  Weak referents remain owned by the ordinary weakref
//! node; its optional container cookie provides generation-safe target-death
//! removal without a second global registry.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::object::heap_lifecycle::DetachedEdgeSink;
use crate::object::layout::{
    iter_set_cached_tuple, iter_set_expected_version, iter_set_index, iter_set_projection,
};
use crate::object::{
    MoltHeader, TYPE_ID_ITER, TYPE_ID_WEAK_CONTAINER_STATE, alloc_object, bits_from_ptr,
    dec_ref_bits, inc_ref_bits, object_type_id,
};
use crate::{
    MoltObject, PyToken, alloc_list, alloc_tuple, exception_pending, int_bits_from_i64, is_truthy,
    maybe_ptr_from_bits, molt_eq, obj_from_bits, raise_exception, to_i64,
};

use super::weakref::{
    WeakContainerCookie, weakref_attach_container_cookie, weakref_container_cookie,
    weakref_detach_container_cookie, weakref_has_live_target, weakref_peek_owned,
    weakref_seed_cached_hash,
};

pub(crate) const WEAK_CONTAINER_KIND_KEY_DICT: u8 = 1;
pub(crate) const WEAK_CONTAINER_KIND_VALUE_DICT: u8 = 2;
pub(crate) const WEAK_CONTAINER_KIND_SET: u8 = 3;

pub(crate) const WEAK_CONTAINER_PROJECTION_KEYS: u8 = 1;
pub(crate) const WEAK_CONTAINER_PROJECTION_VALUES: u8 = 2;
pub(crate) const WEAK_CONTAINER_PROJECTION_ITEMS: u8 = 3;

pub(crate) const WEAK_ITER_VERSION_UNSTARTED: u64 = u64::MAX;
pub(crate) const WEAK_ITER_VERSION_FINISHED: u64 = u64::MAX - 1;
const MAX_STRUCTURAL_VERSION: u64 = WEAK_ITER_VERSION_FINISHED - 1;
const GENERATION_EXHAUSTED: u64 = u64::MAX;

const TABLE_TOMBSTONE: u32 = u32::MAX;
const SLOT_NONE: u32 = u32::MAX;
const MAX_SLOT_COUNT: usize = (u32::MAX - 1) as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeakTableError {
    GenerationExhausted,
    StructuralVersionExhausted,
    ContentVersionExhausted,
    SlotDomainExhausted,
    TableCapacityExhausted,
    IteratorCountExhausted,
    InvalidPreparedCapacity,
}

fn raise_table_error(_py: &PyToken<'_>, error: WeakTableError) -> u64 {
    let message = match error {
        WeakTableError::GenerationExhausted => "weak container generation space exhausted",
        WeakTableError::StructuralVersionExhausted => {
            "weak container structural version space exhausted"
        }
        WeakTableError::ContentVersionExhausted => "weak container content version space exhausted",
        WeakTableError::SlotDomainExhausted => "weak container slot domain exhausted",
        WeakTableError::TableCapacityExhausted => "weak container table capacity exhausted",
        WeakTableError::IteratorCountExhausted => "weak container iterator count exhausted",
        WeakTableError::InvalidPreparedCapacity => "invalid prepared weak container table capacity",
    };
    let exception = if error == WeakTableError::InvalidPreparedCapacity {
        "RuntimeError"
    } else {
        "OverflowError"
    };
    raise_exception::<u64>(_py, exception, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeakContainerKind {
    KeyDict,
    ValueDict,
    Set,
}

impl WeakContainerKind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            WEAK_CONTAINER_KIND_KEY_DICT => Some(Self::KeyDict),
            WEAK_CONTAINER_KIND_VALUE_DICT => Some(Self::ValueDict),
            WEAK_CONTAINER_KIND_SET => Some(Self::Set),
            _ => None,
        }
    }

    fn mutation_error(self) -> &'static str {
        match self {
            Self::Set => "Set changed size during iteration",
            Self::KeyDict | Self::ValueDict => "dictionary changed size during iteration",
        }
    }

    #[inline]
    fn owns_aux(self) -> bool {
        self != Self::Set
    }

    #[inline]
    #[cfg(test)]
    fn owned_edges_per_entry(self) -> usize {
        1 + usize::from(self.owns_aux())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WeakEntryId {
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryState {
    Live,
    PendingDead,
}

struct WeakEntry {
    generation: u64,
    content_version: u64,
    state: EntryState,
    cached_hash: u64,
    weakref_bits: u64,
    /// KeyDict: owned value. ValueDict: owned original canonical key. Set: None.
    aux_bits: u64,
    order_prev: u32,
    order_next: u32,
    pending_next: u32,
}

impl WeakEntry {
    fn id(&self, slot: usize) -> WeakEntryId {
        WeakEntryId {
            slot: slot as u32,
            generation: self.generation,
        }
    }
}

struct WeakTable {
    buckets: Vec<u32>,
    entries: Vec<Option<WeakEntry>>,
    /// State-wide generation source survives slot reuse and full clears.
    next_generation: u64,
    free_slots: Vec<usize>,
    order_head: u32,
    order_tail: u32,
    pending_head: u32,
    pending_tail: u32,
    live_len: usize,
    tombstones: usize,
    structural_version: u64,
    structural_reservations: u64,
    active_iterators: usize,
    #[cfg(test)]
    pending_queue_visits: usize,
}

#[derive(Default)]
struct PreparedInsert {
    buckets: Option<Vec<u32>>,
    bucket_len: Option<usize>,
    entries: Option<Vec<Option<WeakEntry>>>,
    free_slots: Option<Vec<usize>>,
}

#[derive(Default)]
struct DisplacedInsertBuffers {
    buckets: Option<Vec<u32>>,
    entries: Option<Vec<Option<WeakEntry>>>,
    free_slots: Option<Vec<usize>>,
}

#[derive(Clone, Copy)]
struct InsertAdmission {
    slot: usize,
    generation: u64,
    structural_version: u64,
}

impl WeakTable {
    fn new() -> Self {
        Self {
            buckets: Vec::new(),
            entries: Vec::new(),
            next_generation: 1,
            free_slots: Vec::new(),
            order_head: SLOT_NONE,
            order_tail: SLOT_NONE,
            pending_head: SLOT_NONE,
            pending_tail: SLOT_NONE,
            live_len: 0,
            tombstones: 0,
            structural_version: 0,
            structural_reservations: 0,
            active_iterators: 0,
            #[cfg(test)]
            pending_queue_visits: 0,
        }
    }

    fn capacity_for(entries: usize) -> Result<usize, WeakTableError> {
        let doubled = entries
            .max(1)
            .checked_mul(2)
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        let capacity = doubled
            .checked_next_power_of_two()
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        Ok(capacity.max(8))
    }

    fn next_structural_version(&self) -> Result<u64, WeakTableError> {
        let reserved_ceiling = MAX_STRUCTURAL_VERSION
            .checked_sub(self.structural_reservations)
            .ok_or(WeakTableError::StructuralVersionExhausted)?;
        if self.structural_version >= reserved_ceiling {
            return Err(WeakTableError::StructuralVersionExhausted);
        }
        self.structural_version
            .checked_add(1)
            .ok_or(WeakTableError::StructuralVersionExhausted)
    }

    fn reserve_structural_version(&mut self) -> Result<(), WeakTableError> {
        let next_reservations = self
            .structural_reservations
            .checked_add(1)
            .ok_or(WeakTableError::StructuralVersionExhausted)?;
        if self
            .structural_version
            .checked_add(next_reservations)
            .is_none_or(|value| value > MAX_STRUCTURAL_VERSION)
        {
            return Err(WeakTableError::StructuralVersionExhausted);
        }
        self.structural_reservations = next_reservations;
        Ok(())
    }

    fn release_structural_reservation(&mut self) -> Result<(), WeakTableError> {
        self.structural_reservations = self
            .structural_reservations
            .checked_sub(1)
            .ok_or(WeakTableError::StructuralVersionExhausted)?;
        Ok(())
    }

    fn consume_structural_reservation(&mut self) -> Result<(), WeakTableError> {
        let remaining = self
            .structural_reservations
            .checked_sub(1)
            .ok_or(WeakTableError::StructuralVersionExhausted)?;
        let next_version = self
            .structural_version
            .checked_add(1)
            .filter(|version| *version <= MAX_STRUCTURAL_VERSION)
            .ok_or(WeakTableError::StructuralVersionExhausted)?;
        self.structural_reservations = remaining;
        self.structural_version = next_version;
        Ok(())
    }

    fn next_generation(&self) -> Result<u64, WeakTableError> {
        if self.next_generation == GENERATION_EXHAUSTED {
            Err(WeakTableError::GenerationExhausted)
        } else {
            Ok(self.next_generation)
        }
    }

    fn admit_insert(&self) -> Result<InsertAdmission, WeakTableError> {
        if !Self::slot_domain_admits(self.entries.len(), !self.free_slots.is_empty()) {
            return Err(WeakTableError::SlotDomainExhausted);
        }
        let slot = self
            .free_slots
            .last()
            .copied()
            .unwrap_or(self.entries.len());
        if slot >= MAX_SLOT_COUNT {
            return Err(WeakTableError::SlotDomainExhausted);
        }
        let prospective_live = self
            .live_len
            .checked_add(1)
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        Self::capacity_for(prospective_live)?;
        Ok(InsertAdmission {
            slot,
            generation: self.next_generation()?,
            structural_version: self.next_structural_version()?,
        })
    }

    fn slot_domain_admits(entries_len: usize, has_free_slot: bool) -> bool {
        has_free_slot || entries_len < MAX_SLOT_COUNT
    }

    fn entry(&self, id: WeakEntryId) -> Option<&WeakEntry> {
        let entry = self.entries.get(id.slot as usize)?.as_ref()?;
        (entry.generation == id.generation).then_some(entry)
    }

    fn entry_mut(&mut self, id: WeakEntryId) -> Option<&mut WeakEntry> {
        let entry = self.entries.get_mut(id.slot as usize)?.as_mut()?;
        (entry.generation == id.generation).then_some(entry)
    }

    fn live_entry(&self, id: WeakEntryId) -> Option<&WeakEntry> {
        self.entry(id)
            .filter(|entry| entry.state == EntryState::Live)
    }

    fn live_entry_mut(&mut self, id: WeakEntryId) -> Option<&mut WeakEntry> {
        self.entry_mut(id)
            .filter(|entry| entry.state == EntryState::Live)
    }

    fn find_insert_bucket(&self, hash: u64) -> Result<(usize, bool), WeakTableError> {
        if self.buckets.is_empty() {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        let mask = self.buckets.len() - 1;
        let mut bucket = (hash as usize) & mask;
        let mut first_tombstone = None;
        for _ in 0..self.buckets.len() {
            match self.buckets[bucket] {
                0 => {
                    let target = first_tombstone.unwrap_or(bucket);
                    return Ok((target, first_tombstone.is_some()));
                }
                TABLE_TOMBSTONE if first_tombstone.is_none() => {
                    first_tombstone = Some(bucket);
                }
                _ => {}
            }
            bucket = (bucket + 1) & mask;
        }
        first_tombstone
            .map(|bucket| (bucket, true))
            .ok_or(WeakTableError::TableCapacityExhausted)
    }

    fn commit_insert_bucket(
        &mut self,
        bucket: usize,
        reused_tombstone: bool,
        slot: u32,
    ) -> Result<(), WeakTableError> {
        let stored = slot
            .checked_add(1)
            .filter(|stored| *stored != TABLE_TOMBSTONE)
            .ok_or(WeakTableError::SlotDomainExhausted)?;
        if reused_tombstone {
            self.tombstones = self
                .tombstones
                .checked_sub(1)
                .ok_or(WeakTableError::InvalidPreparedCapacity)?;
        }
        self.buckets[bucket] = stored;
        Ok(())
    }

    fn rebuild(&mut self, requested_capacity: usize) -> Result<(), WeakTableError> {
        let capacity = requested_capacity.max(Self::capacity_for(self.live_len)?);
        if capacity > self.buckets.capacity() {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        self.buckets.clear();
        self.buckets.resize(capacity, 0);
        self.tombstones = 0;
        for slot in 0..self.entries.len() {
            let Some(entry) = self.entries[slot].as_ref() else {
                continue;
            };
            if entry.state == EntryState::Live {
                let hash = entry.cached_hash;
                let slot = u32::try_from(slot).map_err(|_| WeakTableError::SlotDomainExhausted)?;
                let (bucket, reused_tombstone) = self.find_insert_bucket(hash)?;
                self.commit_insert_bucket(bucket, reused_tombstone, slot)?;
            }
        }
        Ok(())
    }

    fn needs_bucket_rebuild_for_insert(&self) -> Result<bool, WeakTableError> {
        if self.buckets.is_empty() {
            return Ok(true);
        }
        let occupied = self
            .live_len
            .checked_add(self.tombstones)
            .and_then(|value| value.checked_add(1))
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        let occupied_load = occupied
            .checked_mul(3)
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        let bucket_load = self
            .buckets
            .len()
            .checked_mul(2)
            .ok_or(WeakTableError::TableCapacityExhausted)?;
        Ok(occupied_load >= bucket_load)
    }

    fn install_prepared_insert(
        &mut self,
        prepared: &mut PreparedInsert,
    ) -> Result<DisplacedInsertBuffers, WeakTableError> {
        let mut displaced = DisplacedInsertBuffers::default();
        let rebuild_buckets = self.needs_bucket_rebuild_for_insert()?;
        let target = if rebuild_buckets {
            let prospective_live = self
                .live_len
                .checked_add(1)
                .ok_or(WeakTableError::TableCapacityExhausted)?;
            Some(Self::capacity_for(prospective_live)?)
        } else {
            None
        };
        if let Some(target) = target {
            let prepared_capacity = prepared
                .buckets
                .as_ref()
                .map_or(self.buckets.capacity(), Vec::capacity);
            if prepared_capacity < target {
                return Err(WeakTableError::InvalidPreparedCapacity);
            }
        }
        let grows_entries =
            self.free_slots.is_empty() && self.entries.len() == self.entries.capacity();
        if grows_entries
            && prepared
                .entries
                .as_ref()
                .is_none_or(|entries| entries.capacity() <= self.entries.len())
        {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        let prospective_entries_capacity = prepared
            .entries
            .as_ref()
            .map_or(self.entries.capacity(), Vec::capacity);
        if self.free_slots.capacity() < prospective_entries_capacity
            && prepared
                .free_slots
                .as_ref()
                .is_none_or(|slots| slots.capacity() < prospective_entries_capacity)
        {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }

        if let Some(target) = target {
            if let Some(mut buckets) = prepared.buckets.take() {
                std::mem::swap(&mut buckets, &mut self.buckets);
                displaced.buckets = Some(buckets);
            }
            let requested = prepared.bucket_len.take().unwrap_or(target);
            self.rebuild(requested)?;
        }
        if grows_entries {
            let Some(mut entries) = prepared.entries.take() else {
                return Err(WeakTableError::InvalidPreparedCapacity);
            };
            entries.append(&mut self.entries);
            std::mem::swap(&mut entries, &mut self.entries);
            displaced.entries = Some(entries);
        }
        if self.free_slots.capacity() < self.entries.capacity() {
            let Some(mut free_slots) = prepared.free_slots.take() else {
                return Err(WeakTableError::InvalidPreparedCapacity);
            };
            free_slots.append(&mut self.free_slots);
            std::mem::swap(&mut free_slots, &mut self.free_slots);
            displaced.free_slots = Some(free_slots);
        }
        Ok(displaced)
    }

    /// Return the next cached-hash candidate from an open-addressing probe.
    /// `probe_step` makes repeated lookup allocation-free.
    fn next_candidate(&self, hash: u64, mut probe_step: usize) -> Option<(WeakEntryId, usize)> {
        if self.buckets.is_empty() {
            return None;
        }
        let mask = self.buckets.len() - 1;
        while probe_step < self.buckets.len() {
            let bucket = ((hash as usize) + probe_step) & mask;
            probe_step += 1;
            let stored = self.buckets[bucket];
            if stored == 0 {
                return None;
            }
            if stored != TABLE_TOMBSTONE {
                let slot = (stored - 1) as usize;
                if let Some(entry) = self.entries.get(slot).and_then(Option::as_ref)
                    && entry.state == EntryState::Live
                    && entry.cached_hash == hash
                {
                    return Some((entry.id(slot), probe_step));
                }
            }
        }
        None
    }

    fn find_remove_bucket(&self, id: WeakEntryId, hash: u64) -> Option<usize> {
        if self.buckets.is_empty() {
            return None;
        }
        let mask = self.buckets.len() - 1;
        let mut bucket = (hash as usize) & mask;
        for _ in 0..self.buckets.len() {
            let stored = self.buckets[bucket];
            if stored == 0 {
                return None;
            }
            if stored != TABLE_TOMBSTONE && stored - 1 == id.slot {
                return Some(bucket);
            }
            bucket = (bucket + 1) & mask;
        }
        None
    }

    fn insert_entry(
        &mut self,
        hash: u64,
        weakref_bits: u64,
        aux_bits: u64,
        admission: InsertAdmission,
    ) -> Result<WeakEntryId, WeakTableError> {
        if self.needs_bucket_rebuild_for_insert()? {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        let slot = admission.slot;
        let reuses_slot = self.free_slots.last().copied() == Some(slot);
        if reuses_slot {
            if self.entries.get(slot).is_none_or(Option::is_some) {
                return Err(WeakTableError::InvalidPreparedCapacity);
            }
        } else {
            if slot != self.entries.len() || self.entries.len() == self.entries.capacity() {
                return Err(WeakTableError::InvalidPreparedCapacity);
            }
        }
        let slot_u32 = u32::try_from(slot).map_err(|_| WeakTableError::SlotDomainExhausted)?;
        if slot >= MAX_SLOT_COUNT || slot_u32 == SLOT_NONE {
            return Err(WeakTableError::SlotDomainExhausted);
        }
        if self.order_tail != SLOT_NONE
            && self
                .entries
                .get(self.order_tail as usize)
                .is_none_or(Option::is_none)
        {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        let next_generation = admission
            .generation
            .checked_add(1)
            .ok_or(WeakTableError::GenerationExhausted)?;
        let (bucket, reused_tombstone) = self.find_insert_bucket(hash)?;
        let stored_slot = slot_u32
            .checked_add(1)
            .filter(|stored| *stored != TABLE_TOMBSTONE)
            .ok_or(WeakTableError::SlotDomainExhausted)?;
        let next_tombstones = if reused_tombstone {
            self.tombstones
                .checked_sub(1)
                .ok_or(WeakTableError::InvalidPreparedCapacity)?
        } else {
            self.tombstones
        };
        let next_live_len = self
            .live_len
            .checked_add(1)
            .ok_or(WeakTableError::TableCapacityExhausted)?;

        if reuses_slot {
            self.free_slots.pop();
        } else {
            self.entries.push(None);
        }
        self.next_generation = next_generation;
        self.structural_version = admission.structural_version;
        let id = WeakEntryId {
            slot: slot_u32,
            generation: admission.generation,
        };
        self.entries[slot] = Some(WeakEntry {
            generation: admission.generation,
            content_version: 1,
            state: EntryState::Live,
            cached_hash: hash,
            weakref_bits,
            aux_bits,
            order_prev: self.order_tail,
            order_next: SLOT_NONE,
            pending_next: SLOT_NONE,
        });
        if self.order_tail != SLOT_NONE {
            let tail = self.order_tail;
            self.entries[tail as usize]
                .as_mut()
                .ok_or(WeakTableError::InvalidPreparedCapacity)?
                .order_next = slot_u32;
        } else {
            self.order_head = slot_u32;
        }
        self.order_tail = slot_u32;
        self.live_len = next_live_len;
        self.tombstones = next_tombstones;
        self.buckets[bucket] = stored_slot;
        Ok(id)
    }

    fn detach_entry(
        &mut self,
        id: WeakEntryId,
        structural: bool,
    ) -> Result<Option<WeakEntry>, WeakTableError> {
        let (hash, order_prev, order_next, was_live) = {
            let Some(entry) = self.entry(id) else {
                return Ok(None);
            };
            (
                entry.cached_hash,
                entry.order_prev,
                entry.order_next,
                entry.state == EntryState::Live,
            )
        };
        let next_version = structural
            .then(|| self.next_structural_version())
            .transpose()?;
        if self.free_slots.len() == self.free_slots.capacity() {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        if order_prev != SLOT_NONE
            && self
                .entries
                .get(order_prev as usize)
                .is_none_or(Option::is_none)
        {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        if order_next != SLOT_NONE
            && self
                .entries
                .get(order_next as usize)
                .is_none_or(Option::is_none)
        {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }
        let bucket = if was_live {
            Some(
                self.find_remove_bucket(id, hash)
                    .ok_or(WeakTableError::InvalidPreparedCapacity)?,
            )
        } else {
            None
        };
        let next_tombstones = if was_live {
            self.tombstones
                .checked_add(1)
                .ok_or(WeakTableError::TableCapacityExhausted)?
        } else {
            self.tombstones
        };
        let next_live_len = if was_live {
            self.live_len
                .checked_sub(1)
                .ok_or(WeakTableError::InvalidPreparedCapacity)?
        } else {
            self.live_len
        };
        let compact_target = (next_tombstones > self.buckets.len().max(1) / 4
            && self.active_iterators == 0)
            .then(|| Self::capacity_for(next_live_len))
            .transpose()?;
        if compact_target.is_some_and(|capacity| capacity > self.buckets.capacity()) {
            return Err(WeakTableError::InvalidPreparedCapacity);
        }

        if let Some(bucket) = bucket {
            self.buckets[bucket] = TABLE_TOMBSTONE;
        }
        self.tombstones = next_tombstones;
        if order_prev != SLOT_NONE {
            let prev = order_prev;
            if let Some(entry) = self.entries[prev as usize].as_mut() {
                entry.order_next = order_next;
            }
        } else {
            self.order_head = order_next;
        }
        if order_next != SLOT_NONE {
            let next = order_next;
            if let Some(entry) = self.entries[next as usize].as_mut() {
                entry.order_prev = order_prev;
            }
        } else {
            self.order_tail = order_prev;
        }
        let Some(entry) = self.entries[id.slot as usize].take() else {
            return Ok(None);
        };
        self.free_slots.push(id.slot as usize);
        self.live_len = next_live_len;
        if let Some(version) = next_version {
            self.structural_version = version;
        }
        if let Some(capacity) = compact_target {
            self.rebuild(capacity)?;
        }
        Ok(Some(entry))
    }

    fn detach_live_entry(
        &mut self,
        id: WeakEntryId,
        structural: bool,
    ) -> Result<Option<WeakEntry>, WeakTableError> {
        if self.live_entry(id).is_none() {
            return Ok(None);
        }
        self.detach_entry(id, structural)
    }

    fn detach_live_entry_versioned(
        &mut self,
        id: WeakEntryId,
        content_version: u64,
        structural: bool,
    ) -> Result<Option<WeakEntry>, WeakTableError> {
        if self
            .live_entry(id)
            .is_none_or(|entry| entry.content_version != content_version)
        {
            return Ok(None);
        }
        self.detach_entry(id, structural)
    }

    fn target_dead(&mut self, id: WeakEntryId) -> Result<Option<WeakEntry>, WeakTableError> {
        let (hash, is_live) = {
            let Some(entry) = self.entry(id) else {
                return Ok(None);
            };
            (entry.cached_hash, entry.state == EntryState::Live)
        };
        if !is_live {
            return Ok(None);
        }
        if self.active_iterators != 0 {
            let pending_tail = self.pending_tail;
            if pending_tail != SLOT_NONE
                && self
                    .entries
                    .get(pending_tail as usize)
                    .is_none_or(Option::is_none)
            {
                return Err(WeakTableError::InvalidPreparedCapacity);
            }
            let bucket = self
                .find_remove_bucket(id, hash)
                .ok_or(WeakTableError::InvalidPreparedCapacity)?;
            let next_live_len = self
                .live_len
                .checked_sub(1)
                .ok_or(WeakTableError::InvalidPreparedCapacity)?;
            let next_tombstones = self
                .tombstones
                .checked_add(1)
                .ok_or(WeakTableError::TableCapacityExhausted)?;

            let Some(entry) = self.entry_mut(id) else {
                return Err(WeakTableError::InvalidPreparedCapacity);
            };
            entry.state = EntryState::PendingDead;
            entry.pending_next = SLOT_NONE;
            if pending_tail == SLOT_NONE {
                self.pending_head = id.slot;
            } else if let Some(tail) = self.entries[pending_tail as usize].as_mut() {
                tail.pending_next = id.slot;
            }
            self.pending_tail = id.slot;
            self.live_len = next_live_len;
            self.tombstones = next_tombstones;
            self.buckets[bucket] = TABLE_TOMBSTONE;
            return Ok(None);
        }
        self.detach_entry(id, false)
    }

    fn finish_iterator(&mut self) -> Result<bool, WeakTableError> {
        self.active_iterators = self
            .active_iterators
            .checked_sub(1)
            .ok_or(WeakTableError::IteratorCountExhausted)?;
        Ok(self.active_iterators == 0)
    }

    fn detach_next_pending(
        &mut self,
        structural: bool,
    ) -> Result<Option<WeakEntry>, WeakTableError> {
        let slot = self.pending_head;
        if slot == SLOT_NONE {
            return Ok(None);
        }
        #[cfg(test)]
        {
            self.pending_queue_visits += 1;
        }
        let Some(entry) = self.entries[slot as usize].as_ref() else {
            self.pending_head = SLOT_NONE;
            self.pending_tail = SLOT_NONE;
            return Ok(None);
        };
        debug_assert_eq!(entry.state, EntryState::PendingDead);
        let id = entry.id(slot as usize);
        let next = entry.pending_next;
        if structural {
            self.next_structural_version()?;
        }
        self.pending_head = next;
        if next == SLOT_NONE {
            self.pending_tail = SLOT_NONE;
        }
        if let Some(entry) = self.entries[slot as usize].as_mut() {
            entry.pending_next = SLOT_NONE;
        }
        self.detach_entry(id, structural)
    }

    fn detach_all(&mut self, structural: bool) -> Result<Vec<Option<WeakEntry>>, WeakTableError> {
        let next_version = (structural && !self.entries.is_empty())
            .then(|| self.next_structural_version())
            .transpose()?;
        self.buckets.clear();
        self.order_head = SLOT_NONE;
        self.order_tail = SLOT_NONE;
        self.pending_head = SLOT_NONE;
        self.pending_tail = SLOT_NONE;
        self.free_slots.clear();
        self.live_len = 0;
        if let Some(version) = next_version {
            self.structural_version = version;
        }
        self.tombstones = 0;
        Ok(std::mem::take(&mut self.entries))
    }
}

pub(crate) struct WeakContainerState {
    kind: WeakContainerKind,
    table: RwLock<WeakTable>,
}

const _: () = {
    assert!(
        std::mem::align_of::<WeakContainerState>() <= molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES
    );
    assert!(
        std::mem::size_of::<MoltHeader>()
            .is_multiple_of(std::mem::align_of::<WeakContainerState>())
    );
};

impl WeakContainerState {
    fn read(&self) -> RwLockReadGuard<'_, WeakTable> {
        self.table
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, WeakTable> {
        self.table
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn replace_live_aux(
        &self,
        id: WeakEntryId,
        value_bits: u64,
    ) -> Result<Option<u64>, WeakTableError> {
        let mut table = self.write();
        let Some(entry) = table.live_entry_mut(id) else {
            return Ok(None);
        };
        entry.content_version = entry
            .content_version
            .checked_add(1)
            .ok_or(WeakTableError::ContentVersionExhausted)?;
        Ok(Some(std::mem::replace(&mut entry.aux_bits, value_bits)))
    }
}

fn reserved_vec<T>(capacity: usize) -> Option<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).ok()?;
    Some(values)
}

fn prepare_insert(_py: &PyToken<'_>, state: &WeakContainerState) -> Result<PreparedInsert, u64> {
    let prepared_targets = (|| -> Result<_, WeakTableError> {
        let table = state.read();
        table.admit_insert()?;
        let bucket_target = if table.needs_bucket_rebuild_for_insert()? {
            let prospective_live = table
                .live_len
                .checked_add(1)
                .ok_or(WeakTableError::TableCapacityExhausted)?;
            Some(WeakTable::capacity_for(prospective_live)?)
        } else {
            None
        };
        let allocate_buckets = bucket_target.filter(|target| table.buckets.capacity() < *target);
        let entry_target =
            if table.free_slots.is_empty() && table.entries.len() == table.entries.capacity() {
                let required = table
                    .entries
                    .len()
                    .checked_add(1)
                    .ok_or(WeakTableError::TableCapacityExhausted)?;
                let doubled = table
                    .entries
                    .capacity()
                    .max(8)
                    .checked_mul(2)
                    .ok_or(WeakTableError::TableCapacityExhausted)?;
                Some(doubled.max(required))
            } else {
                None
            };
        let prospective_entry_capacity = entry_target.unwrap_or(table.entries.capacity());
        let free_target = (table.free_slots.capacity() < prospective_entry_capacity)
            .then_some(prospective_entry_capacity);
        Ok((bucket_target, allocate_buckets, entry_target, free_target))
    })();
    let (bucket_target, allocate_buckets, entry_target, free_target) = match prepared_targets {
        Ok(targets) => targets,
        Err(error) => return Err(raise_table_error(_py, error)),
    };
    let buckets = if let Some(capacity) = allocate_buckets {
        let mut buckets = reserved_vec(capacity).ok_or_else(|| {
            raise_exception::<u64>(_py, "MemoryError", "weak table allocation failed")
        })?;
        buckets.resize(capacity, 0);
        Some(buckets)
    } else {
        None
    };
    let entries = entry_target
        .map(|capacity| {
            reserved_vec(capacity).ok_or_else(|| {
                raise_exception::<u64>(_py, "MemoryError", "weak entry allocation failed")
            })
        })
        .transpose()?;
    let free_slots = free_target
        .map(|capacity| {
            reserved_vec(capacity).ok_or_else(|| {
                raise_exception::<u64>(_py, "MemoryError", "weak slot allocation failed")
            })
        })
        .transpose()?;
    Ok(PreparedInsert {
        buckets,
        bucket_len: bucket_target,
        entries,
        free_slots,
    })
}

unsafe fn state_from_ptr(ptr: *mut u8) -> Option<&'static WeakContainerState> {
    if ptr.is_null() || unsafe { object_type_id(ptr) } != TYPE_ID_WEAK_CONTAINER_STATE {
        return None;
    }
    Some(unsafe { &*ptr.cast::<WeakContainerState>() })
}

fn state_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
) -> Result<(*mut u8, &'static WeakContainerState), u64> {
    let Some(ptr) = maybe_ptr_from_bits(bits) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "weak container state is not an object",
        ));
    };
    let Some(state) = (unsafe { state_from_ptr(ptr) }) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid weak container state",
        ));
    };
    Ok((ptr, state))
}

fn hash_from_bits(bits: u64) -> u64 {
    to_i64(obj_from_bits(bits)).unwrap_or(bits as i64) as u64
}

fn entry_release(_py: &PyToken<'_>, kind: WeakContainerKind, entry: WeakEntry) {
    weakref_detach_container_cookie(_py, entry.weakref_bits);
    dec_ref_bits(_py, entry.weakref_bits);
    if kind.owns_aux() {
        dec_ref_bits(_py, entry.aux_bits);
    }
}

fn entry_slots_release(
    _py: &PyToken<'_>,
    kind: WeakContainerKind,
    entries: Vec<Option<WeakEntry>>,
) {
    for entry in entries.into_iter().flatten() {
        entry_release(_py, kind, entry);
    }
}

#[inline]
fn entry_detach_owned_edges(
    _py: &PyToken<'_>,
    kind: WeakContainerKind,
    entry: WeakEntry,
    sink: &mut DetachedEdgeSink,
) {
    // Publish the reverse cookie empty before either of the entry's strong
    // edges can run terminal code.
    weakref_detach_container_cookie(_py, entry.weakref_bits);
    sink.detach_if_heap(entry.weakref_bits);
    if kind.owns_aux() {
        sink.detach_if_heap(entry.aux_bits);
    }
}

fn entry_slots_detach_owned_edges(
    _py: &PyToken<'_>,
    kind: WeakContainerKind,
    entries: Vec<Option<WeakEntry>>,
    sink: &mut DetachedEdgeSink,
) {
    for entry in entries.into_iter().flatten() {
        entry_detach_owned_edges(_py, kind, entry, sink);
    }
}

fn py_eq_checked(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Result<bool, u64> {
    if lhs_bits == rhs_bits {
        return Ok(true);
    }
    let eq_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let equal = is_truthy(_py, obj_from_bits(eq_bits));
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(equal)
}

fn find_matching(
    _py: &PyToken<'_>,
    state: &WeakContainerState,
    key_bits: u64,
    hash: u64,
) -> Result<(Option<WeakEntryId>, u64), u64> {
    let mut probe_step = 0;
    let mut observed_version = None;
    loop {
        let snapshot = {
            let table = state.read();
            if observed_version != Some(table.structural_version) {
                probe_step = 0;
                observed_version = Some(table.structural_version);
            }
            let Some((id, next_probe_step)) = table.next_candidate(hash, probe_step) else {
                return Ok((None, table.structural_version));
            };
            let Some(entry) = table.live_entry(id) else {
                probe_step = next_probe_step;
                continue;
            };
            inc_ref_bits(_py, entry.weakref_bits);
            if state.kind == WeakContainerKind::ValueDict {
                // The original canonical dict key is state-owned. Pin it only
                // while equality runs outside table custody.
                inc_ref_bits(_py, entry.aux_bits);
            }
            (id, next_probe_step, entry.weakref_bits, entry.aux_bits)
        };
        let (id, next_probe_step, weakref_bits, aux_bits) = snapshot;
        let compare_bits = if state.kind == WeakContainerKind::ValueDict {
            Some(aux_bits)
        } else {
            weakref_peek_owned(_py, weakref_bits)
        };
        let matched = if let Some(compare_bits) = compare_bits {
            let matched = py_eq_checked(_py, compare_bits, key_bits);
            if state.kind != WeakContainerKind::ValueDict {
                dec_ref_bits(_py, compare_bits);
            }
            matched
        } else {
            Ok(false)
        };
        dec_ref_bits(_py, weakref_bits);
        if state.kind == WeakContainerKind::ValueDict {
            dec_ref_bits(_py, aux_bits);
        }
        if matched? {
            if state
                .read()
                .live_entry(id)
                .is_some_and(|entry| entry.state == EntryState::Live)
            {
                return Ok((Some(id), state.read().structural_version));
            }
            // A matching slot was concurrently replaced. Restart because the
            // replacement may have rebuilt the bucket array.
            probe_step = 0;
        } else {
            probe_step = next_probe_step;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_new(kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let kind_raw = to_i64(obj_from_bits(kind_bits)).unwrap_or(-1);
        let Some(kind) = u8::try_from(kind_raw)
            .ok()
            .and_then(WeakContainerKind::from_u8)
        else {
            return raise_exception::<_>(_py, "ValueError", "invalid weak container kind");
        };
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<WeakContainerState>();
        let ptr = alloc_object(_py, total, TYPE_ID_WEAK_CONTAINER_STATE);
        if ptr.is_null() {
            if !exception_pending(_py) {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "weak container allocation failed",
                );
            }
            return MoltObject::none().bits();
        }
        unsafe {
            std::ptr::write(
                ptr.cast::<WeakContainerState>(),
                WeakContainerState {
                    kind,
                    table: RwLock::new(WeakTable::new()),
                },
            )
        };
        bits_from_ptr(ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_store_probe(
    state_bits: u64,
    key_bits: u64,
    value_bits: u64,
    hash_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let hash = hash_from_bits(hash_bits);
        let Some(id) = (match find_matching(_py, state, key_bits, hash) {
            Ok((found, _)) => found,
            Err(bits) => return bits,
        }) else {
            return MoltObject::from_bool(false).bits();
        };
        match state.kind {
            WeakContainerKind::KeyDict => {
                inc_ref_bits(_py, value_bits);
                let old = match state.replace_live_aux(id, value_bits) {
                    Ok(old) => old,
                    Err(error) => {
                        dec_ref_bits(_py, value_bits);
                        return raise_table_error(_py, error);
                    }
                };
                if let Some(old) = old {
                    dec_ref_bits(_py, old);
                    MoltObject::from_bool(true).bits()
                } else {
                    dec_ref_bits(_py, value_bits);
                    MoltObject::from_bool(false).bits()
                }
            }
            WeakContainerKind::Set => {
                MoltObject::from_bool(state.read().live_entry(id).is_some()).bits()
            }
            // CPython replaces KeyedRef even when the referent identity is
            // unchanged so the latest equal supplied key stays owned by it.
            WeakContainerKind::ValueDict => MoltObject::from_bool(false).bits(),
        }
    })
}

fn attach_entry_cookie(
    _py: &PyToken<'_>,
    state_bits: u64,
    weakref_bits: u64,
    id: WeakEntryId,
) -> bool {
    let Some(cookie) = WeakContainerCookie::new(state_bits, id) else {
        return false;
    };
    weakref_attach_container_cookie(_py, weakref_bits, cookie)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_store_commit(
    state_bits: u64,
    key_bits: u64,
    value_bits: u64,
    weakref_bits: u64,
    hash_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if maybe_ptr_from_bits(weakref_bits).is_none() {
            return raise_exception::<_>(_py, "TypeError", "weak container reference is invalid");
        }
        let hash = hash_from_bits(hash_bits);
        let expected_target_bits = match state.kind {
            WeakContainerKind::ValueDict => value_bits,
            WeakContainerKind::KeyDict | WeakContainerKind::Set => key_bits,
        };
        let target_valid = match state.kind {
            WeakContainerKind::KeyDict | WeakContainerKind::Set => {
                weakref_seed_cached_hash(_py, weakref_bits, expected_target_bits, hash as i64)
            }
            WeakContainerKind::ValueDict => {
                weakref_has_live_target(_py, weakref_bits, expected_target_bits)
            }
        };
        if !target_valid {
            return raise_exception::<_>(
                _py,
                "ReferenceError",
                "weak container reference does not match its live referent",
            );
        }
        loop {
            let (existing, observed_version) = match find_matching(_py, state, key_bits, hash) {
                Ok(found) => found,
                Err(bits) => return bits,
            };
            let Some(id) = existing else {
                let mut prepared = match prepare_insert(_py, state) {
                    Ok(prepared) => prepared,
                    Err(bits) => return bits,
                };
                inc_ref_bits(_py, weakref_bits);
                let aux_bits = match state.kind {
                    WeakContainerKind::KeyDict => value_bits,
                    WeakContainerKind::ValueDict => key_bits,
                    WeakContainerKind::Set => MoltObject::none().bits(),
                };
                if state.kind.owns_aux() {
                    inc_ref_bits(_py, aux_bits);
                }
                let inserted = (|| -> Result<_, WeakTableError> {
                    let mut table = state.write();
                    if table.structural_version != observed_version {
                        Ok(None)
                    } else {
                        let admission = table.admit_insert()?;
                        let displaced = table.install_prepared_insert(&mut prepared)?;
                        let id = table.insert_entry(hash, weakref_bits, aux_bits, admission)?;
                        Ok(Some((id, displaced)))
                    }
                })();
                let inserted = match inserted {
                    Ok(inserted) => inserted,
                    Err(error) => {
                        dec_ref_bits(_py, weakref_bits);
                        if state.kind.owns_aux() {
                            dec_ref_bits(_py, aux_bits);
                        }
                        return raise_table_error(_py, error);
                    }
                };
                let Some((id, displaced)) = inserted else {
                    dec_ref_bits(_py, weakref_bits);
                    if state.kind.owns_aux() {
                        dec_ref_bits(_py, aux_bits);
                    }
                    continue;
                };
                drop(displaced);
                if !attach_entry_cookie(_py, state_bits, weakref_bits, id) {
                    // The insertion already advanced structural_version, so
                    // rollback does not consume another iterator identity.
                    let detached = state.write().detach_entry(id, false).ok().flatten();
                    if let Some(entry) = detached {
                        entry_release(_py, state.kind, entry);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ReferenceError",
                        "weak referent died during insertion",
                    );
                }
                let still_current = state
                    .read()
                    .live_entry(id)
                    .is_some_and(|entry| entry.weakref_bits == weakref_bits);
                if !still_current {
                    weakref_detach_container_cookie(_py, weakref_bits);
                }
                return MoltObject::none().bits();
            };
            match state.kind {
                WeakContainerKind::KeyDict => {
                    inc_ref_bits(_py, value_bits);
                    let old = match state.replace_live_aux(id, value_bits) {
                        Ok(old) => old,
                        Err(error) => {
                            dec_ref_bits(_py, value_bits);
                            return raise_table_error(_py, error);
                        }
                    };
                    if let Some(old) = old {
                        dec_ref_bits(_py, old);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, value_bits);
                }
                WeakContainerKind::Set => {
                    if state.read().live_entry(id).is_some() {
                        return MoltObject::none().bits();
                    }
                }
                WeakContainerKind::ValueDict => {
                    inc_ref_bits(_py, weakref_bits);
                    let replaced = (|| -> Result<Option<(u64, WeakEntryId)>, WeakTableError> {
                        let mut table = state.write();
                        let slot = id.slot as usize;
                        if table.live_entry(id).is_none() {
                            Ok(None)
                        } else {
                            let generation = table.next_generation()?;
                            let next_generation = generation
                                .checked_add(1)
                                .ok_or(WeakTableError::GenerationExhausted)?;
                            table.reserve_structural_version()?;
                            table.next_generation = next_generation;
                            let old = {
                                let Some(entry) =
                                    table.entries.get_mut(slot).and_then(Option::as_mut)
                                else {
                                    table.release_structural_reservation()?;
                                    return Err(WeakTableError::InvalidPreparedCapacity);
                                };
                                entry.generation = generation;
                                std::mem::replace(&mut entry.weakref_bits, weakref_bits)
                            };
                            let new_id = WeakEntryId {
                                slot: slot as u32,
                                generation,
                            };
                            Ok(Some((old, new_id)))
                        }
                    })();
                    let replaced = match replaced {
                        Ok(replaced) => replaced,
                        Err(error) => {
                            dec_ref_bits(_py, weakref_bits);
                            return raise_table_error(_py, error);
                        }
                    };
                    let Some((old, new_id)) = replaced else {
                        dec_ref_bits(_py, weakref_bits);
                        continue;
                    };
                    weakref_detach_container_cookie(_py, old);
                    if !attach_entry_cookie(_py, state_bits, weakref_bits, new_id) {
                        let detached = {
                            let mut table = state.write();
                            if table.entry(new_id).is_some() {
                                if let Err(error) = table.consume_structural_reservation() {
                                    return raise_table_error(_py, error);
                                }
                                table.detach_entry(new_id, false).ok().flatten()
                            } else {
                                if let Err(error) = table.release_structural_reservation() {
                                    return raise_table_error(_py, error);
                                }
                                None
                            }
                        };
                        if let Some(entry) = detached {
                            entry_release(_py, state.kind, entry);
                        }
                        dec_ref_bits(_py, old);
                        return raise_exception::<_>(
                            _py,
                            "ReferenceError",
                            "weak value died during insertion",
                        );
                    }
                    let still_current = {
                        let mut table = state.write();
                        let current = table
                            .live_entry(new_id)
                            .is_some_and(|entry| entry.weakref_bits == weakref_bits);
                        if let Err(error) = table.release_structural_reservation() {
                            return raise_table_error(_py, error);
                        }
                        current
                    };
                    if !still_current {
                        weakref_detach_container_cookie(_py, weakref_bits);
                    }
                    dec_ref_bits(_py, old);
                    return MoltObject::none().bits();
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_get(state_bits: u64, key_bits: u64, hash_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let hash = hash_from_bits(hash_bits);
        let Some(id) = (match find_matching(_py, state, key_bits, hash) {
            Ok((found, _)) => found,
            Err(bits) => return bits,
        }) else {
            return raise_exception::<_>(_py, "KeyError", "weak container key not found");
        };
        let snapshot = {
            let table = state.read();
            table.live_entry(id).map(|entry| {
                match state.kind {
                    WeakContainerKind::KeyDict => inc_ref_bits(_py, entry.aux_bits),
                    WeakContainerKind::ValueDict | WeakContainerKind::Set => {
                        inc_ref_bits(_py, entry.weakref_bits)
                    }
                }
                (entry.weakref_bits, entry.aux_bits)
            })
        };
        let result = snapshot.and_then(|(weakref_bits, aux_bits)| match state.kind {
            WeakContainerKind::KeyDict => Some(aux_bits),
            WeakContainerKind::ValueDict | WeakContainerKind::Set => {
                let result = weakref_peek_owned(_py, weakref_bits);
                dec_ref_bits(_py, weakref_bits);
                result
            }
        });
        result.unwrap_or_else(|| {
            raise_exception::<u64>(_py, "KeyError", "weak container key not found")
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_take(
    state_bits: u64,
    key_bits: u64,
    hash_bits: u64,
    raise_missing_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let raise_missing = is_truthy(_py, obj_from_bits(raise_missing_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let hash = hash_from_bits(hash_bits);
        let Some(id) = (match find_matching(_py, state, key_bits, hash) {
            Ok((found, _)) => found,
            Err(bits) => return bits,
        }) else {
            if raise_missing {
                return raise_exception::<_>(_py, "KeyError", "weak container key not found");
            }
            return MoltObject::none().bits();
        };
        let detached = match state.write().detach_live_entry(id, true) {
            Ok(detached) => detached,
            Err(error) => return raise_table_error(_py, error),
        };
        let result = detached.and_then(|entry| match state.kind {
            WeakContainerKind::KeyDict => {
                weakref_detach_container_cookie(_py, entry.weakref_bits);
                dec_ref_bits(_py, entry.weakref_bits);
                // Transfer the state's owned value edge to the caller.
                Some(entry.aux_bits)
            }
            WeakContainerKind::ValueDict | WeakContainerKind::Set => {
                let result = weakref_peek_owned(_py, entry.weakref_bits);
                entry_release(_py, state.kind, entry);
                result
            }
        });
        result.unwrap_or_else(|| {
            if raise_missing {
                raise_exception::<u64>(_py, "KeyError", "weak container key not found")
            } else {
                MoltObject::none().bits()
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_contains(
    state_bits: u64,
    key_bits: u64,
    hash_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let found = match find_matching(_py, state, key_bits, hash_from_bits(hash_bits)) {
            Ok((found, _)) => found.is_some(),
            Err(bits) => return bits,
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_len(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if state.kind == WeakContainerKind::ValueDict {
            let mut structural = true;
            loop {
                let detached = match state.write().detach_next_pending(structural) {
                    Ok(detached) => detached,
                    Err(error) => return raise_table_error(_py, error),
                };
                let Some(entry) = detached else { break };
                structural = false;
                entry_release(_py, state.kind, entry);
            }
        }
        let live_len = state.read().live_len;
        int_bits_from_i64(_py, live_len as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_iter(state_bits: u64, projection_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if state_from_bits(_py, state_bits).is_err() {
            return MoltObject::none().bits();
        }
        let projection = to_i64(obj_from_bits(projection_bits)).unwrap_or(-1);
        if !matches!(projection, 1..=3) {
            return raise_exception::<_>(_py, "ValueError", "invalid weak iterator projection");
        }
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<u64>()
            + std::mem::size_of::<usize>()
            + std::mem::size_of::<*mut u8>()
            + 2 * std::mem::size_of::<u64>();
        let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
        if iter_ptr.is_null() {
            if !exception_pending(_py) {
                return raise_exception::<_>(_py, "MemoryError", "weak iterator allocation failed");
            }
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, state_bits);
        unsafe {
            *(iter_ptr as *mut u64) = state_bits;
            iter_set_index(iter_ptr, 0);
            iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
            iter_set_expected_version(iter_ptr, WEAK_ITER_VERSION_UNSTARTED);
            iter_set_projection(iter_ptr, projection as u64);
        }
        bits_from_ptr(iter_ptr)
    })
}

pub(crate) fn weakcontainer_iter_begin(
    _py: &PyToken<'_>,
    state_ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return Ok(None);
    };
    let mut table = state.write();
    let Some(next) = table.active_iterators.checked_add(1) else {
        return Err(raise_table_error(
            _py,
            WeakTableError::IteratorCountExhausted,
        ));
    };
    table.active_iterators = next;
    Ok(Some(table.structural_version))
}

pub(crate) fn weakcontainer_iter_finish(_py: &PyToken<'_>, state_ptr: *mut u8) {
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return;
    };
    let drain = match state.write().finish_iterator() {
        Ok(drain) => drain,
        Err(error) => {
            raise_table_error(_py, error);
            return;
        }
    };
    if drain {
        loop {
            let detached = state.write().detach_next_pending(false).ok().flatten();
            let Some(entry) = detached else { break };
            entry_release(_py, state.kind, entry);
        }
    }
}

pub(crate) fn weakcontainer_iter_finish_detach(
    _py: &PyToken<'_>,
    state_ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return;
    };
    let drain = state
        .write()
        .finish_iterator()
        .unwrap_or_else(|_| std::process::abort());
    if drain {
        loop {
            let detached = state
                .write()
                .detach_next_pending(false)
                .unwrap_or_else(|_| std::process::abort());
            let Some(entry) = detached else { break };
            entry_detach_owned_edges(_py, state.kind, entry, sink);
        }
    }
}

pub(crate) fn weakcontainer_iter_next_value(
    _py: &PyToken<'_>,
    state_ptr: *mut u8,
    cursor: usize,
    expected_version: u64,
    projection: u8,
) -> Result<(usize, Option<u64>), u64> {
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return Ok((cursor, None));
    };
    loop {
        let snapshot = {
            let table = state.read();
            if table.structural_version != expected_version {
                None
            } else if cursor == usize::MAX {
                return Ok((cursor, None));
            } else {
                let mut slot = if cursor == 0 {
                    table.order_head
                } else {
                    (cursor - 1) as u32
                };
                let mut found = None;
                while slot != SLOT_NONE {
                    let current = slot;
                    let Some(entry) = table.entries[current as usize].as_ref() else {
                        break;
                    };
                    slot = entry.order_next;
                    if entry.state != EntryState::Live {
                        continue;
                    }
                    inc_ref_bits(_py, entry.weakref_bits);
                    if !obj_from_bits(entry.aux_bits).is_none() {
                        inc_ref_bits(_py, entry.aux_bits);
                    }
                    let next = if slot == SLOT_NONE {
                        usize::MAX
                    } else {
                        slot as usize + 1
                    };
                    found = Some((
                        entry.id(current as usize),
                        next,
                        entry.weakref_bits,
                        entry.aux_bits,
                    ));
                    break;
                }
                Some(found)
            }
        };
        let Some(snapshot) = snapshot else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                state.kind.mutation_error(),
            ));
        };
        let Some((id, next, weakref_bits, aux_bits)) = snapshot else {
            return Ok((cursor, None));
        };
        let referent = weakref_peek_owned(_py, weakref_bits);
        if referent.is_none() {
            dec_ref_bits(_py, weakref_bits);
            if !obj_from_bits(aux_bits).is_none() {
                dec_ref_bits(_py, aux_bits);
            }
            let detached = match state.write().target_dead(id) {
                Ok(detached) => detached,
                Err(error) => return Err(raise_table_error(_py, error)),
            };
            if let Some(entry) = detached {
                entry_release(_py, state.kind, entry);
            }
            continue;
        }
        let value = match (state.kind, projection) {
            (WeakContainerKind::KeyDict, WEAK_CONTAINER_PROJECTION_KEYS) => referent,
            (WeakContainerKind::KeyDict, WEAK_CONTAINER_PROJECTION_VALUES) => {
                inc_ref_bits(_py, aux_bits);
                Some(aux_bits)
            }
            (WeakContainerKind::KeyDict, WEAK_CONTAINER_PROJECTION_ITEMS) => {
                let key = referent.unwrap_or(MoltObject::none().bits());
                let tuple = alloc_tuple(_py, &[key, aux_bits]);
                if tuple.is_null() {
                    None
                } else {
                    Some(bits_from_ptr(tuple))
                }
            }
            (WeakContainerKind::ValueDict, WEAK_CONTAINER_PROJECTION_KEYS) => {
                inc_ref_bits(_py, aux_bits);
                Some(aux_bits)
            }
            (WeakContainerKind::ValueDict, WEAK_CONTAINER_PROJECTION_VALUES) => referent,
            (WeakContainerKind::ValueDict, WEAK_CONTAINER_PROJECTION_ITEMS) => {
                let Some(value) = referent else {
                    dec_ref_bits(_py, weakref_bits);
                    dec_ref_bits(_py, aux_bits);
                    let detached = match state.write().target_dead(id) {
                        Ok(detached) => detached,
                        Err(error) => return Err(raise_table_error(_py, error)),
                    };
                    if let Some(entry) = detached {
                        entry_release(_py, state.kind, entry);
                    }
                    continue;
                };
                let tuple = alloc_tuple(_py, &[aux_bits, value]);
                if tuple.is_null() {
                    None
                } else {
                    Some(bits_from_ptr(tuple))
                }
            }
            (WeakContainerKind::Set, _) => referent,
            _ => None,
        };
        if let Some(bits) = referent
            && value != Some(bits)
        {
            dec_ref_bits(_py, bits);
        }
        dec_ref_bits(_py, weakref_bits);
        if !obj_from_bits(aux_bits).is_none() {
            dec_ref_bits(_py, aux_bits);
        }
        if value.is_none() && exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok((next, value));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_refs(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let refs: Vec<u64> = loop {
            let capacity = state.read().live_len;
            let mut refs = Vec::new();
            if refs.try_reserve_exact(capacity).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "weak container reference snapshot allocation failed",
                );
            }
            let table = state.read();
            if table.live_len > capacity {
                continue;
            }
            let mut slot = table.order_head;
            let mut invalid_order = false;
            while slot != SLOT_NONE {
                let current = slot;
                let Some(entry) = table.entries.get(current as usize).and_then(Option::as_ref)
                else {
                    invalid_order = true;
                    break;
                };
                slot = entry.order_next;
                if entry.state == EntryState::Live {
                    inc_ref_bits(_py, entry.weakref_bits);
                    refs.push(entry.weakref_bits);
                }
            }
            drop(table);
            if invalid_order {
                for weakref_bits in refs {
                    dec_ref_bits(_py, weakref_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "weak container order index is corrupt",
                );
            }
            break refs;
        };
        let ptr = alloc_list(_py, &refs);
        for weakref_bits in refs {
            dec_ref_bits(_py, weakref_bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            bits_from_ptr(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_pop(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        loop {
            let id = {
                let table = state.read();
                let mut slot = table.order_tail;
                let mut found = None;
                while slot != SLOT_NONE {
                    let current = slot;
                    let Some(entry) = table.entries.get(current as usize).and_then(Option::as_ref)
                    else {
                        return raise_exception::<_>(
                            _py,
                            "RuntimeError",
                            "weak container order index is corrupt",
                        );
                    };
                    slot = entry.order_prev;
                    if entry.state == EntryState::Live {
                        found = Some(entry.id(current as usize));
                        break;
                    }
                }
                found
            };
            let Some(id) = id else {
                let msg = if state.kind == WeakContainerKind::Set {
                    "pop from empty WeakSet"
                } else {
                    "popitem(): dictionary is empty"
                };
                return raise_exception::<_>(_py, "KeyError", msg);
            };
            let snapshot = {
                let table = state.read();
                let Some(entry) = table.live_entry(id) else {
                    continue;
                };
                inc_ref_bits(_py, entry.weakref_bits);
                if !obj_from_bits(entry.aux_bits).is_none() {
                    inc_ref_bits(_py, entry.aux_bits);
                }
                (entry.weakref_bits, entry.aux_bits, entry.content_version)
            };
            let referent = weakref_peek_owned(_py, snapshot.0);
            let result = match state.kind {
                WeakContainerKind::KeyDict => referent.and_then(|key| {
                    let tuple = alloc_tuple(_py, &[key, snapshot.1]);
                    dec_ref_bits(_py, key);
                    (!tuple.is_null()).then(|| bits_from_ptr(tuple))
                }),
                WeakContainerKind::ValueDict => referent.and_then(|value| {
                    let tuple = alloc_tuple(_py, &[snapshot.1, value]);
                    dec_ref_bits(_py, value);
                    (!tuple.is_null()).then(|| bits_from_ptr(tuple))
                }),
                WeakContainerKind::Set => referent,
            };
            dec_ref_bits(_py, snapshot.0);
            if !obj_from_bits(snapshot.1).is_none() {
                dec_ref_bits(_py, snapshot.1);
            }
            if result.is_none() && exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let detached = match state
                .write()
                .detach_live_entry_versioned(id, snapshot.2, true)
            {
                Ok(detached) => detached,
                Err(error) => {
                    if let Some(result) = result {
                        dec_ref_bits(_py, result);
                    }
                    return raise_table_error(_py, error);
                }
            };
            if let Some(entry) = detached {
                entry_release(_py, state.kind, entry);
                if let Some(result) = result {
                    return result;
                }
            } else if let Some(result) = result {
                dec_ref_bits(_py, result);
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_clear(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let entries = match state.write().detach_all(true) {
            Ok(entries) => entries,
            Err(error) => return raise_table_error(_py, error),
        };
        entry_slots_release(_py, state.kind, entries);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakcontainer_dead(state_bits: u64, weakref_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (_, state) = match state_from_bits(_py, state_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(cookie) = weakref_container_cookie(_py, weakref_bits) else {
            return MoltObject::none().bits();
        };
        if cookie.state_bits() != state_bits {
            return MoltObject::none().bits();
        }
        let current = {
            let table = state.read();
            table
                .live_entry(cookie.entry)
                .is_some_and(|entry| entry.weakref_bits == weakref_bits)
        };
        if !current {
            return MoltObject::none().bits();
        }
        // Manual callback parity: WeakValue only removes a genuinely dead,
        // still-current KeyedRef. WeakKey/WeakSet callbacks remove immediately.
        if state.kind == WeakContainerKind::ValueDict
            && let Some(target_bits) = weakref_peek_owned(_py, weakref_bits)
        {
            dec_ref_bits(_py, target_bits);
            return MoltObject::none().bits();
        }
        {
            let detached = match state.write().target_dead(cookie.entry) {
                Ok(detached) => detached,
                Err(error) => return raise_table_error(_py, error),
            };
            if let Some(entry) = detached {
                entry_release(_py, state.kind, entry);
            }
        }
        MoltObject::none().bits()
    })
}

pub(crate) fn weakcontainer_target_dead(_py: &PyToken<'_>, cookie: WeakContainerCookie) {
    let Some(state_ptr) = maybe_ptr_from_bits(cookie.state_bits()) else {
        return;
    };
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return;
    };
    let detached = match state.write().target_dead(cookie.entry) {
        Ok(detached) => detached,
        Err(error) => {
            raise_table_error(_py, error);
            return;
        }
    };
    if let Some(entry) = detached {
        entry_release(_py, state.kind, entry);
    }
}

pub(crate) fn weakcontainer_target_dead_detach(
    _py: &PyToken<'_>,
    cookie: WeakContainerCookie,
    sink: &mut DetachedEdgeSink,
) {
    let Some(state_ptr) = maybe_ptr_from_bits(cookie.state_bits()) else {
        return;
    };
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return;
    };
    let detached = state
        .write()
        .target_dead(cookie.entry)
        .unwrap_or_else(|_| std::process::abort());
    if let Some(entry) = detached {
        entry_detach_owned_edges(_py, state.kind, entry, sink);
    }
}

pub(crate) fn weakcontainer_target_dead_detach_edge_count(cookie: WeakContainerCookie) -> usize {
    let Some(state_ptr) = maybe_ptr_from_bits(cookie.state_bits()) else {
        return 0;
    };
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return 0;
    };
    let table = state.read();
    if table.active_iterators != 0 {
        return 0;
    }
    let Some(entry) = table.entry(cookie.entry) else {
        return 0;
    };
    if entry.state != EntryState::Live {
        return 0;
    }
    usize::from(maybe_ptr_from_bits(entry.weakref_bits).is_some())
        + usize::from(state.kind.owns_aux() && maybe_ptr_from_bits(entry.aux_bits).is_some())
}

pub(crate) fn weakcontainer_iter_finish_detach_edge_count(state_ptr: *mut u8) -> usize {
    let Some(state) = (unsafe { state_from_ptr(state_ptr) }) else {
        return 0;
    };
    let table = state.read();
    if table.active_iterators != 1 {
        return 0;
    }
    table
        .entries
        .iter()
        .filter_map(Option::as_ref)
        .filter(|entry| entry.state == EntryState::PendingDead)
        .map(|entry| {
            usize::from(maybe_ptr_from_bits(entry.weakref_bits).is_some())
                + usize::from(
                    state.kind.owns_aux() && maybe_ptr_from_bits(entry.aux_bits).is_some(),
                )
        })
        .sum()
}

pub(crate) unsafe fn weakcontainer_traverse(ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    let Some(state) = (unsafe { state_from_ptr(ptr) }) else {
        return;
    };
    // Cycle collection owns a stop-the-world epoch, so the internal visitor
    // may borrow state edges directly without per-object scratch allocation.
    let table = state.read();
    for entry in table.entries.iter().filter_map(Option::as_ref) {
        if let Some(child) = maybe_ptr_from_bits(entry.weakref_bits) {
            visit(child);
        }
        if state.kind.owns_aux()
            && let Some(child) = maybe_ptr_from_bits(entry.aux_bits)
        {
            visit(child);
        }
    }
}

pub(crate) unsafe fn weakcontainer_detach_state(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    let Some(state) = (unsafe { state_from_ptr(ptr) }) else {
        return;
    };
    let entries = state
        .write()
        .detach_all(false)
        .unwrap_or_else(|_| std::process::abort());
    entry_slots_detach_owned_edges(_py, state.kind, entries, sink);
}

pub(crate) unsafe fn weakcontainer_drop_detached_state(ptr: *mut u8) {
    unsafe { std::ptr::drop_in_place(ptr.cast::<WeakContainerState>()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(table: &mut WeakTable, hash: u64, weakref_bits: u64, aux_bits: u64) -> WeakEntryId {
        if table
            .needs_bucket_rebuild_for_insert()
            .expect("load arithmetic")
        {
            let capacity = WeakTable::capacity_for(table.live_len + 1).expect("table capacity");
            if table.buckets.capacity() < capacity {
                table.buckets.reserve_exact(capacity);
            }
            table.rebuild(capacity).expect("table rebuild");
        }
        if table.free_slots.is_empty() && table.entries.len() == table.entries.capacity() {
            table.entries.reserve(8.max(table.entries.len()));
        }
        if table.free_slots.capacity() < table.entries.capacity() {
            table.free_slots.reserve(table.entries.capacity());
        }
        let admission = table.admit_insert().expect("test insertion admission");
        table
            .insert_entry(hash, weakref_bits, aux_bits, admission)
            .expect("test insertion")
    }

    #[test]
    fn weak_entry_stays_cache_line_sized() {
        assert!(std::mem::size_of::<WeakEntry>() <= 64);
        assert_eq!(std::mem::size_of::<u32>(), 4);
        assert!(
            std::mem::align_of::<WeakContainerState>()
                <= molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES
        );
    }

    #[test]
    fn cached_hash_table_gates_mismatched_hashes() {
        let mut table = WeakTable::new();
        let a = insert(&mut table, 1, 11, 21);
        let b = insert(&mut table, 9, 12, 22);
        assert_eq!(table.next_candidate(1, 0), Some((a, 1)));
        assert_eq!(table.next_candidate(9, 0), Some((b, 2)));
    }

    #[test]
    fn generation_rejects_reused_slot_cookie() {
        let mut table = WeakTable::new();
        let old = insert(&mut table, 1, 11, 21);
        let _ = table
            .detach_entry(old, true)
            .expect("structural version")
            .expect("old entry");
        let new = insert(&mut table, 2, 12, 22);
        assert_eq!(old.slot, new.slot);
        assert_ne!(old.generation, new.generation);
        assert!(table.entry(old).is_none());
        assert!(table.entry(new).is_some());
    }

    #[test]
    fn weakvalue_canonical_key_is_a_state_owned_gc_edge() {
        assert_eq!(WeakContainerKind::KeyDict.owned_edges_per_entry(), 2);
        assert_eq!(WeakContainerKind::ValueDict.owned_edges_per_entry(), 2);
        assert_eq!(WeakContainerKind::Set.owned_edges_per_entry(), 1);
    }

    #[test]
    fn clear_refill_storage_is_bounded_by_peak_live_slots() {
        let mut table = WeakTable::new();
        for value in 0..64 {
            insert(&mut table, value, value + 1, value + 101);
        }
        let peak_capacity = table.entries.capacity();
        let detached = table.detach_all(true).expect("clear admission");
        assert_eq!(detached.len(), 64);
        assert!(table.entries.is_empty());
        for value in 0..64 {
            insert(&mut table, value, value + 1, value + 101);
        }
        assert!(table.entries.capacity() <= peak_capacity);
        assert_eq!(table.live_len, 64);
    }

    #[test]
    fn prepared_hot_mutations_do_not_grow_storage_under_custody() {
        let mut table = WeakTable::new();
        let capacity = WeakTable::capacity_for(1).expect("bucket capacity");
        let mut prepared = PreparedInsert {
            buckets: Some(vec![0; capacity]),
            bucket_len: Some(capacity),
            entries: Some(reserved_vec(8).expect("entry storage")),
            free_slots: Some(reserved_vec(8).expect("free-slot storage")),
        };
        drop(
            table
                .install_prepared_insert(&mut prepared)
                .expect("prepared install"),
        );
        let capacities = (
            table.buckets.capacity(),
            table.entries.capacity(),
            table.free_slots.capacity(),
        );
        let first_admission = table.admit_insert().expect("first admission");
        let first = table
            .insert_entry(1, 11, 21, first_admission)
            .expect("first insert");
        let _ = table
            .detach_live_entry(first, true)
            .expect("detach admission")
            .expect("detach");
        let second_admission = table.admit_insert().expect("second admission");
        table
            .insert_entry(2, 12, 22, second_admission)
            .expect("second insert");
        assert_eq!(
            capacities,
            (
                table.buckets.capacity(),
                table.entries.capacity(),
                table.free_slots.capacity(),
            )
        );
    }

    #[test]
    fn pending_deaths_drain_fifo_in_one_visit_each() {
        let mut table = WeakTable::new();
        let first = insert(&mut table, 1, 11, 21);
        let second = insert(&mut table, 2, 12, 22);
        let third = insert(&mut table, 3, 13, 23);
        table.active_iterators = 1;
        assert!(table.target_dead(second).expect("second death").is_none());
        assert!(table.target_dead(third).expect("third death").is_none());
        assert_eq!(table.pending_head, second.slot);
        assert_eq!(table.pending_tail, third.slot);

        let drained_second = table
            .detach_next_pending(false)
            .expect("pending drain")
            .expect("second pending");
        let drained_third = table
            .detach_next_pending(false)
            .expect("pending drain")
            .expect("third pending");
        assert_eq!(drained_second.generation, second.generation);
        assert_eq!(drained_third.generation, third.generation);
        assert!(
            table
                .detach_next_pending(false)
                .expect("empty pending drain")
                .is_none()
        );
        assert_eq!(table.pending_queue_visits, 2);
        assert!(table.live_entry(first).is_some());
    }

    #[test]
    fn identity_sentinels_never_collide_with_live_domains() {
        const {
            assert!(MAX_STRUCTURAL_VERSION < WEAK_ITER_VERSION_FINISHED);
            assert!(WEAK_ITER_VERSION_FINISHED < WEAK_ITER_VERSION_UNSTARTED);
        }
        assert_eq!(GENERATION_EXHAUSTED, u64::MAX);
        assert_eq!(SLOT_NONE, TABLE_TOMBSTONE);
        assert!(MAX_SLOT_COUNT <= u32::MAX as usize);
    }

    #[test]
    fn exhausted_counters_and_slot_domain_fail_before_mutation() {
        let mut table = WeakTable::new();
        let id = insert(&mut table, 1, 11, 21);
        table.structural_version = MAX_STRUCTURAL_VERSION;
        assert!(matches!(
            table.detach_live_entry(id, true),
            Err(WeakTableError::StructuralVersionExhausted)
        ));
        assert!(table.live_entry(id).is_some());

        table.next_generation = GENERATION_EXHAUSTED;
        assert_eq!(
            table.admit_insert().map(|_| ()),
            Err(WeakTableError::GenerationExhausted)
        );
        assert!(!WeakTable::slot_domain_admits(MAX_SLOT_COUNT, false));
        assert!(WeakTable::slot_domain_admits(MAX_SLOT_COUNT, true));

        let entry = table.live_entry_mut(id).expect("live entry");
        entry.content_version = u64::MAX;
        assert!(entry.content_version.checked_add(1).is_none());
    }

    #[test]
    fn capacity_and_counter_boundaries_fail_without_wrapping() {
        assert_eq!(
            WeakTable::capacity_for(usize::MAX),
            Err(WeakTableError::TableCapacityExhausted)
        );
        assert_eq!(
            WeakTable::capacity_for(usize::MAX / 2 + 1),
            Err(WeakTableError::TableCapacityExhausted)
        );

        let mut table = WeakTable::new();
        table.buckets = vec![0; 8];
        table.live_len = usize::MAX;
        assert_eq!(
            table.needs_bucket_rebuild_for_insert(),
            Err(WeakTableError::TableCapacityExhausted)
        );

        table.live_len = 0;
        table.structural_version = MAX_STRUCTURAL_VERSION;
        table.structural_reservations = 1;
        assert_eq!(
            table.consume_structural_reservation(),
            Err(WeakTableError::StructuralVersionExhausted)
        );
        assert_eq!(table.structural_reservations, 1);
        assert_eq!(table.structural_version, MAX_STRUCTURAL_VERSION);

        table.structural_reservations = 0;
        assert_eq!(
            table.release_structural_reservation(),
            Err(WeakTableError::StructuralVersionExhausted)
        );
        assert_eq!(
            table.finish_iterator(),
            Err(WeakTableError::IteratorCountExhausted)
        );

        table.free_slots.push(MAX_SLOT_COUNT);
        assert_eq!(
            table.admit_insert().map(|_| ()),
            Err(WeakTableError::SlotDomainExhausted)
        );
    }
}
