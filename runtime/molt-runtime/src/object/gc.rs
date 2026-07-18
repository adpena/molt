//! Tier-2 cyclic garbage collector (CPython 3.12 `gc_collect_main` parity).
//!
//! molt reclaims the acyclic majority with precise reference counting (Tier 1,
//! the TIR drop-insertion pipeline). Pure RC cannot reclaim a self-sustaining
//! reference cycle: `a.peer = b; b.peer = a`, once both stack roots are dropped,
//! leaves each node pinned at refcount 1 by its peer. This module adds the
//! CPython-parity cycle collector that reclaims exactly those cycles.
//!
//! ## Algorithm — CPython's partition form (the proven dual of Bacon-Rajan
//! synchronous trial deletion; same garbage set, iterative, gc-module parity)
//!
//! `deduce_unreachable` over the tracked candidate set:
//!   1. `update_refs`: snapshot each tracked object's refcount into a transient
//!      `gc_refs` map; mark it COLLECTING.
//!   2. `subtract_refs`: for each tracked object, `traverse` its children and
//!      decrement `gc_refs` of every child that is itself in the
//!      candidate set. After this, `gc_refs > 0` ⟺ the object is
//!      referenced from OUTSIDE the candidate set (a root);
//!      `gc_refs == 0` ⟺ a cycle candidate.
//!   3. `move_unreachable`: BFS from the roots (`gc_refs > 0`). A root re-marks all
//!      its transitive referents reachable (`gc_refs := 1`). The
//!      objects still at `gc_refs == 0` after the BFS are the
//!      unreachable cycle garbage.
//!
//! Then the CPython 3.12 destruction order (verbatim — the most parity-sensitive
//! contract, do NOT reorder; verified against CPython 3.12 `Modules/gcmodule.c`
//! `gc_collect_main`):
//!   - move_legacy_finalizers / move_legacy_finalizer_reachable: NO-OP for molt.
//!     molt has no legacy `tp_del`; every finalizer is a PEP-442 `tp_finalize`-class
//!     `__del__`, so normal collection leaves `gc.garbage` empty (every
//!     `__del__`-bearing cycle is collectable). `DEBUG_SAVEALL` is the explicit
//!     retention mode. These two steps collapse but their POSITION (before
//!     weakrefs) is documented here so the surviving order matches CPython.
//!   - `handle_weakrefs`: a two-pass batched protocol over the WHOLE unreachable
//!     set — PASS 1 clears every weakref pointing into the set (so callbacks read
//!     None) and enqueues a callback only if the weakref object itself is NOT in the
//!     unreachable set (`gc_is_collecting`); PASS 2 invokes the enqueued callbacks.
//!     This is NOT the acyclic per-object `weakref_clear_for_ptr` (which clears and
//!     calls per target — wrong ordering for a cycle). Weakref clearing STRICTLY
//!     precedes finalizers.
//!   - `finalize_garbage`: run each object's `__del__` ONCE (set FINALIZER_RAN),
//!     in unreachable-list order.
//!   - `handle_resurrected_objects`: re-run `deduce_unreachable` over the
//!     post-finalization set; anything a `__del__` resurrected (re-rooted) leaves
//!     the collectable set. MANDATORY — omitting it is use-after-free on resurrected
//!     objects.
//!   - `delete_garbage`: `clear` (tp_clear) each still-unreachable object — drop its
//!     children's refs IN PLACE without freeing the container. The RC cascade then
//!     collapses the cycle through the normal `dec_ref` path.
//!
//! ## Data-structure adaptation to molt's NaN-boxed runtime
//!
//! molt has no intrusive `PyGC_Head` on the 24-byte header. The candidate set, the
//! `gc_refs` scratch, and the unreachable set have TRANSIENT logical contents under
//! the stop-the-world/GIL boundary. Their allocation-backed workspace is reused
//! across collections and explicitly released at runtime teardown; no object pointer
//! survives a lease. Per-object `gc_refs` lives in a `HashMap` keyed by the object's
//! EXPOSED-PROVENANCE address (Miri strict-provenance clean — never an auxiliary
//! sidecar address, which is metadata rather than object identity). The COLLECTING
//! bit has a dedicated assignment in the header `flags` registry.
//!
//! ## MayFormCycle (the GREEN bit)
//!
//! The acyclic majority pays ZERO. A type that cannot transitively hold a reference
//! cycle (int/float/bool/str/bytes/None and the runtime's leaf types) is GREEN: it is
//! never registered in the tracked set, never scanned, never `clear`ed. The
//! generated heap-kind authority tracks every cycle-capable owner; exact dicts and
//! tuples are dynamically projected with CPython-compatible timing.

#[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
use std::cell::Cell;
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Instant;

use crate::object::{
    HEADER_FLAG_FINALIZER_RAN, HEADER_FLAG_GC_COLLECTING, HEADER_FLAG_GC_PINNED,
    HEADER_FLAG_HAS_ABI_VIEW, PtrSlot, dec_ref_ptr, header_from_obj_ptr,
    object_class_has_finalizer, object_type_id,
};
use crate::{
    GC_REGISTRY_LOCK_CONTENTION_COUNT, GC_REGISTRY_LOCK_WAIT_NS, GC_SNAPSHOT_ALLOC_FAILURE_COUNT,
    GC_TRACK_COUNT, GC_TRACKED_HIGH_WATER, GC_TRACKED_LIVE, GC_UNTRACK_COUNT, MoltObject, PyToken,
    profile_enabled_unchecked, profile_hit_bytes_unchecked, profile_hit_unchecked,
};

pub(crate) const NUM_GENERATIONS: usize = 3;
pub(crate) const OLDEST_GENERATION: u8 = (NUM_GENERATIONS - 1) as u8;
pub(crate) const PERMANENT_GENERATION: u8 = NUM_GENERATIONS as u8;
const DEFAULT_THRESHOLDS: [i64; NUM_GENERATIONS] = [700, 10, 10];
const DEBUG_STATS: i64 = 1;
const DEBUG_COLLECTABLE: i64 = 2;
const DEBUG_SAVEALL: i64 = 32;

/// A control-plane word selected by the same concurrency policy as object
/// reference counts: a plain cell under the deterministic GIL (including
/// wasm32), and lock-free atomic state only for an explicitly free-threaded
/// native build. Automatic collection itself remains fail-closed in the latter
/// mode until the runtime owns a real stop-the-world epoch.
#[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
#[repr(transparent)]
struct GcWord(AtomicU64);

#[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
#[repr(transparent)]
struct GcWord(Cell<u64>);

