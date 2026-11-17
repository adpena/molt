//! Raw type lookup observations. Namespace owners retain the values; this
//! cache owns only exact-string keys so address reuse cannot alias a key.
//!
//! Two stores, split by what an eviction would expose:
//!
//! - A callback-free result sits in a bounded direct-mapped table. Losing it
//!   to a collision is invisible: the lookup runs again, runs no Python, and
//!   finds the same value.
//! - A callback-dependent result (the class dictionary probe ran a key's
//!   `__eq__`) is CPython's method-cache rule: it stays valid until a class
//!   changes, even if the callback would now answer differently. Its lifetime
//!   must not depend on which other lookups share its slot, because slots are
//!   chosen by heap address and differ per build (HF-886). So it lives in a
//!   record that never evicts within one global type epoch; the first
//!   publication after the epoch moves drops every older entry.
use crate::object::gc::RegistryAddressHash;
use crate::{
    MoltObject, PyToken, TYPE_ID_STRING, dec_ref_bits, inc_ref_bits, obj_from_bits,
    object_class_bits, object_type_id, string_len,
};
use std::collections::HashMap;

const SLOTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LookupKey {
    class: u64,
    layout: u64,
    epoch: u64,
    name: u64,
}

impl LookupKey {
    pub(super) unsafe fn new(py: &PyToken<'_>, class: *mut u8, name: u64) -> Option<Self> {
        unsafe {
            let pointer = obj_from_bits(name).as_ptr()?;
            if object_type_id(pointer) != TYPE_ID_STRING {
                return None;
            }
            // Exact strings have no user finalizer. Bounded length also bounds
            // retained name memory independently of a caller's arbitrary input.
            let owner = object_class_bits(pointer);
            if string_len(pointer) > 100 || (owner != 0 && owner != crate::builtin_classes(py).str)
            {
                return None;
            }
            Some(Self {
                class: MoltObject::from_ptr(class).bits(),
                layout: crate::class_layout_version_bits(class),
                epoch: crate::object::global_type_version(),
                name,
            })
        }
    }
    fn slot(self) -> usize {
        (((self.class >> 4) ^ (self.name >> 4)) as usize) % SLOTS
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum LookupPurity {
    CallbackFree,
    CallbackDependent,
}

#[derive(Clone, Copy)]
pub(super) struct LookupValue {
    pub(super) value: Option<u64>,
    pub(super) name: Option<crate::NameId>,
    pub(super) purity: LookupPurity,
}

#[derive(Clone, Copy)]
struct Entry {
    key: LookupKey,
    value: LookupValue,
}

/// A class and a name; the entry's full key still decides a hit.
#[derive(Clone, Copy, Eq, PartialEq)]
struct PinnedKey {
    class: u64,
    name: u64,
}

impl std::hash::Hash for PinnedKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_usize((self.class ^ self.name.rotate_left(29)) as usize);
    }
}

struct Store {
    slots: Box<[Option<Entry>; SLOTS]>,
    /// Callback-dependent results of `epoch`; see the module comment.
    pinned: HashMap<PinnedKey, Entry, RegistryAddressHash>,
    epoch: u64,
}

impl Store {
    fn new() -> Self {
        Self {
            slots: Box::new([None; SLOTS]),
            pinned: HashMap::with_hasher(RegistryAddressHash::new()),
            epoch: 0,
        }
    }

    fn get(&self, key: LookupKey) -> Option<LookupValue> {
        let hit = |entry: &Entry| (entry.key == key).then_some(entry.value);
        if let Some(value) = self.slots[key.slot()].as_ref().and_then(hit) {
            return Some(value);
        }
        if self.pinned.is_empty() || self.epoch != key.epoch {
            return None;
        }
        self.pinned
            .get(&PinnedKey {
                class: key.class,
                name: key.name,
            })
            .and_then(hit)
    }

    /// Stores the entry. Returns the name of the entry it replaced, and the
    /// names of an older epoch's record, which only an epoch change empties.
    fn put(&mut self, entry: Entry) -> (Option<u64>, Vec<u64>) {
        let key = entry.key;
        if entry.value.purity == LookupPurity::CallbackFree {
            let replaced = self.slots[key.slot()].replace(entry);
            return (replaced.map(|old| old.key.name), Vec::new());
        }
        let expired = if self.epoch == key.epoch {
            Vec::new()
        } else {
            self.epoch = key.epoch;
            self.pinned.drain().map(|(_, old)| old.key.name).collect()
        };
        let pinned = PinnedKey {
            class: key.class,
            name: key.name,
        };
        let replaced = self.pinned.insert(pinned, entry);
        (replaced.map(|old| old.key.name), expired)
    }

    fn take(&mut self) -> impl Iterator<Item = Entry> {
        let slots = std::mem::replace(&mut *self.slots, [None; SLOTS]);
        let pinned = std::mem::take(&mut self.pinned);
        slots.into_iter().flatten().chain(pinned.into_values())
    }
}

pub(crate) struct ClassLookupCache {
    // Keep this bounded cold table out of the runtime state's hot field layout.
    entries: Table,
}