impl GcWord {
    const fn new(value: u64) -> Self {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            Self(AtomicU64::new(value))
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            Self(Cell::new(value))
        }
    }

    #[inline(always)]
    fn load(&self, order: AtomicOrdering) -> u64 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.load(order)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            self.0.get()
        }
    }

    #[inline(always)]
    fn store(&self, value: u64, order: AtomicOrdering) {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        self.0.store(value, order);
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            self.0.set(value);
        }
    }

    #[inline(always)]
    fn swap(&self, value: u64, order: AtomicOrdering) -> u64 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.swap(value, order)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            self.0.replace(value)
        }
    }

    #[inline]
    fn fetch_update<F>(
        &self,
        set_order: AtomicOrdering,
        fetch_order: AtomicOrdering,
        update: F,
    ) -> Result<u64, u64>
    where
        F: FnMut(u64) -> Option<u64>,
    {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.fetch_update(set_order, fetch_order, update)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = (set_order, fetch_order);
            let observed = self.0.get();
            let mut update = update;
            let Some(next) = update(observed) else {
                return Err(observed);
            };
            self.0.set(next);
            Ok(observed)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GenerationStats {
    pub(crate) collections: u64,
    pub(crate) collected: u64,
    pub(crate) uncollectable: u64,
    pub(crate) scanned: u64,
}

struct GenerationStatsWords {
    collections: GcWord,
    collected: GcWord,
    uncollectable: GcWord,
    scanned: GcWord,
}

#[derive(Default)]
struct GcApiRoots {
    callbacks: u64,
    garbage: u64,
}

impl GenerationStatsWords {
    const fn new() -> Self {
        Self {
            collections: GcWord::new(0),
            collected: GcWord::new(0),
            uncollectable: GcWord::new(0),
            scanned: GcWord::new(0),
        }
    }

    fn snapshot(&self) -> GenerationStats {
        GenerationStats {
            collections: self.collections.load(AtomicOrdering::Relaxed),
            collected: self.collected.load(AtomicOrdering::Relaxed),
            uncollectable: self.uncollectable.load(AtomicOrdering::Relaxed),
            scanned: self.scanned.load(AtomicOrdering::Relaxed),
        }
    }

    fn reset(&self) {
        self.collections.store(0, AtomicOrdering::Relaxed);
        self.collected.store(0, AtomicOrdering::Relaxed);
        self.uncollectable.store(0, AtomicOrdering::Relaxed);
        self.scanned.store(0, AtomicOrdering::Relaxed);
    }
}

/// Per-runtime GC scheduling and statistics authority. The tracked registry owns
/// object membership; this state owns only scheduling policy and counters. It is
/// embedded in `RuntimeState`, so shutdown/re-init cannot inherit thresholds,
/// pending work, or statistics from a prior embedded interpreter.
pub(crate) struct GcRuntimeState {
    enabled: GcWord,
    pending: GcWord,
    debug_flags: GcWord,
    thresholds: [GcWord; NUM_GENERATIONS],
    counts: [GcWord; NUM_GENERATIONS],
    stats: [GenerationStatsWords; NUM_GENERATIONS],
    long_lived_total: GcWord,
    long_lived_pending: GcWord,
    api_roots: Mutex<GcApiRoots>,
}

// Cell-backed words are accessed only while `PyToken` proves the deterministic
// runtime GIL. The free-threaded representation is entirely atomic.
unsafe impl Sync for GcRuntimeState {}
// Isolate initialization catches setup panics only to discard the unpublished
// RuntimeState. No partially reset GC state can cross that publication boundary,
// so the cell-backed deterministic representation is unwind-safe in that scope.
impl std::panic::RefUnwindSafe for GcRuntimeState {}
impl std::panic::UnwindSafe for GcRuntimeState {}

impl GcRuntimeState {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: GcWord::new(1),
            pending: GcWord::new(0),
            debug_flags: GcWord::new(0),
            thresholds: [
                GcWord::new(DEFAULT_THRESHOLDS[0] as u64),
                GcWord::new(DEFAULT_THRESHOLDS[1] as u64),
                GcWord::new(DEFAULT_THRESHOLDS[2] as u64),
            ],
            counts: [GcWord::new(0), GcWord::new(0), GcWord::new(0)],
            stats: [
                GenerationStatsWords::new(),
                GenerationStatsWords::new(),
                GenerationStatsWords::new(),
            ],
            long_lived_total: GcWord::new(0),
            long_lived_pending: GcWord::new(0),
            api_roots: Mutex::new(GcApiRoots {
                callbacks: 0,
                garbage: 0,
            }),
        }
    }

    #[inline(always)]
    fn assert_custody() {
        #[cfg(not(feature = "free-threaded"))]
        crate::gil_assert();
    }

    pub(crate) fn enabled(&self) -> bool {
        Self::assert_custody();
        self.enabled.load(AtomicOrdering::Relaxed) != 0
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        Self::assert_custody();
        self.enabled
            .store(u64::from(enabled), AtomicOrdering::Relaxed);
        if !enabled {
            self.pending.store(0, AtomicOrdering::Relaxed);
        } else {
            self.schedule_if_due();
        }
    }

    pub(crate) fn thresholds(&self) -> [i64; NUM_GENERATIONS] {
        Self::assert_custody();
        std::array::from_fn(|index| self.thresholds[index].load(AtomicOrdering::Relaxed) as i64)
    }

    pub(crate) fn set_thresholds(&self, thresholds: [i64; NUM_GENERATIONS]) {
        Self::assert_custody();
        for (word, threshold) in self.thresholds.iter().zip(thresholds) {
            word.store(threshold as u64, AtomicOrdering::Relaxed);
        }
        self.pending.store(0, AtomicOrdering::Relaxed);
        self.schedule_if_due();
    }

    pub(crate) fn counts(&self) -> [i64; NUM_GENERATIONS] {
        Self::assert_custody();
        std::array::from_fn(|index| self.counts[index].load(AtomicOrdering::Relaxed) as i64)
    }

    pub(crate) fn debug_flags(&self) -> i64 {
        Self::assert_custody();
        self.debug_flags.load(AtomicOrdering::Relaxed) as i64
    }

    pub(crate) fn set_debug_flags(&self, flags: i64) {
        Self::assert_custody();
        self.debug_flags
            .store(flags as u64, AtomicOrdering::Relaxed);
    }

    pub(crate) fn generation_stats(&self) -> [GenerationStats; NUM_GENERATIONS] {
        Self::assert_custody();
        std::array::from_fn(|index| self.stats[index].snapshot())
    }

    pub(crate) fn on_allocation(&self) {
        Self::assert_custody();
        self.counts[0]
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
                count.checked_add(1)
            })
            .unwrap_or_else(|_| std::process::abort());
        self.schedule_if_due();
    }

    pub(crate) fn on_deallocation(&self) {
        Self::assert_custody();
        let _ = self.counts[0].fetch_update(
            AtomicOrdering::Relaxed,
            AtomicOrdering::Relaxed,
            |count| (count != 0).then(|| count - 1),
        );
    }

    #[inline]
    fn schedule_if_due(&self) {
        if cfg!(feature = "free-threaded") || !self.enabled() {
            return;
        }
        let threshold0 = self.thresholds[0].load(AtomicOrdering::Relaxed) as i64;
        let count0 = self.counts[0].load(AtomicOrdering::Relaxed) as i64;
        if threshold0 != 0 && count0 > threshold0 {
            self.pending.store(1, AtomicOrdering::Release);
        }
    }

    fn take_scheduled_generation(&self) -> Option<u8> {
        Self::assert_custody();
        if self.pending.swap(0, AtomicOrdering::AcqRel) == 0 || !self.enabled() {
            return None;
        }
        for generation in (0..NUM_GENERATIONS).rev() {
            let count = self.counts[generation].load(AtomicOrdering::Relaxed) as i64;
            let threshold = self.thresholds[generation].load(AtomicOrdering::Relaxed) as i64;
            if count <= threshold {
                continue;
            }
            if generation == NUM_GENERATIONS - 1 {
                let pending = self.long_lived_pending.load(AtomicOrdering::Relaxed);
                let total = self.long_lived_total.load(AtomicOrdering::Relaxed);
                if pending < total / 4 {
                    continue;
                }
            }
            return Some(generation as u8);
        }
        None
    }

    fn rearm_pending(&self) {
        Self::assert_custody();
        self.pending.store(1, AtomicOrdering::Release);
    }

    fn begin_collection(&self, generation: u8) {
        Self::assert_custody();
        let generation = generation as usize;
        if generation + 1 < NUM_GENERATIONS {
            self.counts[generation + 1]
                .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
                    count.checked_add(1)
                })
                .unwrap_or_else(|_| std::process::abort());
        }
        for count in &self.counts[..=generation] {
            count.store(0, AtomicOrdering::Relaxed);
        }
    }

    fn finish_collection(
        &self,
        generation: u8,
        scanned: usize,
        collected: usize,
        survivors: usize,
    ) {
        Self::assert_custody();
        let generation = generation as usize;
        for (word, delta) in [
            (&self.stats[generation].collections, 1u64),
            (&self.stats[generation].collected, collected as u64),
            (&self.stats[generation].scanned, scanned as u64),
        ] {
            word.fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |value| {
                value.checked_add(delta)
            })
            .unwrap_or_else(|_| std::process::abort());
        }
        if generation == NUM_GENERATIONS - 2 {
            self.long_lived_pending
                .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |value| {
                    value.checked_add(survivors as u64)
                })
                .unwrap_or_else(|_| std::process::abort());
        } else if generation == NUM_GENERATIONS - 1 {
            self.long_lived_pending.store(0, AtomicOrdering::Relaxed);
            self.long_lived_total
                .store(survivors as u64, AtomicOrdering::Relaxed);
        }
        self.schedule_if_due();
    }

    pub(crate) fn reset(&self) {
        Self::assert_custody();
        self.enabled.store(1, AtomicOrdering::Relaxed);
        self.pending.store(0, AtomicOrdering::Relaxed);
        self.debug_flags.store(0, AtomicOrdering::Relaxed);
        for (index, threshold) in DEFAULT_THRESHOLDS.into_iter().enumerate() {
            self.thresholds[index].store(threshold as u64, AtomicOrdering::Relaxed);
            self.counts[index].store(0, AtomicOrdering::Relaxed);
            self.stats[index].reset();
        }
        self.long_lived_total.store(0, AtomicOrdering::Relaxed);
        self.long_lived_pending.store(0, AtomicOrdering::Relaxed);
        let roots = self
            .api_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(roots.callbacks, 0, "GC callback root survived teardown");
        assert_eq!(roots.garbage, 0, "GC garbage root survived teardown");
    }

    fn api_root_bits(&self, py: &PyToken<'_>, callbacks: bool) -> u64 {
        Self::assert_custody();
        let read = |roots: &GcApiRoots| {
            if callbacks {
                roots.callbacks
            } else {
                roots.garbage
            }
        };
        {
            let roots = self
                .api_roots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let bits = read(&roots);
            if bits != 0 {
                crate::inc_ref_bits(py, bits);
                return bits;
            }
        }

        // Never allocate while holding the root mutex: allocation may reach an
        // automatic-GC safepoint and recursively ask for the callback list.
        let ptr = crate::alloc_list(py, &[]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let created = MoltObject::from_ptr(ptr).bits();
        let mut roots = self
            .api_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = if callbacks {
            &mut roots.callbacks
        } else {
            &mut roots.garbage
        };
        if *slot == 0 {
            *slot = created;
            crate::inc_ref_bits(py, created);
            created
        } else {
            let existing = *slot;
            crate::inc_ref_bits(py, existing);
            drop(roots);
            crate::dec_ref_bits(py, created);
            existing
        }
    }

    pub(crate) fn callbacks_bits(&self, py: &PyToken<'_>) -> u64 {
        self.api_root_bits(py, true)
    }

    pub(crate) fn garbage_bits(&self, py: &PyToken<'_>) -> u64 {
        self.api_root_bits(py, false)
    }

    fn existing_api_root_bits(&self, py: &PyToken<'_>, callbacks: bool) -> u64 {
        Self::assert_custody();
        let roots = self
            .api_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bits = if callbacks {
            roots.callbacks
        } else {
            roots.garbage
        };
        if bits != 0 {
            crate::inc_ref_bits(py, bits);
        }
        bits
    }

    pub(crate) fn clear_api_roots(&self, py: &PyToken<'_>) {
        Self::assert_custody();
        let (callbacks, garbage) = {
            let mut roots = self
                .api_roots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let values = (roots.callbacks, roots.garbage);
            roots.callbacks = 0;
            roots.garbage = 0;
            values
        };
        for bits in [callbacks, garbage] {
            if bits != 0 {
                crate::dec_ref_bits(py, bits);
            }
        }
    }
}

pub(crate) fn gc_clear_api_roots(py: &PyToken<'_>) {
    crate::runtime_state(py).gc.clear_api_roots(py);
}

/// Side registry of live cycle-capable objects (CPython's three gc-tracked
/// generations). Each pointer receives a
/// monotonic allocation ordinal, and collection snapshots sort by that ordinal;
/// allocator addresses and randomized hash iteration therefore cannot change
/// finalizer/clear order across identical runs. Populated at allocation of a
/// non-GREEN object and removed at free. GREEN/atomic objects are never inserted.
///
/// This is its OWN structure, not the provenance pointer registry — the latter is
/// populated only in debug builds (`from_ptr` skips `register_ptr` in release), so
/// it cannot enumerate live objects in the shipped profile.
struct TrackedRegistryShard {
    entries: HashMap<PtrSlot, TrackedEntry>,
}

#[derive(Clone, Copy)]
struct TrackedEntry {
    allocation_id: u64,
    generation: u8,
}

const TRACKED_REGISTRY_SHARDS: usize = 64;
const _: () = assert!(TRACKED_REGISTRY_SHARDS.is_power_of_two());

#[cfg(test)]
static GC_REGISTRY_ACCESS_COUNT: AtomicU64 = AtomicU64::new(0);

struct TrackedRegistry {
    shards: [Mutex<TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS],
    next_allocation_id: AtomicU64,
    owner_runtime: AtomicUsize,
}

fn tracked_registry() -> &'static TrackedRegistry {
    static REGISTRY: OnceLock<TrackedRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| TrackedRegistry {
        shards: std::array::from_fn(|_| {
            Mutex::new(TrackedRegistryShard {
                entries: HashMap::new(),
            })
        }),
        next_allocation_id: AtomicU64::new(1),
        owner_runtime: AtomicUsize::new(0),
    })
}

#[inline]
fn claim_registry_owner(owner: &AtomicUsize, identity: usize) -> Result<(), usize> {
    debug_assert_ne!(identity, 0);
    match owner.compare_exchange(0, identity, AtomicOrdering::AcqRel, AtomicOrdering::Acquire) {
        Ok(_) => Ok(()),
        Err(existing) if existing == identity => Ok(()),
        Err(existing) => Err(existing),
    }
}

#[inline]
fn release_registry_owner(owner: &AtomicUsize, identity: usize) -> Result<(), usize> {
    owner
        .compare_exchange(identity, 0, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .map(|_| ())
}

/// Bind the process-global membership storage to one concrete `RuntimeState`.
///
/// Molt's lifecycle publishes at most one process runtime at a time. Keeping the
/// registry allocation process-global lets a sequential embedded re-init reuse
/// the authority without a second pointer map, but membership must never cross
/// runtime identities. A competing embedded runtime therefore fails closed at
/// initialization instead of silently collecting objects owned by another heap.
pub(crate) fn gc_bind_registry(state: &crate::RuntimeState) {
    let identity = std::ptr::from_ref(state).expose_provenance();
    if let Err(existing) = claim_registry_owner(&tracked_registry().owner_runtime, identity) {
        panic!(
            "GC registry already belongs to runtime 0x{existing:x}; competing runtime 0x{identity:x} cannot share process-global membership"
        );
    }
}

#[cfg(test)]
pub(crate) fn gc_registry_owner_identity() -> usize {
    tracked_registry()
        .owner_runtime
        .load(AtomicOrdering::Acquire)
}

#[inline]
fn tracked_registry_shard_index(ptr: *mut u8) -> usize {
    tracked_registry_shard_index_from_address(ptr.expose_provenance())
}

#[inline]
fn tracked_registry_shard_index_from_address(address: usize) -> usize {
    // Heap pointers are aligned, so their low bits carry no entropy. Drop those
    // bits, then use the SplitMix64 finalizer to spread both compact arenas and
    // discontiguous system allocations across every shard. The explicit u64
    // lane is portable to wasm32 (where shifting a usize by 33 does not compile).
    let mut mixed = (address as u64) >> 3;
    mixed ^= mixed >> 30;
    mixed = mixed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed ^= mixed >> 27;
    mixed = mixed.wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^= mixed >> 31;
    (mixed as usize) & (TRACKED_REGISTRY_SHARDS - 1)
}

fn lock_tracked_registry_shard(index: usize) -> MutexGuard<'static, TrackedRegistryShard> {
    #[cfg(test)]
    GC_REGISTRY_ACCESS_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
    let shard = &tracked_registry().shards[index];
    if !profile_enabled_unchecked() {
        return shard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    match shard.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => {
            profile_hit_unchecked(&GC_REGISTRY_LOCK_CONTENTION_COUNT);
            let started = Instant::now();
            let guard = shard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let wait_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            profile_hit_bytes_unchecked(&GC_REGISTRY_LOCK_WAIT_NS, wait_ns);
            guard
        }
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
    }
}

#[inline]
fn profile_gc_track() {
    if profile_enabled_unchecked() {
        GC_TRACK_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
        let live = GC_TRACKED_LIVE.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        GC_TRACKED_HIGH_WATER.fetch_max(live, AtomicOrdering::Relaxed);
    }
}

#[inline]
fn profile_gc_untrack(count: u64) {
    if profile_enabled_unchecked() {
        GC_UNTRACK_COUNT.fetch_add(count, AtomicOrdering::Relaxed);
        let _ = GC_TRACKED_LIVE.fetch_update(
            AtomicOrdering::Relaxed,
            AtomicOrdering::Relaxed,
            |live| Some(live.saturating_sub(count)),
        );
    }
}

/// `MOLT_TRACE_GC=1` enables collector tracing (candidate/unreachable/collected
/// counts) to stderr. Diagnostic-only; never part of observable program behavior.
fn gc_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_GC").as_deref() == Ok("1"))
}

/// MayFormCycle: `true` when an object of this `type_id` can transitively hold a
/// reference cycle and must therefore be tracked by the collector. The complement
/// (GREEN) is sound-conservative: a GREEN object provably cannot be part of a cycle,
/// so it pays zero collector cost.
///
/// The generated heap-kind table classifies the complete ref-holding family. Fixed
/// tracked kinds enter directly; exact dicts and tuples use CPython-compatible
/// dynamic projection. The lifecycle handler owns exhaustive visit and clear
/// dispatch, so adding a kind without both operations fails structural generation
/// tests instead of silently creating a leak lane.
#[inline]
pub(crate) fn may_form_cycle(type_id: u32) -> bool {
    !matches!(
        super::heap_track_projection(type_id),
        None | Some(super::HeapTrackProjection::Never)
    )
}

/// Re-evaluate an exact dict after its new contents have been fully published.
/// Every dict mutation family calls this once per completed transaction. Tuples
/// deliberately do not use this path: non-empty tuples start tracked and are
/// only reprojected by the collector, matching CPython.
pub(crate) unsafe fn gc_reproject_dict(py: &PyToken<'_>, ptr: *mut u8) {
    if !unsafe { (*header_from_obj_ptr(ptr)).gc_is_published() } {
        return;
    }
    if super::heap_track_projection(unsafe { object_type_id(ptr) })
        != Some(super::HeapTrackProjection::DictDynamic)
    {
        return;
    }
    let should_track = unsafe { super::heap_lifecycle::projected_track_state(py, ptr) };
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let slot = PtrSlot(ptr);
    if should_track {
        if let Entry::Vacant(entry) = shard.entries.entry(slot) {
            let allocation_id = tracked_registry()
                .next_allocation_id
                .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |next| {
                    next.checked_add(1)
                })
                .expect("GC allocation ordinal exhausted");
            entry.insert(TrackedEntry {
                allocation_id,
                generation: 0,
            });
            profile_gc_track();
        }
    } else if shard.entries.remove(&slot).is_some() {
        profile_gc_untrack(1);
    }
}

#[derive(Clone, Copy)]
struct GcCandidate {
    allocation_id: u64,
    ptr: PtrSlot,
}

unsafe fn reproject_reachable_immutable_tuples(
    py: &PyToken<'_>,
    candidates: &[GcCandidate],
    marks: &[u8],
) {
    for (candidate_index, candidate) in candidates.iter().enumerate() {
        // CPython runs deduce_unreachable first and untracks atomic tuples only
        // from the reachable generation list. An unreachable tuple remains a
        // candidate for this collection and contributes to gc.collect()'s count.
        if marks[candidate_index] != 2 {
            continue;
        }
        let ptr = candidate.ptr.0;
        if super::heap_track_projection(unsafe { object_type_id(ptr) })
            != Some(super::HeapTrackProjection::TupleDynamic)
            || unsafe { super::heap_lifecycle::projected_track_state(py, ptr) }
        {
            continue;
        }
        unsafe {
            gc_untrack(
                py,
                ptr,
                super::TYPE_ID_TUPLE,
                GcUntrackReason::DynamicProjection,
            )
        };
    }
}

/// Register a freshly-allocated object in the tracked set IFF it can form a cycle.
/// Called from the allocator for every heap object; GREEN types return immediately.
///
/// # Safety
/// `ptr` must be a live object pointer (data pointer, past the header).
#[inline]
pub(crate) unsafe fn gc_track_if_cyclic(py: &PyToken<'_>, ptr: *mut u8, type_id: u32) {
    if !may_form_cycle(type_id) {
        return;
    }
    let registry = tracked_registry();
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let slot = PtrSlot(ptr);
    if let Entry::Vacant(entry) = shard.entries.entry(slot) {
        crate::runtime_state(py).gc.on_allocation();
        let allocation_id = registry
            .next_allocation_id
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |next| {
                next.checked_add(1)
            })
            .expect("GC allocation ordinal exhausted");
        entry.insert(TrackedEntry {
            allocation_id,
            generation: 0,
        });
        profile_gc_track();
    }
}

/// Release-publish a completely initialized heap payload to the collector.
/// Constructors call this exactly once after every payload/class/sidecar edge is
/// valid. Registry membership may precede publication, but snapshots never expose
/// an unpublished entry.
#[inline]
pub(crate) unsafe fn gc_publish_initialized(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe { (*header_from_obj_ptr(ptr)).gc_publish_initialized() };
    if super::heap_track_projection(unsafe { object_type_id(ptr) })
        == Some(super::HeapTrackProjection::DictDynamic)
    {
        unsafe { gc_reproject_dict(py, ptr) };
    }
}

/// Remove an object from the tracked set as it is freed. Called from the
/// deallocator for every freed object; a no-op (cheap set miss) for GREEN types and
/// untracked objects.
///
/// # Safety
/// `ptr` identifies the object being freed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GcUntrackReason {
    Deallocation,
    DynamicProjection,
}

pub(crate) unsafe fn gc_untrack(
    py: &PyToken<'_>,
    ptr: *mut u8,
    type_id: u32,
    reason: GcUntrackReason,
) {
    if !may_form_cycle(type_id) {
        return;
    }
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    if shard.entries.remove(&PtrSlot(ptr)).is_some() {
        profile_gc_untrack(1);
    }
    drop(shard);
    // CPython's generation-0 counter is allocations minus deallocations of GC
    // objects, not current tracked-set membership. Dynamically projected exact
    // dicts/tuples still retire their original allocation here.
    if reason == GcUntrackReason::Deallocation {
        crate::runtime_state(py).gc.on_deallocation();
    }
}

/// Is this object currently in the tracked set? Backs `gc.is_tracked`.
///
/// # Safety
/// `ptr` is treated as an opaque key; not dereferenced.
pub(crate) unsafe fn gc_is_tracked(ptr: *mut u8) -> bool {
    lock_tracked_registry_shard(tracked_registry_shard_index(ptr))
        .entries
        .contains_key(&PtrSlot(ptr))
}

/// Drop the entire tracked set without touching the objects. Used at runtime
/// teardown AFTER the heap has been reclaimed, so the static does not dangle into
/// the next embedded runtime instance.
pub(crate) fn gc_reset_registry(state: &crate::RuntimeState) {
    let registry = tracked_registry();
    let identity = std::ptr::from_ref(state).expose_provenance();
    assert_eq!(
        registry.owner_runtime.load(AtomicOrdering::Acquire),
        identity,
        "GC registry teardown attempted by a non-owner runtime"
    );
    // Hold every shard until the ordinal is reset. This makes teardown a single
    // registry transaction and prevents a future free-threaded allocator from
    // inserting between a partial clear and the return to ordinal 1.
    let mut shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    let mut removed = 0u64;
    for shard in &mut shards {
        removed = removed.saturating_add(shard.entries.len() as u64);
        // Full embedded-runtime teardown must release peak registry capacity;
        // `HashMap::clear` would pin the largest prior heap in this OnceLock.
        shard.entries = HashMap::new();
    }
    registry
        .next_allocation_id
        .store(1, AtomicOrdering::Relaxed);
    profile_gc_untrack(removed);
    release_registry_owner(&registry.owner_runtime, identity)
        .expect("GC registry owner changed during teardown transaction");
}

#[inline]
fn try_reserve_total<T>(values: &mut Vec<T>, required: usize) -> bool {
    values.capacity() >= required
        || values
            .try_reserve_exact(required.saturating_sub(values.len()))
            .is_ok()
}

#[derive(Clone, Copy)]
enum RegistrySelection {
    Through(u8),
    Exact(u8),
    Ordinary,
    All,
}

impl RegistrySelection {
    #[inline]
    fn contains(self, generation: u8) -> bool {
        match self {
            Self::Through(maximum) => generation <= maximum,
            Self::Exact(expected) => generation == expected,
            Self::Ordinary => generation < PERMANENT_GENERATION,
            Self::All => true,
        }
    }
}

fn snapshot_registry(candidates: &mut Vec<GcCandidate>, selection: RegistrySelection) -> bool {
    // Freeze the entire registry while taking the snapshot. Object graph
    // traversal still requires the runtime's stop-the-world/GIL collection
    // boundary, but tracking metadata itself is now coherent under a future
    // free-threaded allocator rather than a per-shard temporal patchwork.
    let shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    candidates.clear();
    let entry_count = shards
        .iter()
        .map(|shard| {
            shard
                .entries
                .values()
                .filter(|entry| selection.contains(entry.generation))
                .count()
        })
        .sum::<usize>();
    if !try_reserve_total(candidates, entry_count) {
        profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
        return false;
    }
    for shard in &shards {
        candidates.extend(shard.entries.iter().filter_map(|(slot, entry)| {
            if !selection.contains(entry.generation) {
                return None;
            }
            // Acquire pairs with constructor publication. A concurrent
            // collector may observe registry insertion first, but never
            // traverses a partially initialized payload.
            unsafe { (*header_from_obj_ptr(slot.0)).gc_is_published() }.then_some(GcCandidate {
                allocation_id: entry.allocation_id,
                ptr: *slot,
            })
        }));
    }
    drop(shards);
    candidates.sort_unstable_by_key(|candidate| candidate.allocation_id);
    true
}

fn snapshot_tracked_registry(candidates: &mut Vec<GcCandidate>, generation: u8) -> bool {
    snapshot_registry(candidates, RegistrySelection::Through(generation))
}

/// Move every ordinary tracked object to CPython's permanent generation.
/// Allocation order and membership remain unchanged; normal collection
/// snapshots exclude these entries until `unfreeze()` restores generation 2.
pub(crate) fn freeze_tracked_registry() {
    let mut shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    for shard in &mut shards {
        for entry in shard.entries.values_mut() {
            if entry.generation < PERMANENT_GENERATION {
                entry.generation = PERMANENT_GENERATION;
            }
        }
    }
}

pub(crate) fn unfreeze_tracked_registry() {
    let mut shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    for shard in &mut shards {
        for entry in shard.entries.values_mut() {
            if entry.generation == PERMANENT_GENERATION {
                entry.generation = OLDEST_GENERATION;
            }
        }
    }
}

pub(crate) fn permanent_generation_count() -> usize {
    let shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    shards
        .iter()
        .map(|shard| {
            shard
                .entries
                .values()
                .filter(|entry| entry.generation == PERMANENT_GENERATION)
                .count()
        })
        .sum()
}

/// Promote one deterministic candidate partition after a collection. Holding all
/// shards turns promotion into one metadata transaction and preserves the
/// snapshot's allocation-ordinal order independently from hash iteration.
fn promote_marked_candidates(
    candidates: &[GcCandidate],
    marks: &[u8],
    selected_mark: u8,
    target_generation: u8,
) -> usize {
    let mut shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    let mut promoted = 0usize;
    for (index, candidate) in candidates.iter().enumerate() {
        if marks[index] != selected_mark {
            continue;
        }
        let shard_index = tracked_registry_shard_index(candidate.ptr.0);
        let Some(entry) = shards[shard_index].entries.get_mut(&candidate.ptr) else {
            continue;
        };
        entry.generation = target_generation;
        promoted += 1;
    }
    promoted
}