/// The table. In a GIL build every access holds the GIL, which each method
/// proves with its `PyToken`, so the table is a plain cell: an attribute
/// lookup took a mutex lock and unlock it never contended (3.7% of
/// bench_struct's samples). Free-threaded execution keeps the mutex, as
/// list storage keeps its atomics (`object::backing`).
#[cfg(not(feature = "free-threaded"))]
struct Table(std::cell::UnsafeCell<Store>);

// SAFETY: every access runs `Table::with`, which takes the `PyToken` that
// only the GIL holder has; no access re-enters the table.
#[cfg(not(feature = "free-threaded"))]
unsafe impl Sync for Table {}

// An access copies or replaces whole `Copy` entries, so a panic cannot leave
// a torn table; the mutex it replaces ignored poisoning for the same reason.
#[cfg(not(feature = "free-threaded"))]
impl std::panic::RefUnwindSafe for Table {}

#[cfg(not(feature = "free-threaded"))]
impl Table {
    fn new() -> Self {
        Self(std::cell::UnsafeCell::new(Store::new()))
    }

    fn with<R>(&self, _py: &PyToken<'_>, access: impl FnOnce(&mut Store) -> R) -> R {
        crate::gil_assert();
        // SAFETY: the GIL serializes every access, and `access` never runs
        // Python or touches the table again.
        access(unsafe { &mut *self.0.get() })
    }
}

#[cfg(feature = "free-threaded")]
struct Table(std::sync::Mutex<Store>);

#[cfg(feature = "free-threaded")]
impl Table {
    fn new() -> Self {
        Self(std::sync::Mutex::new(Store::new()))
    }

    fn with<R>(&self, _py: &PyToken<'_>, access: impl FnOnce(&mut Store) -> R) -> R {
        access(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl ClassLookupCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Table::new(),
        }
    }

    pub(super) fn lookup(&self, py: &PyToken<'_>, key: LookupKey) -> Option<LookupValue> {
        self.entries.with(py, |store| store.get(key))
    }

    /// Caller verifies the original key's versions after resolution, including
    /// callbacks and lazy declaration publication. No callback runs under lock.
    pub(super) fn publish(&self, py: &PyToken<'_>, key: LookupKey, value: LookupValue) {
        inc_ref_bits(py, key.name);
        let (replaced, expired) = self
            .entries
            .with(py, |store| store.put(Entry { key, value }));
        for name in replaced.into_iter().chain(expired) {
            dec_ref_bits(py, name);
        }
    }

    pub(crate) fn clear(&self, py: &PyToken<'_>) -> bool {
        let entries: Vec<Entry> = self.entries.with(py, |store| store.take().collect());
        for entry in &entries {
            dec_ref_bits(py, entry.key.name);
        }
        !entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(class: u64, name: u64, epoch: u64) -> LookupKey {
        LookupKey {
            class,
            layout: 1,
            epoch,
            name,
        }
    }

    fn entry(key: LookupKey, value: u64, purity: LookupPurity) -> Entry {
        Entry {
            key,
            value: LookupValue {
                value: Some(value),
                name: None,
                purity,
            },
        }
    }

    #[test]
    fn a_callback_dependent_result_survives_a_colliding_lookup_until_the_epoch_moves() {
        let mut store = Store::new();
        let held = key(0x1000, 0x2000, 7);
        // Same slot: ((class >> 4) ^ (name >> 4)) % 256 matches `held`.
        let rival = key(
            0x1000 + (SLOTS as u64) * 16,
            0x2000 + (SLOTS as u64) * 16,
            7,
        );
        assert_eq!(held.slot(), rival.slot());

        assert_eq!(
            store.put(entry(held, 11, LookupPurity::CallbackDependent)),
            (None, vec![])
        );
        assert_eq!(
            store.put(entry(rival, 22, LookupPurity::CallbackFree)),
            (None, vec![])
        );
        assert_eq!(store.get(held).and_then(|v| v.value), Some(11));
        assert_eq!(store.get(rival).and_then(|v| v.value), Some(22));

        // A second callback-free result in the slot evicts only the first.
        let third = key(
            0x1000 + (SLOTS as u64) * 32,
            0x2000 + (SLOTS as u64) * 32,
            7,
        );
        assert_eq!(third.slot(), held.slot());
        assert_eq!(
            store.put(entry(third, 33, LookupPurity::CallbackFree)),
            (Some(rival.name), vec![])
        );
        assert_eq!(store.get(held).and_then(|v| v.value), Some(11));

        // A class change moves the epoch: the held key no longer matches, and
        // the next dependent publication drops the older epoch's record.
        let later = key(0x3000, 0x4000, 8);
        assert!(store.get(key(0x1000, 0x2000, 8)).is_none());
        assert_eq!(
            store.put(entry(later, 44, LookupPurity::CallbackDependent)),
            (None, vec![held.name])
        );
        assert!(store.get(held).is_none());
        assert_eq!(store.get(later).and_then(|v| v.value), Some(44));

        let names: Vec<u64> = store.take().map(|entry| entry.key.name).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&third.name) && names.contains(&later.name));
        assert!(store.get(later).is_none() && store.get(third).is_none());
    }
}