// ---------------------------------------------------------------------------
// molt_traverse / molt_clear — the single child-enumeration authority
// ---------------------------------------------------------------------------

/// Visit every heap-pointer CHILD of `ptr` (a tracked container), passing each
/// child's RAW OBJECT POINTER to `visit`. This is molt's `tp_traverse`: the single
/// source of truth for "what does this object reference". It enumerates EXACTLY the
/// children that the deallocator's `dec_ref` cascade releases — the collector must
/// see the same edges the deallocator frees, or it would leak (missed edge) or
/// double-free (cleared an edge the dealloc also frees). Generated exhaustive
/// dispatch plus lifecycle edge-equivalence tests pin this contract.
///
/// Primitive children (int/float/bool/None/str/bytes — anything that is not a heap
/// pointer, or a GREEN leaf) are skipped: only TAG_PTR values reach `visit`.
///
/// # Safety
/// `ptr` must be a live object of a `may_form_cycle` type. The GIL is held (the
/// `TYPE_ID_OBJECT` arm reads class metadata through the shared inline-field walker).
pub(crate) unsafe fn molt_traverse(py: &PyToken<'_>, ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    unsafe { super::heap_lifecycle::visit_owned_edges(py, ptr, visit) }
}

/// molt's `tp_clear`: drop every heap-pointer child reference IN PLACE, emptying the
/// container's backing store WITHOUT freeing the container itself. Called by the
/// collector's `delete_garbage` on each unreachable cycle member; the resulting
/// `dec_ref` cascade collapses the cycle through the normal RC path. The container's
/// own memory is freed by that cascade (when its refcount, now no longer pinned by a
/// cleared peer, reaches zero) — `clear` must NOT free it directly (freeing while
/// other members still reference it would double-free).
///
/// # Safety
/// `ptr` must be a live object of a `may_form_cycle` type.
#[cfg(test)]
pub(crate) unsafe fn molt_clear(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe { super::heap_lifecycle::clear_cycle_edges(py, ptr) }
}

// ---------------------------------------------------------------------------
// The collector — deduce_unreachable + CPython 6-step destruction
// ---------------------------------------------------------------------------

/// Result of one `collect_cycles` invocation: the number of objects reclaimed (the
/// `m` that `gc.collect()` returns; `n` = uncollectable = 0 for molt since
/// `gc.garbage` is always empty under PEP 442).
pub(crate) struct CollectStats {
    pub(crate) collected: usize,
    #[cfg(test)]
    pub(crate) scanned: usize,
    #[cfg(test)]
    pub(crate) survivors: usize,
    pub(crate) status: GcCollectStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GcCollectStatus {
    Completed,
    ReentrantNoop,
    ResourceError(&'static str),
    UnsupportedConcurrency,
}

impl CollectStats {
    fn completed(collected: usize, scanned: usize, survivors: usize) -> Self {
        #[cfg(not(test))]
        let _ = (scanned, survivors);
        Self {
            collected,
            #[cfg(test)]
            scanned,
            #[cfg(test)]
            survivors,
            status: GcCollectStatus::Completed,
        }
    }

    fn failure(py: &PyToken<'_>, status: GcCollectStatus) -> Self {
        let code = match status {
            GcCollectStatus::Completed => 0,
            GcCollectStatus::ReentrantNoop => 1,
            GcCollectStatus::ResourceError(_) => 2,
            GcCollectStatus::UnsupportedConcurrency => 3,
        };
        crate::runtime_state(py)
            .gc_last_failure
            .store(code, AtomicOrdering::Release);
        Self {
            collected: 0,
            #[cfg(test)]
            scanned: 0,
            #[cfg(test)]
            survivors: 0,
            status,
        }
    }
}

#[inline]
unsafe fn header_refcount(ptr: *mut u8) -> u32 {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).ref_count_snapshot()
    }
}

#[inline]
unsafe fn effective_gc_refcount(ptr: *mut u8) -> i64 {
    // Use a target-independent signed lane. On wasm32, `u32 as isize` turns
    // every count above i32::MAX negative and could classify a live object as
    // unreachable. The runtime permits every non-immortal u32 count, so GC
    // scratch must represent that complete domain on every architecture.
    let mut raw = i64::from(unsafe { header_refcount(ptr) });
    let header = unsafe { header_from_obj_ptr(ptr) };
    if unsafe { (*header).has_flag(HEADER_FLAG_GC_PINNED) } {
        raw -= 1;
    }
    if !unsafe { (*header).has_flag(HEADER_FLAG_HAS_ABI_VIEW) } {
        return raw;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let adjusted = raw
        + i64::try_from(molt_cpython_abi::bridge::GLOBAL_BRIDGE.gc_ref_adjustment(bits))
            .unwrap_or_else(|_| std::process::abort());
    if molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(bits) {
        adjusted.max(1)
    } else {
        adjusted
    }
}

unsafe fn pin_unreachable(ptrs: &[PtrSlot]) {
    for &PtrSlot(ptr) in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).pin_for_gc() };
    }
}

unsafe fn release_unreachable_pins(py: &PyToken<'_>, ptrs: &[PtrSlot]) {
    for &PtrSlot(ptr) in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED) };
        unsafe { dec_ref_ptr(py, ptr) };
    }
}

unsafe fn release_index_pins(py: &PyToken<'_>, candidates: &[GcCandidate], indices: &[usize]) {
    for &index in indices {
        let ptr = candidates[index].ptr.0;
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED) };
        unsafe { dec_ref_ptr(py, ptr) };
    }
}

unsafe fn detach_requirements(
    py: &PyToken<'_>,
    candidates: &[GcCandidate],
    indices: &[usize],
) -> (usize, usize) {
    let mut edges = 0usize;
    let mut resources = 0usize;
    for &index in indices {
        let ptr = candidates[index].ptr.0;
        unsafe {
            molt_traverse(py, ptr, &mut |_| {
                edges = edges
                    .checked_add(1)
                    .unwrap_or_else(|| std::process::abort())
            });
            resources = resources
                .checked_add(super::heap_lifecycle::detached_resource_count(ptr))
                .unwrap_or_else(|| std::process::abort());
        }
    }
    (edges, resources)
}

#[inline]
unsafe fn header_set_collecting(ptr: *mut u8, on: bool) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        if on {
            (*header).fetch_or_flags(HEADER_FLAG_GC_COLLECTING);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_GC_COLLECTING);
        }
    }
}

#[inline]
unsafe fn header_is_collecting(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).has_flag(HEADER_FLAG_GC_COLLECTING)
    }
}

#[inline]
fn addr_key(ptr: *mut u8) -> u64 {
    ptr.expose_provenance() as u64
}

/// `deduce_unreachable` (CPython): partition `candidates` into reachable (re-rooted)
/// and unreachable (cycle garbage). Returns the unreachable pointers in deterministic
/// order. Sets/clears the COLLECTING flag on candidates as part of the partition; on
/// return, ONLY the returned unreachable objects still carry COLLECTING (so the
/// weakref pass can ask `gc_is_collecting` of any object). Reachable objects have
/// COLLECTING cleared.
///
/// # Safety
/// `candidates` are live tracked objects; the GIL is held.
#[derive(Default)]
struct GcScratch {
    candidates: Vec<GcCandidate>,
    index: HashMap<u64, usize>,
    refs: Vec<i64>,
    marks: Vec<u8>,
    queue: Vec<usize>,
    first_unreachable: Vec<usize>,
    first_unreachable_ptrs: Vec<PtrSlot>,
    final_unreachable: Vec<usize>,
    api_values: Vec<u64>,
    api_targets: Vec<u64>,
    api_target_membership: HashSet<u64>,
}

impl GcScratch {
    fn acquire() -> GcScratchLease {
        let scratch = gc_scratch_pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap_or_default();
        GcScratchLease {
            scratch: Some(scratch),
        }
    }

    fn try_prepare_candidates(&mut self) -> bool {
        let len = self.candidates.len();
        self.index.clear();
        self.refs.clear();
        self.marks.clear();
        self.queue.clear();
        self.first_unreachable.clear();
        self.first_unreachable_ptrs.clear();
        self.final_unreachable.clear();
        self.api_values.clear();
        self.api_targets.clear();
        self.api_target_membership.clear();
        if self.index.try_reserve(len).is_err()
            || !try_reserve_total(&mut self.refs, len)
            || !try_reserve_total(&mut self.marks, len)
            || !try_reserve_total(&mut self.queue, len)
            || !try_reserve_total(&mut self.first_unreachable, len)
            || !try_reserve_total(&mut self.first_unreachable_ptrs, len)
            || !try_reserve_total(&mut self.final_unreachable, len)
        {
            return false;
        }
        for (candidate_index, candidate) in self.candidates.iter().enumerate() {
            self.index
                .insert(addr_key(candidate.ptr.0), candidate_index);
        }
        self.refs.resize(len, 0);
        self.marks.resize(len, 0);
        true
    }

    fn clear_for_reuse(&mut self) {
        self.candidates.clear();
        self.index.clear();
        self.refs.clear();
        self.marks.clear();
        self.queue.clear();
        self.first_unreachable.clear();
        self.first_unreachable_ptrs.clear();
        self.final_unreachable.clear();
        self.api_values.clear();
        self.api_targets.clear();
        self.api_target_membership.clear();
    }
}

/// One collection may run per runtime at a time, while collection callbacks may
/// re-enter read-only GC introspection and therefore lease a second workspace.
/// `PtrSlot` is the runtime's canonical cross-thread opaque-pointer carrier, so
/// cached capacity needs no alternate raw-pointer `Send` promise. Runtime
/// teardown drops every learned high-water buffer explicitly.
fn gc_scratch_pool() -> &'static Mutex<Vec<GcScratch>> {
    static POOL: OnceLock<Mutex<Vec<GcScratch>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

struct GcScratchLease {
    scratch: Option<GcScratch>,
}

impl std::ops::Deref for GcScratchLease {
    type Target = GcScratch;

    fn deref(&self) -> &Self::Target {
        self.scratch.as_ref().expect("live GC workspace lease")
    }
}

impl std::ops::DerefMut for GcScratchLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.scratch.as_mut().expect("live GC workspace lease")
    }
}

impl Drop for GcScratchLease {
    fn drop(&mut self) {
        let mut scratch = self.scratch.take().expect("live GC workspace lease");
        scratch.clear_for_reuse();
        let mut pool = gc_scratch_pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pool.push(scratch);
    }
}

/// Release the current runtime thread's cached collector high-water capacity.
/// Called only after heap and registry teardown, so no live pointer can remain.
pub(crate) fn gc_reset_workspace() {
    gc_scratch_pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn snapshot_api_values(
    scratch: &mut GcScratch,
    selection: RegistrySelection,
) -> Result<(), &'static str> {
    if !snapshot_registry(&mut scratch.candidates, selection) {
        return Err("tracked-registry snapshot allocation failed");
    }
    scratch.api_values.clear();
    if !try_reserve_total(&mut scratch.api_values, scratch.candidates.len()) {
        return Err("GC introspection result allocation failed");
    }
    Ok(())
}

/// Build `gc.get_objects()` from the same deterministic registry snapshot used
/// by collection. `None` excludes the permanent generation; `Some(g)` selects
/// exactly one ordinary generation, matching CPython's public contract.
pub(crate) fn get_objects(
    py: &PyToken<'_>,
    generation: Option<u8>,
) -> Result<*mut u8, &'static str> {
    let selection = generation.map_or(RegistrySelection::Ordinary, RegistrySelection::Exact);
    let mut scratch = GcScratch::acquire();
    snapshot_api_values(&mut scratch, selection)?;
    let GcScratch {
        candidates,
        api_values,
        ..
    } = &mut *scratch;
    for candidate in candidates.iter() {
        api_values.push(MoltObject::from_ptr(candidate.ptr.0).bits());
    }
    let result = crate::alloc_list(py, &scratch.api_values);
    (!result.is_null())
        .then_some(result)
        .ok_or("GC introspection result allocation failed")
}

/// Build `gc.get_referents(*objects)` from the exhaustive lifecycle value
/// authority. Unlike cycle traversal, this intentionally retains inline values.
pub(crate) unsafe fn get_referents(
    py: &PyToken<'_>,
    objects_ptr: *mut u8,
) -> Result<*mut u8, &'static str> {
    let mut scratch = GcScratch::acquire();
    scratch.api_values.clear();
    let mut required = 0usize;
    unsafe {
        super::seq_access::with_borrowed(objects_ptr, |objects| {
            for &bits in objects {
                if let Some(ptr) = crate::obj_from_bits(bits).as_ptr() {
                    super::heap_lifecycle::visit_owned_values(py, ptr, &mut |_| required += 1);
                }
            }
        });
    }
    if !try_reserve_total(&mut scratch.api_values, required) {
        return Err("GC introspection result allocation failed");
    }
    unsafe {
        super::seq_access::with_borrowed(objects_ptr, |objects| {
            for &bits in objects {
                if let Some(ptr) = crate::obj_from_bits(bits).as_ptr() {
                    super::heap_lifecycle::visit_owned_values(py, ptr, &mut |child| {
                        scratch.api_values.push(child);
                    });
                }
            }
        });
    }
    let result = crate::alloc_list(py, &scratch.api_values);
    (!result.is_null())
        .then_some(result)
        .ok_or("GC introspection result allocation failed")
}

/// Build `gc.get_referrers(*objects)` by scanning every tracked generation,
/// including the frozen permanent generation, through the same lifecycle value
/// authority. Each referring container appears once regardless of edge count.
pub(crate) unsafe fn get_referrers(
    py: &PyToken<'_>,
    objects_ptr: *mut u8,
) -> Result<*mut u8, &'static str> {
    let mut scratch = GcScratch::acquire();
    snapshot_api_values(&mut scratch, RegistrySelection::All)?;
    scratch.api_target_membership.clear();
    let target_count = unsafe { super::seq_access::with_borrowed(objects_ptr, <[u64]>::len) };
    if scratch.api_target_membership.capacity() < target_count
        && scratch
            .api_target_membership
            .try_reserve(target_count)
            .is_err()
    {
        return Err("GC introspection target allocation failed");
    }
    unsafe {
        super::seq_access::with_borrowed(objects_ptr, |values| {
            scratch.api_target_membership.extend(values.iter().copied())
        });
    }
    if scratch.api_target_membership.is_empty() {
        return Ok(crate::alloc_list(py, &[]));
    }
    let args_identity = objects_ptr.expose_provenance();
    let GcScratch {
        candidates,
        api_values,
        api_target_membership,
        ..
    } = &mut *scratch;
    for candidate in candidates.iter() {
        if candidate.ptr.0.expose_provenance() == args_identity {
            continue;
        }
        let mut refers = false;
        unsafe {
            super::heap_lifecycle::visit_owned_values(py, candidate.ptr.0, &mut |child| {
                refers |= api_target_membership.contains(&child);
            });
        }
        if refers {
            api_values.push(MoltObject::from_ptr(candidate.ptr.0).bits());
        }
    }
    let result = crate::alloc_list(py, &scratch.api_values);
    (!result.is_null())
        .then_some(result)
        .ok_or("GC introspection result allocation failed")
}

#[inline]
fn scratch_push(storage: &mut Vec<usize>, value: usize) {
    if storage.len() >= storage.capacity() {
        std::process::abort();
    }
    storage.push(value);
}

unsafe fn deduce_subset(
    py: &PyToken<'_>,
    candidates: &[GcCandidate],
    index: &HashMap<u64, usize>,
    refs: &mut [i64],
    marks: &mut [u8],
    queue: &mut Vec<usize>,
    subset: Option<&[usize]>,
    output: &mut Vec<usize>,
) {
    marks.fill(0);
    queue.clear();
    output.clear();

    let mut initialize = |candidate_index: usize| {
        let ptr = candidates[candidate_index].ptr.0;
        refs[candidate_index] = unsafe { effective_gc_refcount(ptr) };
        marks[candidate_index] = 1;
        unsafe { header_set_collecting(ptr, true) };
    };
    if let Some(subset) = subset {
        for &candidate_index in subset {
            initialize(candidate_index);
        }
    } else {
        for candidate_index in 0..candidates.len() {
            initialize(candidate_index);
        }
    }

    let mut subtract = |candidate_index: usize| unsafe {
        molt_traverse(py, candidates[candidate_index].ptr.0, &mut |child| {
            if let Some(&child_index) = index.get(&addr_key(child))
                && marks[child_index] == 1
            {
                refs[child_index] -= 1;
            }
        });
    };
    if let Some(subset) = subset {
        for &candidate_index in subset {
            subtract(candidate_index);
        }
    } else {
        for candidate_index in 0..candidates.len() {
            subtract(candidate_index);
        }
    }

    let mut seed = |candidate_index: usize| {
        if refs[candidate_index] > 0 {
            marks[candidate_index] = 2;
            scratch_push(queue, candidate_index);
        }
    };
    if let Some(subset) = subset {
        for &candidate_index in subset {
            seed(candidate_index);
        }
    } else {
        for candidate_index in 0..candidates.len() {
            seed(candidate_index);
        }
    }

    while let Some(candidate_index) = queue.pop() {
        unsafe {
            molt_traverse(py, candidates[candidate_index].ptr.0, &mut |child| {
                if let Some(&child_index) = index.get(&addr_key(child))
                    && marks[child_index] == 1
                {
                    marks[child_index] = 2;
                    scratch_push(queue, child_index);
                }
            });
        }
    }

    let mut partition = |candidate_index: usize| {
        if marks[candidate_index] == 2 {
            unsafe { header_set_collecting(candidates[candidate_index].ptr.0, false) };
        } else {
            scratch_push(output, candidate_index);
        }
    };
    if let Some(subset) = subset {
        for &candidate_index in subset {
            partition(candidate_index);
        }
    } else {
        for candidate_index in 0..candidates.len() {
            partition(candidate_index);
        }
    }
}

unsafe fn deduce_all(py: &PyToken<'_>, scratch: &mut GcScratch) {
    let GcScratch {
        candidates,
        index,
        refs,
        marks,
        queue,
        first_unreachable,
        first_unreachable_ptrs,
        ..
    } = scratch;
    unsafe {
        deduce_subset(
            py,
            candidates,
            index,
            refs,
            marks,
            queue,
            None,
            first_unreachable,
        )
    };
    first_unreachable_ptrs.clear();
    for &candidate_index in first_unreachable.iter() {
        if first_unreachable_ptrs.len() >= first_unreachable_ptrs.capacity() {
            std::process::abort();
        }
        first_unreachable_ptrs.push(candidates[candidate_index].ptr);
    }
}

unsafe fn deduce_after_finalizers(py: &PyToken<'_>, scratch: &mut GcScratch) {
    let GcScratch {
        candidates,
        index,
        refs,
        marks,
        queue,
        first_unreachable,
        first_unreachable_ptrs: _,
        final_unreachable,
        ..
    } = scratch;
    unsafe {
        deduce_subset(
            py,
            candidates,
            index,
            refs,
            marks,
            queue,
            Some(first_unreachable.as_slice()),
            final_unreachable,
        )
    };
}

fn gc_callback_info(
    py: &PyToken<'_>,
    generation: u8,
    collected: usize,
    uncollectable: usize,
) -> Option<*mut u8> {
    let keys: [&[u8]; 3] = [b"generation", b"collected", b"uncollectable"];
    let values = [generation as usize, collected, uncollectable];
    let mut pairs = [0u64; 6];
    let mut owned_keys = [0u64; 3];
    for (index, (key, value)) in keys.into_iter().zip(values).enumerate() {
        let key_ptr = crate::alloc_string(py, key);
        if key_ptr.is_null() {
            for bits in &owned_keys[..index] {
                crate::dec_ref_bits(py, *bits);
            }
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value = i64::try_from(value).unwrap_or_else(|_| std::process::abort());
        pairs[index * 2] = key_bits;
        pairs[index * 2 + 1] = MoltObject::from_int(value).bits();
        owned_keys[index] = key_bits;
    }
    let info = crate::alloc_dict_with_pairs(py, &pairs);
    for bits in owned_keys {
        crate::dec_ref_bits(py, bits);
    }
    (!info.is_null()).then_some(info)
}

fn invoke_gc_callbacks(
    py: &PyToken<'_>,
    scratch: &mut GcScratch,
    phase: &'static [u8],
    generation: u8,
    collected: usize,
) -> Result<(), &'static str> {
    let callbacks_bits = crate::runtime_state(py).gc.existing_api_root_bits(py, true);
    if callbacks_bits == 0 {
        return Ok(());
    }
    let Some(callbacks_ptr) = crate::obj_from_bits(callbacks_bits).as_ptr() else {
        crate::dec_ref_bits(py, callbacks_bits);
        return Ok(());
    };
    scratch.api_targets.clear();
    let callback_count =
        unsafe { super::seq_access::with_borrowed(callbacks_ptr, |callbacks| callbacks.len()) };
    if !try_reserve_total(&mut scratch.api_targets, callback_count) {
        crate::dec_ref_bits(py, callbacks_bits);
        return Err("GC callback snapshot allocation failed");
    }
    unsafe {
        super::seq_access::with_borrowed(callbacks_ptr, |callbacks| {
            for &callback in callbacks {
                crate::inc_ref_bits(py, callback);
                scratch.api_targets.push(callback);
            }
        });
    }
    crate::dec_ref_bits(py, callbacks_bits);
    if scratch.api_targets.is_empty() {
        return Ok(());
    }

    let phase_ptr = crate::alloc_string(py, phase);
    let Some(info_ptr) = gc_callback_info(py, generation, collected, 0) else {
        for callback in scratch.api_targets.drain(..) {
            crate::dec_ref_bits(py, callback);
        }
        if !phase_ptr.is_null() {
            crate::dec_ref_bits(py, MoltObject::from_ptr(phase_ptr).bits());
        }
        return Err("GC callback argument allocation failed");
    };
    if phase_ptr.is_null() {
        for callback in scratch.api_targets.drain(..) {
            crate::dec_ref_bits(py, callback);
        }
        crate::dec_ref_bits(py, MoltObject::from_ptr(info_ptr).bits());
        return Err("GC callback argument allocation failed");
    }
    let phase_bits = MoltObject::from_ptr(phase_ptr).bits();
    let info_bits = MoltObject::from_ptr(info_ptr).bits();
    for callback in scratch.api_targets.drain(..) {
        let result = crate::builtins::exceptions::run_unraisable(
            py,
            callback,
            Some("Exception ignored in gc callback"),
            || unsafe {
                crate::call::dispatch::call_callable2(py, callback, phase_bits, info_bits)
            },
        );
        if !crate::obj_from_bits(result).is_none() {
            crate::dec_ref_bits(py, result);
        }
        crate::dec_ref_bits(py, callback);
    }
    crate::dec_ref_bits(py, phase_bits);
    crate::dec_ref_bits(py, info_bits);
    Ok(())
}

fn completed_collection(
    py: &PyToken<'_>,
    generation: u8,
    collected: usize,
    scanned: usize,
    survivors: usize,
) -> CollectStats {
    crate::runtime_state(py)
        .gc
        .finish_collection(generation, scanned, collected, survivors);
    CollectStats::completed(collected, scanned, survivors)
}

/// Collect one CPython generation, including every younger generation.
/// Stop-the-world under the deterministic GIL.
///
/// # Safety
/// The GIL must be held (asserted). Reentrancy is prevented by `GC_RUNNING`.
pub(crate) unsafe fn collect_generation(py: &PyToken<'_>, generation: u8) -> CollectStats {
    unsafe {
        crate::gil_assert();
        debug_assert!((generation as usize) < NUM_GENERATIONS);

        if cfg!(feature = "free-threaded") {
            // Raw candidate traversal requires a runtime-owned stop-the-world
            // epoch. Until the free-threaded scheduler exposes that guard, fail
            // before snapshot/mutation instead of treating a GIL token as STW.
            return CollectStats::failure(py, GcCollectStatus::UnsupportedConcurrency);
        }

        // Reentrancy guard: a `__del__` run during finalization must not recursively
        // launch another collection (CPython sets `gcstate->collecting`).
        let gc_running = &crate::runtime_state(py).gc_running;
        if gc_running.swap(true, AtomicOrdering::AcqRel) {
            return CollectStats::failure(py, GcCollectStatus::ReentrantNoop);
        }
        let _guard = GcRunningGuard(gc_running);
        crate::runtime_state(py)
            .gc_last_failure
            .store(0, AtomicOrdering::Release);
        let mut scratch = GcScratch::acquire();
        if let Err(message) = invoke_gc_callbacks(py, &mut scratch, b"start", generation, 0) {
            return CollectStats::failure(py, GcCollectStatus::ResourceError(message));
        }
        let outcome = (|| {
            crate::runtime_state(py).gc.begin_collection(generation);

            // Snapshot directly into the reusable collector workspace. The registry
            // mutex is released before traversal so re-entrant dec_ref during
            // finalize/clear can update it; allocation ordinals preserve order.
            let snapshot_ok = snapshot_tracked_registry(&mut scratch.candidates, generation);
            if !snapshot_ok {
                return CollectStats::failure(
                    py,
                    GcCollectStatus::ResourceError("tracked-registry snapshot allocation failed"),
                );
            }
            let scanned = scratch.candidates.len();
            let target_generation = generation.saturating_add(1).min(OLDEST_GENERATION);
            let debug_flags = crate::runtime_state(py).gc.debug_flags();
            if gc_trace_enabled() || debug_flags & DEBUG_STATS != 0 {
                eprintln!("molt gc: generation={generation} candidates={scanned}",);
            }
            if scratch.candidates.is_empty() {
                return completed_collection(py, generation, 0, 0, 0);
            }

            if !scratch.try_prepare_candidates() {
                profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
                return CollectStats::failure(
                    py,
                    GcCollectStatus::ResourceError("cycle-collector scratch allocation failed"),
                );
            }

            // STEP 1-3: trial-deletion partition using one preallocated index/mark arena.
            deduce_all(py, &mut scratch);
            reproject_reachable_immutable_tuples(py, &scratch.candidates, &scratch.marks);
            if gc_trace_enabled() || debug_flags & DEBUG_STATS != 0 {
                eprintln!(
                    "molt gc: deduce_unreachable unreachable={}",
                    scratch.first_unreachable.len()
                );
            }
            if scratch.first_unreachable.is_empty() {
                let survivors = promote_marked_candidates(
                    &scratch.candidates,
                    &scratch.marks,
                    2,
                    target_generation,
                );
                return completed_collection(py, generation, 0, scanned, survivors);
            }

            // Reserve the current detach high-water before any callback. Finalizers
            // may grow a still-unreachable container; that case is revalidated
            // fallibly before mutation and restores every pin on failure.
            let (initial_edges, initial_resources) =
                detach_requirements(py, &scratch.candidates, &scratch.first_unreachable);
            let Some(mut detached) = super::heap_lifecycle::DetachedEdgeSink::try_with_capacities(
                initial_edges,
                initial_resources,
            ) else {
                for &PtrSlot(ptr) in &scratch.first_unreachable_ptrs {
                    header_set_collecting(ptr, false);
                }
                profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
                return CollectStats::failure(
                    py,
                    GcCollectStatus::ResourceError("detached-edge reservation failed"),
                );
            };

            // CPython promotes the reachable partition before weakref callbacks and
            // finalizers. Once the detach reservation succeeds, the collection has
            // a stable destruction workspace and promotion cannot be rolled back.
            let mut survivors = promote_marked_candidates(
                &scratch.candidates,
                &scratch.marks,
                2,
                target_generation,
            );

            // Pin the entire set before the first callback/finalizer.
            pin_unreachable(&scratch.first_unreachable_ptrs);

            crate::object::weakref::weakref_handle_cycle_unreachable(
                py,
                &scratch.first_unreachable_ptrs,
                |wr_ptr| header_is_collecting(wr_ptr),
            );

            for &PtrSlot(ptr) in &scratch.first_unreachable_ptrs {
                run_finalizer_once(py, ptr);
            }

            // Reuse the exact same index, refs, mark, queue, and output storage.
            // No allocation is permitted in the post-callback resurrection partition.
            deduce_after_finalizers(py, &mut scratch);
            survivors += promote_marked_candidates(
                &scratch.candidates,
                &scratch.marks,
                2,
                target_generation,
            );
            if scratch.final_unreachable.is_empty() {
                release_unreachable_pins(py, &scratch.first_unreachable_ptrs);
                return completed_collection(py, generation, 0, scanned, survivors);
            }

            // Marks == 2 are resurrected/reachable after the second partition.
            for &candidate_index in &scratch.first_unreachable {
                if scratch.marks[candidate_index] == 2 {
                    let ptr = scratch.candidates[candidate_index].ptr.0;
                    let header = header_from_obj_ptr(ptr);
                    (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED);
                    dec_ref_ptr(py, ptr);
                }
            }

            let collected = scratch.final_unreachable.len();
            if gc_trace_enabled() || debug_flags & DEBUG_STATS != 0 {
                eprintln!("molt gc: delete_garbage collected={collected}");
            }

            if debug_flags & DEBUG_COLLECTABLE != 0 {
                for &candidate_index in &scratch.final_unreachable {
                    let object = MoltObject::from_ptr(scratch.candidates[candidate_index].ptr.0);
                    eprintln!("gc: collectable <{}>", crate::type_name(py, object));
                }
            }

            if debug_flags & DEBUG_SAVEALL != 0 {
                let garbage_bits = crate::runtime_state(py)
                    .gc
                    .existing_api_root_bits(py, false);
                if garbage_bits == 0 {
                    for &candidate_index in &scratch.final_unreachable {
                        header_set_collecting(scratch.candidates[candidate_index].ptr.0, false);
                    }
                    release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);
                    return CollectStats::failure(
                        py,
                        GcCollectStatus::ResourceError(
                            "gc.garbage is unavailable for DEBUG_SAVEALL",
                        ),
                    );
                }
                let mut appended_all = true;
                for &candidate_index in &scratch.final_unreachable {
                    let bits =
                        MoltObject::from_ptr(scratch.candidates[candidate_index].ptr.0).bits();
                    appended_all &= crate::object::ops_list::molt_list_append_with_projection(
                        garbage_bits,
                        bits,
                        std::ptr::null_mut(),
                    );
                }
                crate::dec_ref_bits(py, garbage_bits);
                for &candidate_index in &scratch.final_unreachable {
                    header_set_collecting(scratch.candidates[candidate_index].ptr.0, false);
                }
                release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);
                if !appended_all {
                    return CollectStats::failure(
                        py,
                        GcCollectStatus::ResourceError("gc.garbage append failed"),
                    );
                }
                return completed_collection(py, generation, collected, scanned, survivors);
            }

            let (required_edges, required_resources) =
                detach_requirements(py, &scratch.candidates, &scratch.final_unreachable);
            if !detached.try_ensure_capacities(required_edges, required_resources) {
                for &candidate_index in &scratch.final_unreachable {
                    header_set_collecting(scratch.candidates[candidate_index].ptr.0, false);
                }
                release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);
                profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
                return CollectStats::failure(
                    py,
                    GcCollectStatus::ResourceError(
                        "post-finalizer detached-edge reservation failed",
                    ),
                );
            }

            for &candidate_index in &scratch.final_unreachable {
                header_set_collecting(scratch.candidates[candidate_index].ptr.0, false);
            }
            for &candidate_index in &scratch.final_unreachable {
                super::heap_lifecycle::clear_cycle_edges_with_sink(
                    py,
                    scratch.candidates[candidate_index].ptr.0,
                    &mut detached,
                );
            }
            detached.release_all(py);
            release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);

            crate::runtime_state(py)
                .gc_last_failure
                .store(0, AtomicOrdering::Release);
            completed_collection(py, generation, collected, scanned, survivors)
        })();
        let _ = invoke_gc_callbacks(py, &mut scratch, b"stop", generation, outcome.collected);
        outcome
    }
}

/// Full explicit/shutdown collection (the default `gc.collect()` generation).
pub(crate) unsafe fn collect_cycles(py: &PyToken<'_>) -> CollectStats {
    unsafe { collect_generation(py, OLDEST_GENERATION) }
}

/// Consume an allocation-scheduled collection at a generated runtime safepoint.
/// A recursive finalizer poll re-arms the request for the next outer safepoint;
/// resource failure likewise preserves pressure rather than silently disabling
/// automatic GC.
pub(crate) unsafe fn collect_pending(py: &PyToken<'_>) -> CollectStats {
    let state = &crate::runtime_state(py).gc;
    let Some(generation) = state.take_scheduled_generation() else {
        return CollectStats::completed(0, 0, 0);
    };
    let outcome = unsafe { collect_generation(py, generation) };
    if matches!(
        outcome.status,
        GcCollectStatus::ReentrantNoop | GcCollectStatus::ResourceError(_)
    ) {
        state.rearm_pending();
    }
    outcome
}

/// Run an object's `__del__` exactly once during cyclic finalization, WITHOUT the
/// acyclic path's inc/dec-self + `prev>1` resurrection verdict (which is wrong in a
/// cycle, where every member has rc≥1 from its peers — `prev>1` would always be
/// true). Resurrection in the cycle path is detected by the re-run of
/// `deduce_unreachable`, not here. Shares the underlying `__del__`-invocation
/// machinery with the acyclic path via `maybe_run_object_finalizer_for_cycle`.
///
/// # Safety
/// GIL held; `ptr` is a live unreachable object.
unsafe fn run_finalizer_once(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        if !object_class_has_finalizer(ptr) {
            return;
        }
        if (*header).has_flag(HEADER_FLAG_FINALIZER_RAN) {
            return;
        }
        crate::object::maybe_run_object_finalizer_for_cycle(py, ptr);
    }
}

/// Reentrancy flag for `collect_cycles` (CPython `gcstate->collecting`).
struct GcRunningGuard(&'static std::sync::atomic::AtomicBool);
impl Drop for GcRunningGuard {
    fn drop(&mut self) {
        self.0.store(false, AtomicOrdering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::builders::{alloc_dict_with_pairs, alloc_list, alloc_tuple};
    use crate::object::dec_ref_bits;
    use crate::object::{
        TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_LIST, TYPE_ID_OBJECT, TYPE_ID_SET, TYPE_ID_TUPLE,
    };
    use crate::{DEALLOC_COUNT, obj_from_bits};
    use std::sync::atomic::Ordering;

    #[test]
    #[ignore = "release cyclic-GC workspace allocation/time probe"]
    fn repeated_reachable_collection_workspace_bench() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            const OBJECTS: usize = 4_096;
            const ROUNDS: usize = 101;
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };
            let roots = (0..OBJECTS)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();

            // Warm all lazy runtime and collector state outside the sample.
            assert_eq!(unsafe { collect_cycles(_py) }.collected, 0);
            let mut samples = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                let started = std::time::Instant::now();
                assert_eq!(unsafe { collect_cycles(_py) }.collected, 0);
                samples.push(started.elapsed().as_nanos() as u64);
            }
            samples.sort_unstable();

            println!(
                "{{\"objects\":{OBJECTS},\"rounds\":{ROUNDS},\"median_ns\":{},\"p95_ns\":{}}}",
                samples[ROUNDS / 2],
                samples[ROUNDS * 95 / 100],
            );
            for bits in roots {
                dec_ref_bits(_py, bits);
            }
            state.reset();
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    #[ignore = "cycle-capable allocation/deallocation registry hot-path probe"]
    fn cycle_capable_registry_hot_path_bench() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            const OBJECTS: usize = 4_096;
            const ROUNDS: usize = 31;
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };
            let mut roots = Vec::with_capacity(OBJECTS);

            // Warm allocator, registry shards, and Vec capacity outside the sample.
            for _ in 0..OBJECTS {
                let ptr = alloc_list(_py, &[]);
                assert!(!ptr.is_null());
                roots.push(MoltObject::from_ptr(ptr).bits());
            }
            for bits in roots.drain(..) {
                dec_ref_bits(_py, bits);
            }

            let mut alloc_ns = Vec::with_capacity(ROUNDS);
            let mut dealloc_ns = Vec::with_capacity(ROUNDS);
            let mut round_ns = Vec::with_capacity(ROUNDS);
            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            GC_REGISTRY_ACCESS_COUNT.store(0, AtomicOrdering::Relaxed);
            let lock_contention_before =
                GC_REGISTRY_LOCK_CONTENTION_COUNT.load(AtomicOrdering::Relaxed);
            let lock_wait_before = GC_REGISTRY_LOCK_WAIT_NS.load(AtomicOrdering::Relaxed);
            for _ in 0..ROUNDS {
                let round_started = std::time::Instant::now();
                let alloc_started = std::time::Instant::now();
                for _ in 0..OBJECTS {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    roots.push(MoltObject::from_ptr(ptr).bits());
                }
                alloc_ns.push(alloc_started.elapsed().as_nanos() as u64);
                let dealloc_started = std::time::Instant::now();
                for bits in roots.drain(..) {
                    dec_ref_bits(_py, bits);
                }
                dealloc_ns.push(dealloc_started.elapsed().as_nanos() as u64);
                round_ns.push(round_started.elapsed().as_nanos() as u64);
            }
            crate::attestation_probe::set_tracking(false);
            let observed = crate::attestation_probe::snapshot();
            let registry_accesses = GC_REGISTRY_ACCESS_COUNT.load(AtomicOrdering::Relaxed);
            let lock_contention = GC_REGISTRY_LOCK_CONTENTION_COUNT
                .load(AtomicOrdering::Relaxed)
                .saturating_sub(lock_contention_before);
            let lock_wait_ns = GC_REGISTRY_LOCK_WAIT_NS
                .load(AtomicOrdering::Relaxed)
                .saturating_sub(lock_wait_before);
            alloc_ns.sort_unstable();
            dealloc_ns.sort_unstable();
            round_ns.sort_unstable();
            println!(
                "{{\"objects\":{OBJECTS},\"rounds\":{ROUNDS},\"registry_accesses\":{registry_accesses},\"lock_contention\":{lock_contention},\"lock_wait_ns\":{lock_wait_ns},\"allocations\":{},\"allocated_bytes\":{},\"peak_live_bytes\":{},\"alloc_median_ns\":{},\"alloc_p95_ns\":{},\"dealloc_median_ns\":{},\"dealloc_p95_ns\":{},\"round_median_ns\":{},\"round_p95_ns\":{}}}",
                observed.allocations,
                observed.allocated_bytes,
                observed.peak_live_bytes,
                alloc_ns[ROUNDS / 2],
                alloc_ns[ROUNDS * 95 / 100],
                dealloc_ns[ROUNDS / 2],
                dealloc_ns[ROUNDS * 95 / 100],
                round_ns[ROUNDS / 2],
                round_ns[ROUNDS * 95 / 100],
            );
            assert_eq!(registry_accesses, (OBJECTS * ROUNDS * 2) as u64);
            state.reset();
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn repeated_reachable_collection_workspace_allocations() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };
            let roots = (0..256)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            assert_eq!(unsafe { collect_cycles(_py) }.collected, 0);

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..100 {
                assert_eq!(unsafe { collect_cycles(_py) }.collected, 0);
            }
            crate::attestation_probe::set_tracking(false);
            let observed = crate::attestation_probe::snapshot();
            println!("{observed:?}");
            assert_eq!(observed.allocations, 0, "{observed:?}");
            assert_eq!(observed.allocated_bytes, 0, "{observed:?}");
            assert_eq!(observed.peak_live_bytes, 0, "{observed:?}");

            for bits in roots {
                dec_ref_bits(_py, bits);
            }
            state.reset();
        });
    }

    #[test]
    fn generation_control_matches_cpython_312_threshold_and_count_semantics() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let state = GcRuntimeState::new();
            assert!(state.enabled());
            assert_eq!(state.thresholds(), [700, 10, 10]);
            assert_eq!(state.counts(), [0, 0, 0]);

            state.set_thresholds([2, 1, 1]);
            state.on_allocation();
            state.on_allocation();
            assert_eq!(state.take_scheduled_generation(), None);
            state.on_allocation();
            assert_eq!(state.take_scheduled_generation(), Some(0));
            state.begin_collection(0);
            state.finish_collection(0, 3, 0, 3);
            assert_eq!(state.counts(), [0, 1, 0]);
            assert_eq!(state.generation_stats()[0].collections, 1);

            state.set_enabled(false);
            for _ in 0..8 {
                state.on_allocation();
            }
            assert_eq!(state.take_scheduled_generation(), None);
            state.set_enabled(true);
            assert_eq!(state.take_scheduled_generation(), Some(0));

            state.set_thresholds([0, 1, 1]);
            for _ in 0..8 {
                state.on_allocation();
            }
            assert_eq!(state.take_scheduled_generation(), None);
        });
    }

    #[test]
    fn young_collection_excludes_promoted_long_lived_objects() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };

            let long_lived = (0..512)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            let promotion = unsafe { collect_generation(_py, 0) };
            assert_eq!(promotion.status, GcCollectStatus::Completed);
            assert_eq!(promotion.scanned, 512);
            assert_eq!(promotion.survivors, 512);

            let young = (0..16)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            let young_only = unsafe { collect_generation(_py, 0) };
            assert_eq!(young_only.status, GcCollectStatus::Completed);
            assert_eq!(young_only.scanned, 16);
            assert_eq!(young_only.survivors, 16);
            assert_eq!(promotion.scanned / young_only.scanned, 32);

            for bits in young.into_iter().chain(long_lived) {
                dec_ref_bits(_py, bits);
            }
            state.reset();
        });
    }

    #[test]
    fn automatic_collection_is_deferred_to_the_runtime_safepoint() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };
            state.reset();
            state.set_thresholds([2, 10, 10]);

            let roots = (0..3)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            assert_eq!(state.counts()[0], 3);
            assert_eq!(state.generation_stats()[0].collections, 0);

            let outcome = unsafe { collect_pending(_py) };
            assert_eq!(outcome.status, GcCollectStatus::Completed);
            assert_eq!(outcome.scanned, 3);
            assert_eq!(outcome.survivors, 3);
            assert_eq!(state.counts(), [0, 1, 0]);
            assert_eq!(state.generation_stats()[0].collections, 1);

            for bits in roots {
                dec_ref_bits(_py, bits);
            }
            state.reset();
        });
    }

    #[test]
    #[ignore = "generational GC long-lived/young scan and tail-latency probe"]
    fn generational_scan_reduction_bench() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            const LONG_LIVED: usize = 4_096;
            const YOUNG: usize = 64;
            const ROUNDS: usize = 31;
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let baseline = unsafe { collect_cycles(_py) }.scanned;
            let roots = (0..LONG_LIVED)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            let full_scan = unsafe { collect_cycles(_py) }.scanned;
            assert!(
                (baseline + LONG_LIVED).abs_diff(full_scan) <= baseline,
                "long-lived population did not enter the full-generation snapshot: baseline={baseline} full={full_scan}"
            );

            let mut young_ns = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                let young = (0..YOUNG)
                    .map(|_| {
                        let ptr = alloc_list(_py, &[]);
                        assert!(!ptr.is_null());
                        MoltObject::from_ptr(ptr).bits()
                    })
                    .collect::<Vec<_>>();
                let started = Instant::now();
                let outcome = unsafe { collect_generation(_py, 0) };
                young_ns.push(started.elapsed().as_nanos() as u64);
                assert_eq!(outcome.scanned, YOUNG);
                for bits in young {
                    dec_ref_bits(_py, bits);
                }
            }
            let mut full_ns = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                let started = Instant::now();
                let outcome = unsafe { collect_cycles(_py) };
                full_ns.push(started.elapsed().as_nanos() as u64);
                assert_eq!(outcome.scanned, full_scan);
            }
            young_ns.sort_unstable();
            full_ns.sort_unstable();
            println!(
                "{{\"long_lived\":{LONG_LIVED},\"young\":{YOUNG},\"rounds\":{ROUNDS},\"scan_reduction_x\":{},\"young_median_ns\":{},\"young_p95_ns\":{},\"full_median_ns\":{},\"full_p95_ns\":{}}}",
                full_scan / YOUNG,
                young_ns[ROUNDS / 2],
                young_ns[ROUNDS * 95 / 100],
                full_ns[ROUNDS / 2],
                full_ns[ROUNDS * 95 / 100],
            );
            for bits in roots {
                dec_ref_bits(_py, bits);
            }
            state.reset();
        });
    }

    #[test]
    fn compact_aligned_arenas_spread_across_every_registry_shard() {
        let mut counts = [0usize; TRACKED_REGISTRY_SHARDS];
        for index in 0..4096usize {
            let address = 0x1_0000usize + index * 24;
            counts[tracked_registry_shard_index_from_address(address)] += 1;
        }
        assert!(counts.iter().all(|count| *count > 0), "{counts:?}");
        assert!(
            counts.iter().copied().max().unwrap_or(0) < 100,
            "compact arena distribution is pathologically skewed: {counts:?}"
        );
    }

    #[test]
    fn process_registry_rejects_competing_runtime_owner() {
        let owner = AtomicUsize::new(0);
        assert_eq!(claim_registry_owner(&owner, 0x111), Ok(()));
        assert_eq!(claim_registry_owner(&owner, 0x111), Ok(()));
        assert_eq!(claim_registry_owner(&owner, 0x222), Err(0x111));
        assert_eq!(release_registry_owner(&owner, 0x222), Err(0x111));
        assert_eq!(release_registry_owner(&owner, 0x111), Ok(()));
        assert_eq!(claim_registry_owner(&owner, 0x222), Ok(()));
        assert_eq!(release_registry_owner(&owner, 0x222), Ok(()));
    }

    #[test]
    fn may_form_cycle_is_green_for_leaf_types() {
        // GREEN: leaf/atomic types pay zero — never tracked.
        assert!(!may_form_cycle(crate::object::TYPE_ID_STRING));
        assert!(!may_form_cycle(crate::object::TYPE_ID_BIGINT));
        assert!(!may_form_cycle(crate::object::TYPE_ID_FLOAT));
        // Tracked: the canonical cycle formers.
        assert!(may_form_cycle(TYPE_ID_OBJECT));
        assert!(may_form_cycle(TYPE_ID_DICT));
        assert!(may_form_cycle(TYPE_ID_LIST));
        assert!(may_form_cycle(TYPE_ID_TUPLE));
        assert!(may_form_cycle(TYPE_ID_SET));
        assert!(may_form_cycle(TYPE_ID_EXCEPTION));
    }

    #[cfg(feature = "free-threaded")]
    #[test]
    fn free_threaded_collection_rejects_known_cycle_before_mutation() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let left = alloc_list(_py, &[]);
            let right = alloc_list(_py, &[]);
            let left_bits = MoltObject::from_ptr(left).bits();
            let right_bits = MoltObject::from_ptr(right).bits();
            crate::molt_list_append(left_bits, right_bits);
            crate::molt_list_append(right_bits, left_bits);

            let left_before = unsafe { header_refcount(left) };
            let right_before = unsafe { header_refcount(right) };
            let outcome = unsafe { collect_cycles(_py) };
            assert_eq!(outcome.status, GcCollectStatus::UnsupportedConcurrency);
            assert_eq!(outcome.collected, 0);
            assert_eq!(unsafe { header_refcount(left) }, left_before);
            assert_eq!(unsafe { header_refcount(right) }, right_before);
            assert!(unsafe { gc_is_tracked(left) && gc_is_tracked(right) });
            assert_eq!(
                crate::runtime_state(_py)
                    .gc_last_failure
                    .load(AtomicOrdering::Acquire),
                3
            );

            // Retained stack roots make deterministic test cleanup safe even
            // though the feature deliberately refuses cyclic collection.
            unsafe { super::molt_clear(_py, left) };
            unsafe { super::molt_clear(_py, right) };
            dec_ref_bits(_py, left_bits);
            dec_ref_bits(_py, right_bits);
        });
    }

    #[test]
    fn lifecycle_visit_is_side_effect_free_and_clear_is_idempotent() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let left = alloc_list(_py, &[]);
            let right = alloc_list(_py, &[]);
            let left_bits = MoltObject::from_ptr(left).bits();
            let right_bits = MoltObject::from_ptr(right).bits();
            let owner = alloc_list(_py, &[left_bits, right_bits]);
            assert!(!left.is_null() && !right.is_null() && !owner.is_null());

            let before = unsafe { (header_refcount(left), header_refcount(right)) };
            let mut visited = Vec::new();
            unsafe {
                super::molt_traverse(_py, owner, &mut |child| visited.push(child));
            }
            assert_eq!(visited, vec![left, right]);
            assert_eq!(
                unsafe { (header_refcount(left), header_refcount(right)) },
                before,
                "visit must not transiently INCREF/DECREF owned edges"
            );

            unsafe { super::molt_clear(_py, owner) };
            assert_eq!(unsafe { header_refcount(left) }, before.0 - 1);
            assert_eq!(unsafe { header_refcount(right) }, before.1 - 1);
            let after_first_clear = unsafe { (header_refcount(left), header_refcount(right)) };
            unsafe { super::molt_clear(_py, owner) };
            assert_eq!(
                unsafe { (header_refcount(left), header_refcount(right)) },
                after_first_clear,
                "a second clear must release no edge twice"
            );

            dec_ref_bits(_py, MoltObject::from_ptr(owner).bits());
            dec_ref_bits(_py, left_bits);
            dec_ref_bits(_py, right_bits);
        });
    }

    #[test]
    fn dynamic_dict_and_tuple_tracking_matches_cpython_timing() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let empty_dict = alloc_dict_with_pairs(_py, &[]);
            assert!(!unsafe { gc_is_tracked(empty_dict) });

            let direct_dict_bits = crate::molt_dict_new(16);
            let direct_dict = obj_from_bits(direct_dict_bits)
                .as_ptr()
                .expect("direct dict allocation");
            assert!(
                !unsafe { gc_is_tracked(direct_dict) },
                "every exact empty-dict constructor must apply dynamic projection"
            );

            let atomic_dict = alloc_dict_with_pairs(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert!(!unsafe { gc_is_tracked(atomic_dict) });

            let list = alloc_list(_py, &[]);
            let list_bits = MoltObject::from_ptr(list).bits();
            let container_dict =
                alloc_dict_with_pairs(_py, &[MoltObject::from_int(1).bits(), list_bits]);
            assert!(unsafe { gc_is_tracked(container_dict) });

            let atomic_tuple = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
            assert!(unsafe { gc_is_tracked(atomic_tuple) });
            let _ = unsafe { collect_cycles(_py) };
            assert!(!unsafe { gc_is_tracked(atomic_tuple) });

            let container_tuple = alloc_tuple(_py, &[list_bits]);
            assert!(unsafe { gc_is_tracked(container_tuple) });
            let _ = unsafe { collect_cycles(_py) };
            assert!(unsafe { gc_is_tracked(container_tuple) });

            dec_ref_bits(_py, MoltObject::from_ptr(empty_dict).bits());
            dec_ref_bits(_py, direct_dict_bits);
            dec_ref_bits(_py, MoltObject::from_ptr(atomic_dict).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(container_dict).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(atomic_tuple).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(container_tuple).bits());
            dec_ref_bits(_py, list_bits);
        });
    }

    #[test]
    fn exception_self_cycle_with_physical_abi_projection_is_collectible() {
        let _guard = crate::test_mutex_guard();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_exception(_py, "RuntimeError", "cycle");
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                bits,
            )
            .expect("self context edge");

            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            let exception_view = view.cast::<molt_cpython_abi::abi_types::PyBaseExceptionObject>();
            assert_ne!(
                unsafe { (*exception_view).context },
                std::ptr::null_mut(),
                "the physical context projection must be a second owned GC edge"
            );

            crate::dec_ref_bits(_py, bits);
            let stats = unsafe { collect_cycles(_py) };
            assert_eq!(
                stats.collected, 2,
                "the exception and its tracked args tuple are both cycle-garbage candidates"
            );
            assert!(!crate::exception_pending(_py));
        });
    }

    #[test]
    fn exception_landing_external_c_ref_roots_self_cycle_until_released() {
        let _guard = crate::test_mutex_guard();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_exception(_py, "RuntimeError", "externally rooted cycle");
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                bits,
            )
            .expect("self context edge");
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            unsafe { molt_cpython_abi::api::refcount::Py_INCREF(view) };

            crate::dec_ref_bits(_py, bits);
            assert_eq!(
                unsafe { collect_cycles(_py) }.collected,
                0,
                "the direct C reference is an external GC root"
            );
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
            assert_eq!(
                unsafe { collect_cycles(_py) }.collected,
                1,
                "the prior collection projected the atomic args tuple out of GC; releasing the direct C root exposes the exception cycle"
            );
            assert!(!crate::exception_pending(_py));
        });
    }

    /// End-to-end proof: a 2-cycle of lists `a -> b -> a`, unreachable after the
    /// stack roots are dropped, is RECLAIMED by `collect_cycles` (pure RC cannot —
    /// each list stays pinned at rc 1 by its peer). Asserts the deallocator actually
    /// ran (DEALLOC_COUNT rose by the two cycle members) and both are gone from the
    /// tracked registry.
    #[test]
    fn collect_reclaims_unreachable_list_cycle() {
        let _lock = crate::test_mutex_guard();
        // Force-enable the alloc/dealloc counters so DEALLOC_COUNT is a live signal
        // (otherwise `profile_hit` is a no-op and the deallocation is invisible to the
        // counter, though `gc_is_tracked` below remains an unconditional proof).
        // SAFETY: single-threaded test serialized by `test_mutex_guard`.
        unsafe {
            std::env::set_var("MOLT_PROFILE", "1");
        }
        crate::state::metrics::init_profile_enabled_from_env();
        crate::with_gil_entry_nopanic!(_py, {
            // a = []; b = []
            let a_ptr = alloc_list(_py, &[]);
            let b_ptr = alloc_list(_py, &[]);
            assert!(!a_ptr.is_null() && !b_ptr.is_null());
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();

            // a.append(b); b.append(a)  (molt_list_append inc_refs the element).
            crate::molt_list_append(a_bits, b_bits);
            crate::molt_list_append(b_bits, a_bits);

            // Both must be tracked (cycle-capable containers registered at alloc).
            assert!(unsafe { gc_is_tracked(a_ptr) }, "list a should be tracked");
            assert!(unsafe { gc_is_tracked(b_ptr) }, "list b should be tracked");

            // Drop the stack roots. Now a.rc == 1 (held by b) and b.rc == 1 (held by
            // a): a classic unreachable RC cycle that leaks without a collector.
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);
            assert!(
                unsafe { gc_is_tracked(a_ptr) },
                "cycle must still be alive (leaked) before collection"
            );

            let before = DEALLOC_COUNT.load(Ordering::Relaxed);
            let stats = unsafe { collect_cycles(_py) };
            let after = DEALLOC_COUNT.load(Ordering::Relaxed);

            assert_eq!(stats.collected, 2, "both cycle members are collectable");
            assert_eq!(
                after - before,
                2,
                "the deallocator must actually free both list objects"
            );
            assert!(
                !unsafe { gc_is_tracked(a_ptr) },
                "list a must be untracked after reclamation"
            );
            assert!(
                !unsafe { gc_is_tracked(b_ptr) },
                "list b must be untracked after reclamation"
            );
        });
    }

    #[test]
    fn collect_reclaims_cross_shape_list_tuple_cycle() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let list = alloc_list(_py, &[]);
            let list_bits = MoltObject::from_ptr(list).bits();
            let tuple = alloc_tuple(_py, &[list_bits]);
            let tuple_bits = MoltObject::from_ptr(tuple).bits();
            crate::molt_list_append(list_bits, tuple_bits);

            assert!(unsafe { gc_is_tracked(list) });
            assert!(unsafe { gc_is_tracked(tuple) });
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, tuple_bits);

            let stats = unsafe { collect_cycles(_py) };
            assert_eq!(stats.collected, 2);
            assert!(!unsafe { gc_is_tracked(list) });
            assert!(!unsafe { gc_is_tracked(tuple) });
        });
    }

    /// Negative case: a cycle that is STILL REACHABLE from a live external root must
    /// NOT be collected (no false reclamation). `outer` holds `a`, and `a -> b -> a`
    /// is a cycle, but `outer` keeps it alive.
    #[test]
    fn collect_spares_externally_reachable_cycle() {
        let _lock = crate::test_mutex_guard();
        // SAFETY: single-threaded test serialized by `test_mutex_guard`.
        unsafe {
            std::env::set_var("MOLT_PROFILE", "1");
        }
        crate::state::metrics::init_profile_enabled_from_env();
        crate::with_gil_entry_nopanic!(_py, {
            let a_ptr = alloc_list(_py, &[]);
            let b_ptr = alloc_list(_py, &[]);
            let outer_ptr = alloc_list(_py, &[]);
            assert!(!a_ptr.is_null() && !b_ptr.is_null() && !outer_ptr.is_null());
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();
            let outer_bits = MoltObject::from_ptr(outer_ptr).bits();

            crate::molt_list_append(a_bits, b_bits); // a -> b
            crate::molt_list_append(b_bits, a_bits); // b -> a (cycle)
            crate::molt_list_append(outer_bits, a_bits); // outer -> a (external root)

            // Drop the a/b stack roots; `outer` (still held) keeps the cycle alive.
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);

            let before = DEALLOC_COUNT.load(Ordering::Relaxed);
            let stats = unsafe { collect_cycles(_py) };
            let after = DEALLOC_COUNT.load(Ordering::Relaxed);

            assert_eq!(
                stats.collected, 0,
                "externally-reachable cycle is NOT garbage"
            );
            assert_eq!(after - before, 0, "nothing may be freed");
            assert!(
                unsafe { gc_is_tracked(a_ptr) },
                "a must remain alive (reachable via outer)"
            );

            // Clean up: dropping outer breaks the external root; the now-unreachable
            // cycle is reclaimable by a subsequent collection.
            dec_ref_bits(_py, outer_bits);
            let stats2 = unsafe { collect_cycles(_py) };
            assert_eq!(
                stats2.collected, 2,
                "after the external root drops, the cycle is collectable"
            );
        });
    }

    #[test]
    fn abi_view_hold_does_not_root_an_unreachable_cycle() {
        let _lock = crate::test_mutex_guard();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let a_ptr = alloc_list(_py, &[]);
            let b_ptr = alloc_list(_py, &[]);
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();
            crate::molt_list_append(a_bits, b_bits);
            crate::molt_list_append(b_bits, a_bits);
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(a_bits) };
            assert!(!view.is_null());
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);
            let stats = unsafe { collect_cycles(_py) };
            assert_eq!(stats.collected, 2, "view hold is not a GC root");
        });
    }

    #[test]
    fn direct_c_reference_roots_viewed_cycle_until_released() {
        let _lock = crate::test_mutex_guard();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let a_ptr = alloc_list(_py, &[]);
            let b_ptr = alloc_list(_py, &[]);
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();
            crate::molt_list_append(a_bits, b_bits);
            crate::molt_list_append(b_bits, a_bits);
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(a_bits) };
            assert!(!view.is_null());
            unsafe { molt_cpython_abi::api::refcount::Py_INCREF(view) };
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);
            assert_eq!(unsafe { collect_cycles(_py) }.collected, 0);
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
            assert_eq!(unsafe { collect_cycles(_py) }.collected, 2);
        });
    }
}
