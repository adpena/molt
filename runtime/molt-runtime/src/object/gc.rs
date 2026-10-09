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
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Instant;

use crate::object::{
    HEADER_FLAG_FINALIZER_RAN, HEADER_FLAG_GC_ACCOUNTED, HEADER_FLAG_GC_COLLECTING,
    HEADER_FLAG_GC_PINNED, HEADER_FLAG_HAS_ABI_VIEW, PtrSlot, dec_ref_ptr, header_from_obj_ptr,
    object_has_finalizer, object_type_id,
};
use crate::{
    GC_REGISTRY_LOCK_CONTENTION_COUNT, GC_REGISTRY_LOCK_WAIT_NS, GC_SNAPSHOT_ALLOC_FAILURE_COUNT,
    GC_TRACK_COUNT, GC_TRACKED_HIGH_WATER, GC_TRACKED_LIVE, GC_UNTRACK_COUNT, MoltObject, PyToken,
    profile_enabled_unchecked, profile_hit_bytes_unchecked, profile_hit_unchecked,
};
use molt_cpython_abi::{NativeGcEdge, NativeGcEdgeKind};

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
    fn try_update<F>(
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
            self.0.try_update(set_order, fetch_order, update)
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

    #[cfg(test)]
    fn restore(&self, snapshot: GenerationStats) {
        self.collections
            .store(snapshot.collections, AtomicOrdering::Relaxed);
        self.collected
            .store(snapshot.collected, AtomicOrdering::Relaxed);
        self.uncollectable
            .store(snapshot.uncollectable, AtomicOrdering::Relaxed);
        self.scanned
            .store(snapshot.scanned, AtomicOrdering::Relaxed);
    }
}

#[cfg(test)]
pub(crate) struct GcRuntimeTestSnapshot {
    enabled: u64,
    pending: u64,
    debug_flags: u64,
    thresholds: [u64; NUM_GENERATIONS],
    counts: [u64; NUM_GENERATIONS],
    stats: [GenerationStats; NUM_GENERATIONS],
    long_lived_total: u64,
    long_lived_pending: u64,
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

    #[cfg(test)]
    pub(crate) fn runtime_test_snapshot(&self) -> GcRuntimeTestSnapshot {
        Self::assert_custody();
        GcRuntimeTestSnapshot {
            enabled: self.enabled.load(AtomicOrdering::Relaxed),
            pending: self.pending.load(AtomicOrdering::Relaxed),
            debug_flags: self.debug_flags.load(AtomicOrdering::Relaxed),
            thresholds: std::array::from_fn(|index| {
                self.thresholds[index].load(AtomicOrdering::Relaxed)
            }),
            counts: std::array::from_fn(|index| self.counts[index].load(AtomicOrdering::Relaxed)),
            stats: self.generation_stats(),
            long_lived_total: self.long_lived_total.load(AtomicOrdering::Relaxed),
            long_lived_pending: self.long_lived_pending.load(AtomicOrdering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn restore_runtime_test_snapshot(&self, snapshot: &GcRuntimeTestSnapshot) {
        Self::assert_custody();
        self.enabled
            .store(snapshot.enabled, AtomicOrdering::Relaxed);
        self.pending
            .store(snapshot.pending, AtomicOrdering::Relaxed);
        self.debug_flags
            .store(snapshot.debug_flags, AtomicOrdering::Relaxed);
        for index in 0..NUM_GENERATIONS {
            self.thresholds[index].store(snapshot.thresholds[index], AtomicOrdering::Relaxed);
            self.counts[index].store(snapshot.counts[index], AtomicOrdering::Relaxed);
            self.stats[index].restore(snapshot.stats[index]);
        }
        self.long_lived_total
            .store(snapshot.long_lived_total, AtomicOrdering::Relaxed);
        self.long_lived_pending
            .store(snapshot.long_lived_pending, AtomicOrdering::Relaxed);
    }

    pub(crate) fn on_allocation(&self) {
        Self::assert_custody();
        self.counts[0]
            .try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
                count.checked_add(1)
            })
            .unwrap_or_else(|_| std::process::abort());
        self.schedule_if_due();
    }

    pub(crate) fn on_deallocation(&self) {
        Self::assert_custody();
        let _ =
            self.counts[0].try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
                (count != 0).then(|| count - 1)
            });
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
                .try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
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
            word.try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |value| {
                value.checked_add(delta)
            })
            .unwrap_or_else(|_| std::process::abort());
        }
        if generation == NUM_GENERATIONS - 2 {
            self.long_lived_pending
                .try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |value| {
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

    pub(crate) fn clear_api_roots(&self, py: &PyToken<'_>) -> bool {
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
        let changed = callbacks != 0 || garbage != 0;
        for bits in [callbacks, garbage] {
            if bits != 0 {
                crate::dec_ref_bits(py, bits);
            }
        }
        changed
    }
}

pub(crate) fn gc_clear_api_roots(py: &PyToken<'_>) -> bool {
    crate::runtime_state(py).gc.clear_api_roots(py)
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
    native_nodes: HashMap<usize, NativeTrackedEntry>,
}

#[derive(Clone, Copy)]
struct TrackedEntry {
    allocation_id: u64,
    generation: u8,
}

/// Runtime-owned lifecycle metadata for a native CPython GC node.
///
/// Native memory and refcounts remain ABI-owned; membership,
/// generations, deterministic order, finalization state, and collector pins
/// live here beside Molt heap membership so there is exactly one collector and
/// one embedded-runtime teardown authority.
#[derive(Clone, Copy)]
struct NativeTrackedEntry {
    allocation_id: u64,
    generation: u8,
    tracked: bool,
    finalized: bool,
    pinned: bool,
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
                native_nodes: HashMap::new(),
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
        let _ =
            GC_TRACKED_LIVE.try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |live| {
                Some(live.saturating_sub(count))
            });
    }
}

/// `MOLT_TRACE_GC=1` enables collector tracing (candidate/unreachable/collected
/// counts) to stderr. Diagnostic-only; never part of observable program behavior.
fn gc_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_GC").as_deref() == Ok("1"))
}

/// Optional type-filtered reference-deduction ledger for cycle-collector
/// diagnostics. The environment is parsed once, so the collector's hot path is
/// a single predictable `Option` check when disabled. A numeric heap type id
/// reports each candidate's effective refcount, every internal-edge
/// subtraction that targets that type, and the reachable source that re-roots
/// a zero-trial-count candidate during propagation.
fn gc_trace_type_filter() -> Option<u32> {
    static TYPE_ID: OnceLock<Option<u32>> = OnceLock::new();
    *TYPE_ID.get_or_init(|| {
        std::env::var("MOLT_TRACE_GC_TYPE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|&type_id| super::is_valid_heap_type_id(type_id))
    })
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

/// The sole runtime-object membership insertion. Every admitted identity owns
/// the one-shot allocation claim, including explicit tracking and promotion.
/// Existing dictionary allocation claims are preserved without recounting.
#[derive(Clone, Copy, Eq, PartialEq)]
enum GcMembershipAdmission {
    Inserted,
    AlreadyTracked,
    ImmortalRoot,
}

/// Allocation accounting outlives changes to membership and owned payloads.
/// Constructors may enter more than once as a native class edge is initialized;
/// the existing header owns the one allocation claim, not the current projection.
#[inline]
unsafe fn gc_account_allocation(py: &PyToken<'_>, ptr: *mut u8) {
    let header = unsafe { &*header_from_obj_ptr(ptr) };
    if header.fetch_or_flags(HEADER_FLAG_GC_ACCOUNTED) & HEADER_FLAG_GC_ACCOUNTED == 0 {
        crate::runtime_state(py).gc.on_allocation();
    }
}

fn gc_admit_membership(py: &PyToken<'_>, ptr: *mut u8) -> GcMembershipAdmission {
    // Most allocations have no C view. Read an existing view's lifetime only
    // after that header fastpath, and release the bridge lock before touching
    // membership. Explicit C immortality is a process root; later dictionary
    // promotion must not undo SetImmortal's untracking.
    let immortal_root =
        unsafe { (*header_from_obj_ptr(ptr)).has_flag(super::HEADER_FLAG_HAS_ABI_VIEW) }
            && molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .is_immortal_c_view(MoltObject::from_ptr(ptr).bits());
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let admission = if let Entry::Vacant(entry) = shard.entries.entry(PtrSlot(ptr)) {
        if immortal_root {
            return GcMembershipAdmission::ImmortalRoot;
        }
        let allocation_id = tracked_registry()
            .next_allocation_id
            .try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |next| {
                next.checked_add(1)
            })
            .expect("GC allocation ordinal exhausted");
        entry.insert(TrackedEntry {
            allocation_id,
            generation: 0,
        });
        profile_gc_track();
        GcMembershipAdmission::Inserted
    } else {
        GcMembershipAdmission::AlreadyTracked
    };
    drop(shard);
    unsafe { gc_account_allocation(py, ptr) };
    admission
}

/// Honor an explicit C-API tracking request for an already allocated runtime
/// object. Dynamic dict/tuple demotion is a collector optimization, not a ban
/// on explicit enrollment. Admission establishes the one-shot allocation claim
/// and preserves an existing claim across untrack/retrack. Duplicate tracking
/// violates the public C API precondition.
pub(crate) unsafe fn gc_track_existing(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let projection = super::heap_track_projection(unsafe { object_type_id(ptr) });
    match projection {
        None | Some(super::HeapTrackProjection::Never) => return false,
        Some(super::HeapTrackProjection::NativeSubtype)
            if !unsafe { super::native_instance::has_fields(ptr) } =>
        {
            return false;
        }
        Some(super::HeapTrackProjection::ForeignDynamic)
            if !unsafe { super::heap_lifecycle::projected_track_state(py, ptr) } =>
        {
            return false;
        }
        _ => {}
    }
    matches!(
        gc_admit_membership(py, ptr),
        GcMembershipAdmission::Inserted | GcMembershipAdmission::ImmortalRoot
    )
}

/// Promote an exact dictionary without recounting its allocation. Unpublished
/// construction uses the same sticky tracking law; the publication bit still
/// prevents collector snapshots from observing an incomplete payload.
/// Mutations never demote: CPython 3.12/3.13 untrack dictionaries only during a
/// full collection; 3.14 keeps every dictionary tracked for its whole lifetime.
pub(crate) unsafe fn gc_track_dict(py: &PyToken<'_>, ptr: *mut u8) {
    gc_admit_membership(py, ptr);
}

/// Track only the newly published references, before releasing displaced edges.
/// This is independent of dictionary size and shares the collector's child law.
pub(crate) unsafe fn gc_track_dict_references(py: &PyToken<'_>, ptr: *mut u8, bits: &[u64]) {
    if !unsafe { gc_is_tracked(ptr) }
        && bits
            .iter()
            .copied()
            .any(super::heap_lifecycle::value_requires_tracking)
    {
        unsafe {
            gc_track_dict(py, ptr);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum GcNode {
    Runtime(PtrSlot),
    Native(usize),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum GcApiTarget {
    Node(GcNode),
    Inline(u64),
}

impl GcNode {
    #[inline]
    fn runtime_ptr(self) -> Option<*mut u8> {
        match self {
            Self::Runtime(ptr) => Some(ptr.0),
            Self::Native(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
struct GcCandidate {
    allocation_id: u64,
    node: GcNode,
}

unsafe fn reproject_reachable_containers(
    py: &PyToken<'_>,
    candidates: &[GcCandidate],
    marks: &[u8],
    generation: u8,
) {
    let untrack_dicts =
        generation == OLDEST_GENERATION && crate::object::ops_sys::runtime_target_minor(py) < 14;
    // Tuples must be demoted first, even when a referring dictionary was
    // allocated earlier. Both phases precede weakref callbacks and finalizers.
    for projection in [
        super::HeapTrackProjection::TupleDynamic,
        super::HeapTrackProjection::DictDynamic,
    ] {
        if projection == super::HeapTrackProjection::DictDynamic && !untrack_dicts {
            continue;
        }
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            // CPython runs deduce_unreachable first and untracks atomic tuples only
            // from the reachable generation list. An unreachable tuple remains a
            // candidate for this collection and contributes to gc.collect()'s count.
            if marks[candidate_index] != 2 {
                continue;
            }
            let Some(ptr) = candidate.node.runtime_ptr() else {
                continue;
            };
            let type_id = unsafe { object_type_id(ptr) };
            if super::heap_track_projection(type_id) != Some(projection)
                || unsafe { super::heap_lifecycle::projected_track_state(py, ptr) }
            {
                continue;
            }
            unsafe { gc_untrack(py, ptr, type_id, GcUntrackReason::DynamicProjection) };
        }
    }
}

/// Register a freshly-allocated object in the tracked set IFF it can form a cycle.
/// Called from the allocator for every heap object; GREEN types return immediately.
///
/// # Safety
/// `ptr` must be a live object pointer (data pointer, past the header).
#[inline]
pub(crate) unsafe fn gc_track_if_cyclic(py: &PyToken<'_>, ptr: *mut u8, type_id: u32) {
    let projection = super::heap_track_projection(type_id);
    if matches!(projection, None | Some(super::HeapTrackProjection::Never)) {
        return;
    }
    if projection == Some(super::HeapTrackProjection::NativeSubtype)
        && !unsafe { super::native_instance::has_fields(ptr) }
    {
        return;
    }
    if projection == Some(super::HeapTrackProjection::ForeignDynamic) {
        // Foreign payload identity is not valid until constructor publication.
        // Its generated dynamic projection performs the first registry insert,
        // so non-GC wrappers never touch membership storage.
        return;
    }
    if projection == Some(super::HeapTrackProjection::DictDynamic)
        && crate::object::ops_sys::runtime_target_minor(py) < 14
    {
        // Count the allocation once even while the empty dictionary is
        // untracked. Insertions promote it without recounting; deallocation
        // retires this allocation independently of current membership.
        unsafe { gc_account_allocation(py, ptr) };
        return;
    }
    gc_admit_membership(py, ptr);
}

/// Release-publish a completely initialized heap payload to the collector.
/// Constructors call this exactly once after every payload/class/sidecar edge is
/// valid. Registry membership may precede publication, but snapshots never expose
/// an unpublished entry.
#[inline]
pub(crate) unsafe fn gc_publish_initialized(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe { (*header_from_obj_ptr(ptr)).gc_publish_initialized() };
    if super::heap_track_projection(unsafe { object_type_id(ptr) })
        == Some(super::HeapTrackProjection::ForeignDynamic)
        && unsafe { super::heap_lifecycle::projected_track_state(py, ptr) }
    {
        gc_admit_membership(py, ptr);
    }
}

/// Remove an object from the tracked set as it is freed. Called from the
/// deallocator for every freed object. Unenrolled allocations return through the
/// header fast path; previously enrolled objects retire even after clear has
/// detached the native identity or class edge that originally admitted them.
///
/// # Safety
/// `ptr` identifies the object being freed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GcUntrackReason {
    Deallocation,
    DynamicProjection,
    ExplicitControl,
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
    let header = unsafe { &*header_from_obj_ptr(ptr) };
    if !header.has_flag(HEADER_FLAG_GC_ACCOUNTED) {
        return;
    }
    if reason == GcUntrackReason::Deallocation
        && header.fetch_and_flags(!HEADER_FLAG_GC_ACCOUNTED) & HEADER_FLAG_GC_ACCOUNTED == 0
    {
        return;
    }
    let projection = super::heap_track_projection(type_id);
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let removed = shard.entries.remove(&PtrSlot(ptr)).is_some();
    if removed {
        profile_gc_untrack(1);
    }
    drop(shard);
    // CPython's generation-0 counter is allocations minus deallocations of GC
    // objects, not current tracked-set membership. Dynamically projected exact
    // dicts/tuples still retire their original allocation here.
    if reason == GcUntrackReason::Deallocation
        && projection == Some(super::HeapTrackProjection::ForeignDynamic)
    {
        assert!(
            removed,
            "enrolled foreign wrapper lost dynamic GC membership before deallocation"
        );
    }
    if reason == GcUntrackReason::Deallocation {
        crate::runtime_state(py).gc.on_deallocation();
    }
}

/// The runtime header owns the one-shot finalizer state, independently of
/// current collector membership. Untracking/retracking never clears this bit.
pub(crate) unsafe fn gc_is_finalized(ptr: *mut u8) -> bool {
    unsafe { (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_FINALIZER_RAN) }
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

/// Admit one ABI-owned native allocation into the runtime's shared GC
/// lifecycle. Admission is separate from tracking, matching `PyObject_GC_New`
/// followed by `PyObject_GC_Track` and retaining finalized state across an
/// untrack/retrack transition.
pub(crate) fn native_gc_allocate(py: &PyToken<'_>, address: usize) -> bool {
    if address == 0 {
        return false;
    }
    let registry = tracked_registry();
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    if shard.native_nodes.contains_key(&address) {
        // Generic `tp_alloc` and a subtype's `tp_new` may both publish the same
        // live allocation. Identity admission is idempotent; address reuse is
        // impossible until the matching deallocate-before-free transition.
        return true;
    }
    let allocation_id = registry
        .next_allocation_id
        .try_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |next| {
            next.checked_add(1)
        })
        .expect("GC allocation ordinal exhausted");
    shard.native_nodes.insert(
        address,
        NativeTrackedEntry {
            allocation_id,
            generation: 0,
            tracked: false,
            finalized: false,
            pinned: false,
        },
    );
    drop(shard);
    crate::runtime_state(py).gc.on_allocation();
    true
}

/// Re-enroll a live native object unless its C header is a process-lifetime root.
///
/// # Safety
/// An enrolled `address` must identify a live, initialized `PyObject` header.
pub(crate) unsafe fn native_gc_track(address: usize) -> bool {
    if address == 0 {
        return false;
    }
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let Some(entry) = shard.native_nodes.get_mut(&address) else {
        return false;
    };
    if !entry.tracked {
        if molt_cpython_abi::abi_types::is_immortal_refcnt(unsafe {
            molt_cpython_abi::native_gc_node_refcount(address)
        }) {
            return true;
        }
        entry.tracked = true;
        entry.generation = 0;
        profile_gc_track();
    }
    true
}

pub(crate) fn native_gc_untrack(address: usize) {
    if address == 0 {
        return;
    }
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let Some(entry) = shard.native_nodes.get_mut(&address) else {
        return;
    };
    if entry.tracked {
        entry.tracked = false;
        profile_gc_untrack(1);
    }
}

pub(crate) fn native_gc_deallocate(py: &PyToken<'_>, address: usize) {
    if address == 0 {
        return;
    }
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let Some(entry) = shard.native_nodes.remove(&address) else {
        return;
    };
    assert!(
        !entry.pinned,
        "native GC node deallocated while collector-pinned"
    );
    if entry.tracked {
        profile_gc_untrack(1);
    }
    drop(shard);
    crate::runtime_state(py).gc.on_deallocation();
}

pub(crate) fn native_gc_is_enrolled(address: usize) -> bool {
    address != 0
        && lock_tracked_registry_shard(tracked_registry_shard_index_from_address(address))
            .native_nodes
            .contains_key(&address)
}

pub(crate) fn native_gc_is_tracked(address: usize) -> bool {
    address != 0
        && lock_tracked_registry_shard(tracked_registry_shard_index_from_address(address))
            .native_nodes
            .get(&address)
            .is_some_and(|entry| entry.tracked)
}

pub(crate) fn native_gc_is_finalized(address: usize) -> bool {
    address != 0
        && lock_tracked_registry_shard(tracked_registry_shard_index_from_address(address))
            .native_nodes
            .get(&address)
            .is_some_and(|entry| entry.finalized)
}

pub(crate) fn native_gc_claim_finalizer(address: usize) -> c_int {
    if address == 0 {
        return -1;
    }
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let Some(entry) = shard.native_nodes.get_mut(&address) else {
        return -1;
    };
    if entry.finalized {
        return 0;
    }
    entry.finalized = true;
    1
}

fn native_gc_set_pinned(address: usize, pinned: bool) {
    let shard_index = tracked_registry_shard_index_from_address(address);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let entry = shard
        .native_nodes
        .get_mut(&address)
        .expect("native GC node lost shared-registry membership during collection");
    assert_ne!(entry.pinned, pinned, "native GC pin transition repeated");
    entry.pinned = pinned;
}

fn native_gc_is_pinned(address: usize) -> bool {
    lock_tracked_registry_shard(tracked_registry_shard_index_from_address(address))
        .native_nodes
        .get(&address)
        .is_some_and(|entry| entry.pinned)
}

/// Consult the existing allocation authority before forced interpreter type
/// projection retirement. Native tp_clear/tp_dealloc can own C type references;
/// their allocations must drain before those identities become invalid.
pub(crate) fn gc_has_live_native_nodes() -> bool {
    crate::gil_assert();
    (0..TRACKED_REGISTRY_SHARDS)
        .any(|index| !lock_tracked_registry_shard(index).native_nodes.is_empty())
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
    let native_live = shards
        .iter()
        .map(|shard| shard.native_nodes.len())
        .sum::<usize>();
    assert_eq!(
        native_live, 0,
        "runtime teardown reached GC registry reset with {native_live} live native nodes; ABI-owned allocations must retire before embedded re-init"
    );
    let mut removed = 0u64;
    for shard in &mut shards {
        removed = removed.saturating_add(shard.entries.len() as u64);
        // Full embedded-runtime teardown must release peak registry capacity;
        // `HashMap::clear` would pin the largest prior heap in this OnceLock.
        shard.entries = HashMap::new();
        shard.native_nodes = HashMap::new();
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
            let runtime = shard
                .entries
                .values()
                .filter(|entry| selection.contains(entry.generation))
                .count();
            let native = shard
                .native_nodes
                .values()
                .filter(|entry| entry.tracked && selection.contains(entry.generation))
                .count();
            runtime + native
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
                node: GcNode::Runtime(*slot),
            })
        }));
        candidates.extend(shard.native_nodes.iter().filter_map(|(&address, entry)| {
            (entry.tracked && selection.contains(entry.generation)).then_some(GcCandidate {
                allocation_id: entry.allocation_id,
                node: GcNode::Native(address),
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
        for entry in shard.native_nodes.values_mut() {
            if entry.tracked && entry.generation < PERMANENT_GENERATION {
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
        for entry in shard.native_nodes.values_mut() {
            if entry.tracked && entry.generation == PERMANENT_GENERATION {
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
            let runtime = shard
                .entries
                .values()
                .filter(|entry| entry.generation == PERMANENT_GENERATION)
                .count();
            let native = shard
                .native_nodes
                .values()
                .filter(|entry| entry.tracked && entry.generation == PERMANENT_GENERATION)
                .count();
            runtime + native
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
        match candidate.node {
            GcNode::Runtime(ptr) => {
                let shard_index = tracked_registry_shard_index(ptr.0);
                let Some(entry) = shards[shard_index].entries.get_mut(&ptr) else {
                    continue;
                };
                entry.generation = target_generation;
                promoted += 1;
            }
            GcNode::Native(address) => {
                let shard_index = tracked_registry_shard_index_from_address(address);
                let Some(entry) = shards[shard_index].native_nodes.get_mut(&address) else {
                    continue;
                };
                if entry.tracked {
                    entry.generation = target_generation;
                    promoted += 1;
                }
            }
        }
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
#[cfg(test)]
pub(crate) unsafe fn molt_traverse(py: &PyToken<'_>, ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    unsafe { super::heap_lifecycle::visit_owned_edges(py, ptr, visit) }
}

struct NativeVisitContext<'a> {
    visit: &'a mut dyn FnMut(NativeGcEdge),
}

fn node_from_native_gc_edge(edge: NativeGcEdge) -> Option<GcNode> {
    if edge.kind == NativeGcEdgeKind::ManagedHandle as u8 {
        return crate::obj_from_bits(edge.value)
            .as_ptr()
            .map(|ptr| GcNode::Runtime(PtrSlot(ptr)));
    }
    if edge.kind == NativeGcEdgeKind::NativePointer as u8 {
        let Ok(address) = usize::try_from(edge.value) else {
            std::process::abort();
        };
        return (address != 0).then_some(GcNode::Native(address));
    }
    std::process::abort();
}

unsafe extern "C" fn native_gc_visit_edge(edge: NativeGcEdge, context: *mut c_void) -> c_int {
    let context = unsafe { &mut *context.cast::<NativeVisitContext<'_>>() };
    (context.visit)(edge);
    0
}

/// Keep the physical edge carrier until its consumer chooses collector-node
/// identity or public API value identity. Both consumers retain the same native
/// tp_traverse protocol, callback status handling, and edge multiplicity.
unsafe fn visit_native_owned_edges(address: usize, visit: &mut dyn FnMut(NativeGcEdge)) -> bool {
    let mut context = NativeVisitContext { visit };
    let result = unsafe {
        molt_cpython_abi::native_gc_node_visit(
            address,
            native_gc_visit_edge,
            std::ptr::from_mut(&mut context).cast::<c_void>(),
        )
    };
    result == 0
}

/// Visit the generalized shared-GC graph. Runtime owners use the generated
/// lifecycle authority; `TYPE_ID_FOREIGN` contributes its enrolled native
/// custody edge; native nodes delegate allocation-free traversal to
/// their ABI layout authority.
unsafe fn traverse_node(py: &PyToken<'_>, node: GcNode, visit: &mut dyn FnMut(GcNode)) -> bool {
    match node {
        GcNode::Runtime(ptr) => unsafe {
            let status = super::heap_lifecycle::visit_owned_gc_edges(py, ptr.0, &mut |edge| {
                if let Some(child) = node_from_native_gc_edge(edge) {
                    visit(child);
                }
            });
            if status != 0 {
                return false;
            }
            if object_type_id(ptr.0) == super::TYPE_ID_FOREIGN {
                let address = super::foreign::foreign_ptr_from_obj(ptr.0);
                if native_gc_is_enrolled(address) {
                    visit(GcNode::Native(address));
                }
            }
            true
        },
        GcNode::Native(address) => unsafe {
            visit_native_owned_edges(address, &mut |edge| {
                if let Some(child) = node_from_native_gc_edge(edge) {
                    visit(child);
                }
            })
        },
    }
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

/// Result of one collection. Python's count includes unreachable objects kept
/// by DEBUG_SAVEALL or a native clear callback; retirement separately measures
/// whether the original tracked allocation cohort actually made progress.
pub(crate) struct CollectStats {
    /// Original unreachable allocation identities no longer in the tracked
    /// cohort after releasing collector pins. Python's collected count also
    /// includes retained garbage and does not establish fixed-point progress.
    pub(crate) retired: usize,
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
    CallbackError(&'static str),
    UnsupportedConcurrency,
}

impl CollectStats {
    fn completed(collected: usize, scanned: usize, survivors: usize) -> Self {
        #[cfg(not(test))]
        let _ = (scanned, survivors);
        Self {
            retired: 0,
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
            GcCollectStatus::CallbackError(_) => 4,
        };
        crate::runtime_state(py)
            .gc_last_failure
            .store(code, AtomicOrdering::Release);
        Self {
            retired: 0,
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
    // Collector pins are physical lifetime owners but not reachability roots.
    // This scratch projection is the only place the runtime discounts a pin;
    // header/ABI-view retain and release keep its full ownership contribution.
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

unsafe fn effective_node_refcount(node: GcNode) -> i64 {
    match node {
        GcNode::Runtime(ptr) => unsafe { effective_gc_refcount(ptr.0) },
        GcNode::Native(address) => {
            let raw = unsafe { molt_cpython_abi::native_gc_node_refcount(address) };
            if raw <= 0 {
                std::process::abort();
            }
            let mut effective = i64::try_from(raw).unwrap_or_else(|_| std::process::abort());
            let mirrors =
                i64::try_from(molt_cpython_abi::bridge::GLOBAL_BRIDGE.mirrored_c_refcount(address))
                    .unwrap_or_else(|_| std::process::abort());
            if mirrors > effective {
                eprintln!(
                    "molt fatal: native GC mirror references exceed C ownership: address={address:#x} refs={raw} mirrors={mirrors}"
                );
                std::process::abort();
            }
            // Private clean projections mirror the runtime graph even when
            // their target is native. Ordinary physical C edges remain counted.
            effective -= mirrors;
            if native_gc_is_pinned(address) {
                effective -= 1;
            }
            effective
        }
    }
}

unsafe fn pin_node(node: GcNode) {
    match node {
        GcNode::Runtime(ptr) => {
            let header = unsafe { header_from_obj_ptr(ptr.0) };
            unsafe { (*header).pin_for_gc(MoltObject::from_ptr(ptr.0).bits()) };
        }
        GcNode::Native(address) => {
            unsafe { molt_cpython_abi::native_gc_node_incref(address) };
            native_gc_set_pinned(address, true);
        }
    }
}

unsafe fn release_node_pin(py: &PyToken<'_>, node: GcNode) {
    match node {
        GcNode::Runtime(ptr) => {
            let header = unsafe { header_from_obj_ptr(ptr.0) };
            let flags = unsafe { (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED) };
            if flags & HEADER_FLAG_GC_PINNED == 0 {
                eprintln!("molt fatal: cycle collector released an unowned pin");
                std::process::abort();
            }
            // Retire the physical owner through the same bridge/finalization
            // transaction as every other reference, including ABI-view objects.
            unsafe { dec_ref_ptr(py, ptr.0) };
        }
        GcNode::Native(address) => {
            native_gc_set_pinned(address, false);
            unsafe { molt_cpython_abi::native_gc_node_decref(address) };
        }
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
        let Some(ptr) = candidates[index].node.runtime_ptr() else {
            continue;
        };
        unsafe {
            edges = edges
                .checked_add(super::heap_lifecycle::detached_managed_edge_count(py, ptr))
                .unwrap_or_else(|| std::process::abort());
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
unsafe fn node_set_collecting(node: GcNode, on: bool) {
    if let Some(ptr) = node.runtime_ptr() {
        unsafe { header_set_collecting(ptr, on) };
    }
}

unsafe fn clear_node(
    py: &PyToken<'_>,
    node: GcNode,
    detached: &mut super::heap_lifecycle::DetachedEdgeSink,
) -> bool {
    match node {
        GcNode::Runtime(ptr) => {
            // Public tp_clear owns callback errors. Collection reports them at
            // this boundary and restores the caller's two error channels.
            molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
                let status =
                    super::heap_lifecycle::try_clear_cycle_edges_with_sink(py, ptr.0, detached);
                if status != 0 {
                    let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .handle_to_borrowed_pyobj(MoltObject::from_ptr(ptr.0).bits());
                    molt_cpython_abi::api::errors::PyErr_WriteUnraisable(view);
                }
                status == 0
            })
        }
        GcNode::Native(address) => {
            match unsafe { molt_cpython_abi::native_gc_node_clear(address) } {
                0 | 1 => true,
                -1 => false,
                _ => std::process::abort(),
            }
        }
    }
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
    index: HashMap<GcNode, usize>,
    refs: Vec<i64>,
    marks: Vec<u8>,
    queue: Vec<usize>,
    first_unreachable: Vec<usize>,
    first_unreachable_runtime_ptrs: Vec<PtrSlot>,
    final_unreachable: Vec<usize>,
    api_values: Vec<u64>,
    api_targets: Vec<u64>,
    api_value_membership: HashSet<u64>,
    api_target_membership: HashSet<GcApiTarget>,
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
            candidate_pins: CandidatePins::None,
        }
    }

    fn try_prepare_candidates(&mut self) -> bool {
        let len = self.candidates.len();
        self.index.clear();
        self.refs.clear();
        self.marks.clear();
        self.queue.clear();
        self.first_unreachable.clear();
        self.first_unreachable_runtime_ptrs.clear();
        self.final_unreachable.clear();
        self.api_values.clear();
        self.api_targets.clear();
        self.api_value_membership.clear();
        self.api_target_membership.clear();
        if self.index.try_reserve(len).is_err()
            || !try_reserve_total(&mut self.refs, len)
            || !try_reserve_total(&mut self.marks, len)
            || !try_reserve_total(&mut self.queue, len)
            || !try_reserve_total(&mut self.first_unreachable, len)
            || !try_reserve_total(&mut self.first_unreachable_runtime_ptrs, len)
            || !try_reserve_total(&mut self.final_unreachable, len)
        {
            return false;
        }
        for (candidate_index, candidate) in self.candidates.iter().enumerate() {
            self.index.insert(candidate.node, candidate_index);
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
        self.first_unreachable_runtime_ptrs.clear();
        self.final_unreachable.clear();
        self.api_values.clear();
        self.api_targets.clear();
        self.api_value_membership.clear();
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum CandidatePins {
    None,
    Collector,
    Api,
}

struct GcScratchLease {
    scratch: Option<GcScratch>,
    candidate_pins: CandidatePins,
}

impl GcScratchLease {
    /// Raw snapshot identities must remain live across arbitrary extension
    /// callbacks. Collector pins use the existing discounted lifetime owner;
    /// nested API snapshots use ordinary holds, which are real temporary roots.
    unsafe fn pin_candidates(&mut self, py: &PyToken<'_>, kind: CandidatePins) {
        assert!(self.candidate_pins == CandidatePins::None);
        assert!(kind != CandidatePins::None);
        for candidate in &self.candidates {
            unsafe {
                match (kind, candidate.node) {
                    (CandidatePins::Collector, node) => pin_node(node),
                    (CandidatePins::Api, GcNode::Runtime(ptr)) => {
                        crate::inc_ref_bits(py, MoltObject::from_ptr(ptr.0).bits())
                    }
                    (CandidatePins::Api, GcNode::Native(address)) => {
                        molt_cpython_abi::native_gc_node_incref(address)
                    }
                    (CandidatePins::None, _) => unreachable!(),
                }
            }
        }
        self.candidate_pins = kind;
    }

    fn release_candidate_pins(&mut self, py: &PyToken<'_>) {
        let kind = std::mem::replace(&mut self.candidate_pins, CandidatePins::None);
        if kind == CandidatePins::None {
            return;
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            for candidate in &self.candidates {
                unsafe {
                    match (kind, candidate.node) {
                        (CandidatePins::Collector, node) => release_node_pin(py, node),
                        (CandidatePins::Api, GcNode::Runtime(ptr)) => {
                            crate::dec_ref_bits(py, MoltObject::from_ptr(ptr.0).bits())
                        }
                        (CandidatePins::Api, GcNode::Native(address)) => {
                            molt_cpython_abi::native_gc_node_decref(address)
                        }
                        (CandidatePins::None, _) => unreachable!(),
                    }
                }
            }
        });
    }
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
        if self.candidate_pins != CandidatePins::None {
            crate::concurrency::gil::with_gil(|py| self.release_candidate_pins(&py));
        }
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
) -> Result<(), GcIntrospectionError> {
    if !snapshot_registry(&mut scratch.candidates, selection) {
        return Err(GcIntrospectionError::Resource(
            "tracked-registry snapshot allocation failed",
        ));
    }
    scratch.api_values.clear();
    if !try_reserve_total(&mut scratch.api_values, scratch.candidates.len()) {
        return Err(GcIntrospectionError::Resource(
            "GC introspection result allocation failed",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GcIntrospectionError {
    Resource(&'static str),
    Callback(&'static str),
    UnsupportedConcurrency,
}

#[inline]
fn require_pointer_snapshot_epoch() -> Result<(), GcIntrospectionError> {
    if cfg!(feature = "free-threaded") {
        Err(GcIntrospectionError::UnsupportedConcurrency)
    } else {
        Ok(())
    }
}

/// Project one shared-GC node into Molt's public object representation. Native
/// nodes use the canonical bridge identity and return an owned temporary that
/// the caller must release after transferring it into a runtime container.
unsafe fn node_api_value(_py: &PyToken<'_>, node: GcNode) -> Option<(u64, bool)> {
    match node {
        GcNode::Runtime(ptr) => Some((MoltObject::from_ptr(ptr.0).bits(), false)),
        GcNode::Native(address) => unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(std::ptr::with_exposed_provenance_mut::<
                    molt_cpython_abi::abi_types::PyObject,
                >(address))
                .map(|bits| (bits, true))
        },
    }
}

#[inline]
unsafe fn api_target_from_bits(bits: u64) -> GcApiTarget {
    let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
        return GcApiTarget::Inline(bits);
    };
    if unsafe { object_type_id(ptr) } == super::TYPE_ID_FOREIGN {
        let address = unsafe { super::foreign::foreign_ptr_from_obj(ptr) };
        return GcApiTarget::Node(GcNode::Native(address));
    }
    GcApiTarget::Node(GcNode::Runtime(PtrSlot(ptr)))
}

/// A native physical pointer can already denote an inline numeric value,
/// singleton, or registered runtime binding. Public referent/referrer identity
/// uses that canonical value without observing mutable state or allocating a
/// foreign wrapper merely to compare it. Unbound native objects keep their
/// native identity; collector classification remains unchanged.
unsafe fn api_target_from_native_gc_edge(edge: NativeGcEdge) -> Option<GcApiTarget> {
    if edge.kind == NativeGcEdgeKind::ManagedHandle as u8 {
        return Some(unsafe { api_target_from_bits(edge.value) });
    }
    let node = node_from_native_gc_edge(edge)?;
    let GcNode::Native(address) = node else {
        std::process::abort();
    };
    let pointer =
        std::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(address);
    Some(
        match molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_handle_for_pyobj(pointer) {
            Some(value) => unsafe { api_target_from_bits(value.bits()) },
            None => GcApiTarget::Node(node),
        },
    )
}

/// Introspection retains inline payload values and uses the same physical
/// mixed-edge authority as collection. Foreign wrappers expose their native
/// object's referents rather than the wrapper's private custody edge.
unsafe fn visit_api_referents(
    py: &PyToken<'_>,
    node: GcNode,
    visit: &mut dyn FnMut(GcApiTarget),
) -> bool {
    let node = match node {
        GcNode::Runtime(ptr) => unsafe {
            if object_type_id(ptr.0) == super::TYPE_ID_FOREIGN {
                let address = super::foreign::foreign_ptr_from_obj(ptr.0);
                if native_gc_is_enrolled(address) {
                    GcNode::Native(address)
                } else {
                    node
                }
            } else {
                node
            }
        },
        GcNode::Native(_) => node,
    };
    unsafe {
        match node {
            GcNode::Runtime(ptr) => {
                let status = super::heap_lifecycle::visit_owned_gc_edges(py, ptr.0, &mut |edge| {
                    if let Some(target) = api_target_from_native_gc_edge(edge) {
                        visit(target);
                    }
                });
                status == 0
            }
            GcNode::Native(address) => visit_native_owned_edges(address, &mut |edge| {
                if let Some(target) = api_target_from_native_gc_edge(edge) {
                    visit(target);
                }
            }),
        }
    }
}

/// Build `gc.get_objects()` from the same deterministic registry snapshot used
/// by collection. `None` excludes the permanent generation; `Some(g)` selects
/// exactly one ordinary generation, matching CPython's public contract.
pub(crate) fn get_objects(
    py: &PyToken<'_>,
    generation: Option<u8>,
) -> Result<*mut u8, GcIntrospectionError> {
    require_pointer_snapshot_epoch()?;
    let selection = generation.map_or(RegistrySelection::Ordinary, RegistrySelection::Exact);
    let mut scratch = GcScratch::acquire();
    snapshot_api_values(&mut scratch, selection)?;
    unsafe { scratch.pin_candidates(py, CandidatePins::Api) };
    let GcScratch {
        candidates,
        api_values,
        api_targets,
        api_value_membership,
        ..
    } = &mut *scratch;
    if !try_reserve_total(api_targets, candidates.len())
        || api_value_membership.try_reserve(candidates.len()).is_err()
    {
        return Err(GcIntrospectionError::Resource(
            "GC native introspection ownership allocation failed",
        ));
    }
    for candidate in candidates.iter() {
        let Some((bits, owned)) = (unsafe { node_api_value(py, candidate.node) }) else {
            for bits in api_targets.drain(..) {
                crate::dec_ref_bits(py, bits);
            }
            return Err(GcIntrospectionError::Resource(
                "GC native introspection projection failed",
            ));
        };
        if !api_value_membership.insert(bits) {
            if owned {
                crate::dec_ref_bits(py, bits);
            }
            continue;
        }
        api_values.push(bits);
        if owned {
            api_targets.push(bits);
        }
    }
    let result = crate::alloc_list(py, &scratch.api_values);
    for bits in scratch.api_targets.drain(..) {
        crate::dec_ref_bits(py, bits);
    }
    (!result.is_null())
        .then_some(result)
        .ok_or(GcIntrospectionError::Resource(
            "GC introspection result allocation failed",
        ))
}

/// Build `gc.get_referents(*objects)` from the exhaustive lifecycle value
/// authority. Unlike cycle traversal, this intentionally retains inline values.
pub(crate) unsafe fn get_referents(
    py: &PyToken<'_>,
    objects_ptr: *mut u8,
) -> Result<*mut u8, GcIntrospectionError> {
    require_pointer_snapshot_epoch()?;
    let objects = unsafe {
        super::seq_access::snapshot(py, objects_ptr, "GC introspection argument snapshot failed")
    }
    .ok_or(GcIntrospectionError::Resource(
        "GC introspection argument snapshot failed",
    ))?;
    let mut scratch = GcScratch::acquire();
    scratch.api_values.clear();
    scratch.api_targets.clear();
    let mut projection_failed = false;
    let mut callback_failed = false;
    // Traverse once outside argument-storage locks. Each yielded referent gets
    // its own temporary owner before m_traverse may continue and clear state.
    for &bits in objects.iter() {
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            continue;
        };
        let traversed = unsafe {
            visit_api_referents(py, GcNode::Runtime(PtrSlot(ptr)), &mut |child| {
                if projection_failed {
                    return;
                }
                if scratch.api_values.try_reserve(1).is_err()
                    || scratch.api_targets.try_reserve(1).is_err()
                {
                    projection_failed = true;
                    return;
                }
                let projected = match child {
                    GcApiTarget::Inline(bits) => Some((bits, false)),
                    GcApiTarget::Node(node) => node_api_value(py, node),
                };
                let Some((child_bits, owned)) = projected else {
                    projection_failed = true;
                    return;
                };
                if !owned {
                    crate::inc_ref_bits(py, child_bits);
                }
                scratch.api_values.push(child_bits);
                scratch.api_targets.push(child_bits);
            })
        };
        callback_failed |= !traversed;
        if projection_failed || callback_failed {
            break;
        }
    }
    let result = if callback_failed {
        Err(GcIntrospectionError::Callback(
            "GC referent traversal failed",
        ))
    } else if projection_failed {
        Err(GcIntrospectionError::Resource(
            "GC referent projection failed",
        ))
    } else {
        let result = crate::alloc_list(py, &scratch.api_values);
        (!result.is_null())
            .then_some(result)
            .ok_or(GcIntrospectionError::Resource(
                "GC introspection result allocation failed",
            ))
    };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        for bits in scratch.api_targets.drain(..) {
            crate::dec_ref_bits(py, bits);
        }
        drop(objects);
    });
    result
}

/// Build `gc.get_referrers(*objects)` by scanning every tracked generation,
/// including the frozen permanent generation, through the same lifecycle value
/// authority. Each referring container appears once regardless of edge count.
pub(crate) unsafe fn get_referrers(
    py: &PyToken<'_>,
    objects_ptr: *mut u8,
) -> Result<*mut u8, GcIntrospectionError> {
    require_pointer_snapshot_epoch()?;
    let mut scratch = GcScratch::acquire();
    snapshot_api_values(&mut scratch, RegistrySelection::All)?;
    unsafe { scratch.pin_candidates(py, CandidatePins::Api) };
    scratch.api_target_membership.clear();
    let target_count = unsafe { super::seq_access::with_borrowed(objects_ptr, <[u64]>::len) };
    if scratch.api_target_membership.capacity() < target_count
        && scratch
            .api_target_membership
            .try_reserve(target_count)
            .is_err()
    {
        return Err(GcIntrospectionError::Resource(
            "GC introspection target allocation failed",
        ));
    }
    unsafe {
        super::seq_access::with_borrowed(objects_ptr, |values| {
            scratch.api_target_membership.extend(
                values
                    .iter()
                    .copied()
                    .map(|bits| api_target_from_bits(bits)),
            )
        });
    }
    if scratch.api_target_membership.is_empty() {
        return Ok(crate::alloc_list(py, &[]));
    }
    let args_identity = objects_ptr.expose_provenance();
    let GcScratch {
        candidates,
        api_values,
        api_targets,
        api_value_membership,
        api_target_membership,
        ..
    } = &mut *scratch;
    if !try_reserve_total(api_targets, candidates.len())
        || api_value_membership.try_reserve(candidates.len()).is_err()
    {
        return Err(GcIntrospectionError::Resource(
            "GC native referrer ownership allocation failed",
        ));
    }
    for candidate in candidates.iter() {
        let mut refers = false;
        if let GcNode::Runtime(ptr) = candidate.node
            && ptr.0.expose_provenance() == args_identity
        {
            continue;
        }
        let traversed = unsafe {
            visit_api_referents(py, candidate.node, &mut |child| {
                refers |= api_target_membership.contains(&child);
            })
        };
        if !traversed {
            for bits in api_targets.drain(..) {
                crate::dec_ref_bits(py, bits);
            }
            return Err(GcIntrospectionError::Callback(
                "GC referrer traversal failed",
            ));
        }
        if refers {
            let Some((bits, owned)) = (unsafe { node_api_value(py, candidate.node) }) else {
                for bits in api_targets.drain(..) {
                    crate::dec_ref_bits(py, bits);
                }
                return Err(GcIntrospectionError::Resource(
                    "GC native referrer projection failed",
                ));
            };
            if !api_value_membership.insert(bits) {
                if owned {
                    crate::dec_ref_bits(py, bits);
                }
                continue;
            }
            api_values.push(bits);
            if owned {
                api_targets.push(bits);
            }
        }
    }
    let result = crate::alloc_list(py, &scratch.api_values);
    for bits in scratch.api_targets.drain(..) {
        crate::dec_ref_bits(py, bits);
    }
    (!result.is_null())
        .then_some(result)
        .ok_or(GcIntrospectionError::Resource(
            "GC introspection result allocation failed",
        ))
}

#[inline]
fn scratch_push(storage: &mut Vec<usize>, value: usize) {
    if storage.len() >= storage.capacity() {
        std::process::abort();
    }
    storage.push(value);
}

struct GcDeductionWorkspace<'a> {
    candidates: &'a [GcCandidate],
    index: &'a HashMap<GcNode, usize>,
    refs: &'a mut [i64],
    marks: &'a mut [u8],
    queue: &'a mut Vec<usize>,
}

unsafe fn deduce_subset(
    py: &PyToken<'_>,
    workspace: &mut GcDeductionWorkspace<'_>,
    subset: Option<&[usize]>,
    output: &mut Vec<usize>,
) -> bool {
    let candidates = workspace.candidates;
    let index = workspace.index;
    let refs = &mut *workspace.refs;
    let marks = &mut *workspace.marks;
    let queue = &mut *workspace.queue;
    marks.fill(0);
    queue.clear();
    output.clear();

    let mut initialize = |candidate_index: usize| {
        let node = candidates[candidate_index].node;
        refs[candidate_index] = unsafe { effective_node_refcount(node) };
        marks[candidate_index] = 1;
        if let Some(ptr) = node.runtime_ptr() {
            if gc_trace_type_filter()
                .is_some_and(|type_id| unsafe { object_type_id(ptr) } == type_id)
            {
                eprintln!(
                    "molt gc ref init: index={candidate_index} ptr=0x{:x} refs={}",
                    ptr as usize, refs[candidate_index]
                );
            }
            unsafe { header_set_collecting(ptr, true) };
        }
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
        traverse_node(py, candidates[candidate_index].node, &mut |child| {
            if let Some(&child_index) = index.get(&child)
                && marks[child_index] == 1
            {
                let before = refs[child_index];
                refs[child_index] -= 1;
                if child.runtime_ptr().is_some_and(|ptr| {
                    gc_trace_type_filter().is_some_and(|type_id| object_type_id(ptr) == type_id)
                }) {
                    eprintln!(
                        "molt gc ref subtract: source={:?} child={:?} refs={}→{}",
                        candidates[candidate_index].node, child, before, refs[child_index]
                    );
                }
            }
        })
    };
    if let Some(subset) = subset {
        for &candidate_index in subset {
            if !subtract(candidate_index) {
                return false;
            }
        }
    } else {
        for candidate_index in 0..candidates.len() {
            if !subtract(candidate_index) {
                return false;
            }
        }
    }

    let mut seed = |candidate_index: usize| {
        if candidates[candidate_index]
            .node
            .runtime_ptr()
            .is_some_and(|ptr| {
                gc_trace_type_filter()
                    .is_some_and(|type_id| unsafe { object_type_id(ptr) } == type_id)
            })
        {
            eprintln!(
                "molt gc ref final: index={candidate_index} node={:?} refs={}",
                candidates[candidate_index].node, refs[candidate_index]
            );
        }
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
            if !traverse_node(py, candidates[candidate_index].node, &mut |child| {
                if let Some(&child_index) = index.get(&child)
                    && marks[child_index] == 1
                {
                    if child.runtime_ptr().is_some_and(|ptr| {
                        gc_trace_type_filter().is_some_and(|type_id| object_type_id(ptr) == type_id)
                    }) {
                        eprintln!(
                            "molt gc ref reroot: source={:?} source_refs={} child={:?} child_refs={}",
                            candidates[candidate_index].node,
                            refs[candidate_index],
                            child,
                            refs[child_index]
                        );
                    }
                    marks[child_index] = 2;
                    scratch_push(queue, child_index);
                }
            }) {
                return false;
            }
        }
    }

    let mut partition = |candidate_index: usize| {
        if marks[candidate_index] == 2 {
            if let Some(ptr) = candidates[candidate_index].node.runtime_ptr() {
                unsafe { header_set_collecting(ptr, false) };
            }
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
    true
}

unsafe fn deduce_all(py: &PyToken<'_>, scratch: &mut GcScratch) -> bool {
    let GcScratch {
        candidates,
        index,
        refs,
        marks,
        queue,
        first_unreachable,
        first_unreachable_runtime_ptrs,
        ..
    } = scratch;
    let mut workspace = GcDeductionWorkspace {
        candidates,
        index,
        refs,
        marks,
        queue,
    };
    if !unsafe { deduce_subset(py, &mut workspace, None, first_unreachable) } {
        return false;
    }
    first_unreachable_runtime_ptrs.clear();
    for &candidate_index in first_unreachable.iter() {
        if let Some(ptr) = candidates[candidate_index].node.runtime_ptr() {
            if first_unreachable_runtime_ptrs.len() >= first_unreachable_runtime_ptrs.capacity() {
                std::process::abort();
            }
            first_unreachable_runtime_ptrs.push(PtrSlot(ptr));
        }
    }
    true
}

unsafe fn deduce_after_finalizers(py: &PyToken<'_>, scratch: &mut GcScratch) -> bool {
    let GcScratch {
        candidates,
        index,
        refs,
        marks,
        queue,
        first_unreachable,
        first_unreachable_runtime_ptrs: _,
        final_unreachable,
        ..
    } = scratch;
    let mut workspace = GcDeductionWorkspace {
        candidates,
        index,
        refs,
        marks,
        queue,
    };
    unsafe {
        deduce_subset(
            py,
            &mut workspace,
            Some(first_unreachable.as_slice()),
            final_unreachable,
        )
    }
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

/// Observe opaque allocation generations, never dereference a released pin.
/// A native tp_clear may succeed without removing its cycle. Its public GC
/// count is still positive, but the unchanged cohort supplies no progress.
fn retired_unreachable_count(scratch: &GcScratch) -> usize {
    if scratch.first_unreachable.is_empty() {
        return 0;
    }
    let shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    scratch
        .first_unreachable
        .iter()
        .filter(|&&index| {
            let candidate = scratch.candidates[index];
            let current = match candidate.node {
                GcNode::Runtime(ptr) => shards[tracked_registry_shard_index(ptr.0)]
                    .entries
                    .get(&ptr)
                    .map(|entry| entry.allocation_id),
                GcNode::Native(address) => shards
                    [tracked_registry_shard_index_from_address(address)]
                .native_nodes
                .get(&address)
                .filter(|entry| entry.tracked)
                .map(|entry| entry.allocation_id),
            };
            current != Some(candidate.allocation_id)
        })
        .count()
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
        let mut outcome = (|| {
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

            // Pin the entire snapshot before m_traverse can reenter and remove
            // any original root. Keep these owners until every raw candidate
            // identity has left the deduction/clear workspace; only effective
            // GC refcounts discount them, through the existing pin flag.
            scratch.pin_candidates(py, CandidatePins::Collector);

            // STEP 1-3: trial-deletion partition using one preallocated index/mark arena.
            if !deduce_all(py, &mut scratch) {
                for candidate in &scratch.candidates {
                    node_set_collecting(candidate.node, false);
                }
                return CollectStats::failure(
                    py,
                    GcCollectStatus::CallbackError("GC traversal callback failed"),
                );
            }
            reproject_reachable_containers(py, &scratch.candidates, &scratch.marks, generation);
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
                for &candidate_index in &scratch.first_unreachable {
                    node_set_collecting(scratch.candidates[candidate_index].node, false);
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

            crate::object::weakref::weakref_handle_cycle_unreachable(
                py,
                &scratch.first_unreachable_runtime_ptrs,
                |wr_ptr| header_is_collecting(wr_ptr),
            );

            for &candidate_index in &scratch.first_unreachable {
                run_node_finalizer_once(py, scratch.candidates[candidate_index].node);
            }

            // Reuse the exact same index, refs, mark, queue, and output storage.
            // No allocation is permitted in the post-callback resurrection partition.
            if !deduce_after_finalizers(py, &mut scratch) {
                for &index in &scratch.first_unreachable {
                    node_set_collecting(scratch.candidates[index].node, false);
                }
                return CollectStats::failure(
                    py,
                    GcCollectStatus::CallbackError("post-finalizer GC traversal callback failed"),
                );
            }
            survivors += promote_marked_candidates(
                &scratch.candidates,
                &scratch.marks,
                2,
                target_generation,
            );
            if scratch.final_unreachable.is_empty() {
                return completed_collection(py, generation, 0, scanned, survivors);
            }

            let collected = scratch.final_unreachable.len();
            if gc_trace_enabled() || debug_flags & DEBUG_STATS != 0 {
                eprintln!("molt gc: delete_garbage collected={collected}");
            }

            if debug_flags & DEBUG_COLLECTABLE != 0 {
                for &candidate_index in &scratch.final_unreachable {
                    match scratch.candidates[candidate_index].node {
                        GcNode::Runtime(ptr) => {
                            let object = MoltObject::from_ptr(ptr.0);
                            eprintln!("gc: collectable <{}>", crate::type_name(py, object));
                        }
                        GcNode::Native(address) => {
                            eprintln!("gc: collectable <native 0x{address:x}>");
                        }
                    }
                }
            }

            if debug_flags & DEBUG_SAVEALL != 0 {
                let garbage_bits = crate::runtime_state(py)
                    .gc
                    .existing_api_root_bits(py, false);
                if garbage_bits == 0 {
                    for &candidate_index in &scratch.final_unreachable {
                        node_set_collecting(scratch.candidates[candidate_index].node, false);
                    }
                    return CollectStats::failure(
                        py,
                        GcCollectStatus::ResourceError(
                            "gc.garbage is unavailable for DEBUG_SAVEALL",
                        ),
                    );
                }
                let mut appended_all = true;
                let final_count = scratch.final_unreachable.len();
                scratch.api_value_membership.clear();
                if scratch
                    .api_value_membership
                    .try_reserve(final_count)
                    .is_err()
                {
                    crate::dec_ref_bits(py, garbage_bits);
                    for &candidate_index in &scratch.final_unreachable {
                        node_set_collecting(scratch.candidates[candidate_index].node, false);
                    }
                    return CollectStats::failure(
                        py,
                        GcCollectStatus::ResourceError("gc.garbage identity allocation failed"),
                    );
                }
                let GcScratch {
                    candidates,
                    final_unreachable,
                    api_value_membership,
                    ..
                } = &mut *scratch;
                for &candidate_index in final_unreachable.iter() {
                    let Some((bits, owned)) = node_api_value(py, candidates[candidate_index].node)
                    else {
                        appended_all = false;
                        continue;
                    };
                    if !api_value_membership.insert(bits) {
                        if owned {
                            crate::dec_ref_bits(py, bits);
                        }
                        continue;
                    }
                    appended_all &= crate::object::ops_list::molt_list_append_with_projection(
                        garbage_bits,
                        bits,
                        std::ptr::null_mut(),
                    );
                    if owned {
                        crate::dec_ref_bits(py, bits);
                    }
                }
                crate::dec_ref_bits(py, garbage_bits);
                for &candidate_index in &scratch.final_unreachable {
                    node_set_collecting(scratch.candidates[candidate_index].node, false);
                }
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
                    node_set_collecting(scratch.candidates[candidate_index].node, false);
                }
                profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
                return CollectStats::failure(
                    py,
                    GcCollectStatus::ResourceError(
                        "post-finalizer detached-edge reservation failed",
                    ),
                );
            }

            for &candidate_index in &scratch.final_unreachable {
                node_set_collecting(scratch.candidates[candidate_index].node, false);
            }
            let mut cleared_all = true;
            for &candidate_index in &scratch.final_unreachable {
                cleared_all &=
                    clear_node(py, scratch.candidates[candidate_index].node, &mut detached);
            }
            molt_cpython_abi::api::errors::with_preserved_error(|| detached.release_all(py));

            if !cleared_all {
                return CollectStats::failure(
                    py,
                    GcCollectStatus::CallbackError("GC clear failed"),
                );
            }

            crate::runtime_state(py)
                .gc_last_failure
                .store(0, AtomicOrdering::Release);
            completed_collection(py, generation, collected, scanned, survivors)
        })();
        scratch.release_candidate_pins(py);
        if outcome.status == GcCollectStatus::Completed {
            outcome.retired = retired_unreachable_count(&scratch);
            if gc_trace_enabled() {
                eprintln!(
                    "molt gc: completed collected={} retired={}",
                    outcome.collected, outcome.retired
                );
            }
        }
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
    let outcome = molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
        let outcome = collect_generation(py, generation);
        if matches!(outcome.status, GcCollectStatus::CallbackError(_)) {
            molt_cpython_abi::api::errors::PyErr_WriteUnraisable(std::ptr::null_mut());
        }
        outcome
    });
    if matches!(
        outcome.status,
        GcCollectStatus::ReentrantNoop
            | GcCollectStatus::ResourceError(_)
            | GcCollectStatus::CallbackError(_)
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
        if !object_has_finalizer(py, ptr) {
            return;
        }
        if (*header).has_flag(HEADER_FLAG_FINALIZER_RAN) {
            return;
        }
        crate::object::maybe_run_object_finalizer_for_cycle(py, ptr);
    }
}

unsafe fn run_node_finalizer_once(py: &PyToken<'_>, node: GcNode) {
    match node {
        GcNode::Runtime(ptr) => unsafe { run_finalizer_once(py, ptr.0) },
        GcNode::Native(address) => {
            match unsafe { molt_cpython_abi::native_gc_node_finalize(address) } {
                0 | 1 => {}
                -1 => std::process::abort(),
                _ => std::process::abort(),
            }
        }
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
    use crate::{DEALLOC_COUNT, dict_set_in_place, exception_pending, obj_from_bits};
    use molt_cpython_abi::abi_types::{PyLong_Type, PyObject};
    use std::sync::atomic::Ordering;

    #[test]
    fn native_nodes_share_registry_order_and_retain_lifecycle_metadata() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let mut first_object = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut PyLong_Type,
            };
            let mut second_object = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut PyLong_Type,
            };
            let first = std::ptr::from_mut(&mut first_object).expose_provenance();
            let second = std::ptr::from_mut(&mut second_object).expose_provenance();
            assert!(native_gc_allocate(_py, first));
            assert!(native_gc_allocate(_py, first), "admission is idempotent");
            assert!(native_gc_allocate(_py, second));
            assert!(!native_gc_is_tracked(first));
            assert!(unsafe { native_gc_track(first) });
            assert!(unsafe { native_gc_track(second) });

            let mut candidates = Vec::new();
            assert!(snapshot_registry(&mut candidates, RegistrySelection::All));
            let native = candidates
                .iter()
                .filter_map(|candidate| match candidate.node {
                    GcNode::Native(address) if address == first || address == second => {
                        Some(address)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(native, [first, second]);
            let original_unreachable = candidates.iter().enumerate().filter_map(|(index, candidate)| {
                matches!(candidate.node, GcNode::Native(address) if address == first || address == second).then_some(index)
            }).collect();
            let original = GcScratch {
                candidates,
                first_unreachable: original_unreachable,
                ..Default::default()
            };
            assert_eq!(retired_unreachable_count(&original), 0);

            assert_eq!(native_gc_claim_finalizer(first), 1);
            assert_eq!(native_gc_claim_finalizer(first), 0);
            native_gc_untrack(first);
            assert!(!native_gc_is_tracked(first));
            assert!(native_gc_is_finalized(first));
            assert!(unsafe { native_gc_track(first) });
            assert!(native_gc_is_finalized(first));

            native_gc_untrack(first);
            first_object.ob_refcnt = molt_cpython_abi::abi_types::IMMORTAL_REFCNT;
            assert!(unsafe { native_gc_track(first) });
            assert_eq!(
                first_object.ob_refcnt,
                molt_cpython_abi::abi_types::IMMORTAL_REFCNT,
                "tracking cannot change the native object's lifetime"
            );
            assert!(
                !native_gc_is_tracked(first),
                "native immortality remains untracked"
            );
            assert!(native_gc_is_finalized(first));
            native_gc_untrack(second);
            native_gc_deallocate(_py, first);
            native_gc_deallocate(_py, second);
            assert!(!native_gc_is_enrolled(first));
            assert!(!native_gc_is_enrolled(second));
            unsafe {
                (*std::ptr::with_exposed_provenance_mut::<PyObject>(first)).ob_refcnt = 1;
            }
            assert!(native_gc_allocate(_py, first));
            assert!(unsafe { native_gc_track(first) });
            assert_eq!(
                retired_unreachable_count(&original),
                2,
                "reusing an address does not resurrect its original allocation generation"
            );
            native_gc_deallocate(_py, first);
        });
    }

    #[test]
    fn embedded_teardown_fails_closed_on_live_native_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let address = 0x3030usize;
            assert!(native_gc_allocate(_py, address));
            let state = crate::runtime_state(_py);
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gc_reset_registry(state)));
            assert!(result.is_err());
            native_gc_deallocate(_py, address);
        });
    }

    #[test]
    fn foreign_dynamic_projection_tracks_only_enrolled_native_identity() {
        use molt_cpython_abi::api::refcount::OwnedPyObject;

        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let allocation_count = state.counts()[0];
            let atomic_owner = unsafe {
                OwnedPyObject::from_owned(molt_cpython_abi::api::memory::_PyObject_New(
                    &raw mut PyLong_Type,
                ))
            };
            let atomic_pointer = atomic_owner.as_ptr();
            assert!(!atomic_pointer.is_null());
            let atomic_bits = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(atomic_pointer)
            }
            .expect("atomic foreign wrapper");
            let atomic = crate::obj_from_bits(atomic_bits)
                .as_ptr()
                .expect("foreign wrapper");
            assert!(!unsafe { gc_is_tracked(atomic) });
            assert_eq!(state.counts()[0], allocation_count);
            dec_ref_bits(_py, atomic_bits);
            assert_eq!(unsafe { (*atomic_pointer).ob_refcnt }, 1);
            assert_eq!(state.counts()[0], allocation_count);
            drop(atomic_owner);

            // Cleanup collections also run during assertion unwinds. Use real
            // native storage so an enrolled identity never outlives a stack
            // header or exposes a list payload that was not allocated.
            let gc_owner = unsafe {
                OwnedPyObject::from_owned(molt_cpython_abi::api::memory::_PyObject_GC_New(
                    &raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError,
                ))
            };
            let gc_pointer = gc_owner.as_ptr();
            assert!(!gc_pointer.is_null());
            let gc_address = gc_pointer.expose_provenance();
            assert!(native_gc_is_enrolled(gc_address));
            let enrolled_bits =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(gc_pointer) }
                    .expect("enrolled foreign wrapper");
            let enrolled = crate::obj_from_bits(enrolled_bits)
                .as_ptr()
                .expect("enrolled foreign wrapper");
            assert!(unsafe { gc_is_tracked(enrolled) });
            assert_eq!(state.counts()[0], allocation_count + 2);
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            assert!(bridge.managed_handle_for_pyobj(gc_pointer).is_none());
            assert_eq!(
                unsafe { bridge.handle_to_borrowed_pyobj(enrolled_bits) },
                gc_pointer
            );
            assert!(unsafe { native_gc_track(gc_address) });
            unsafe { molt_cpython_abi::api::memory::PyObject_GC_UnTrack(gc_pointer.cast()) };
            assert!(!native_gc_is_tracked(gc_address));
            assert!(
                unsafe { gc_is_tracked(enrolled) },
                "native untracking preserves wrapper membership"
            );
            assert_eq!(state.counts()[0], allocation_count + 2);
            // Exercise the collector's real clear-before-final-pin-release path.
            // The native allocation still has its external C owner, but the
            // wrapper no longer has the payload that admitted it to membership.
            assert_eq!(
                unsafe { super::super::heap_lifecycle::try_clear_cycle_edges(_py, enrolled) },
                0
            );
            assert_eq!(
                unsafe { super::super::foreign::foreign_ptr_from_obj(enrolled) },
                0
            );
            assert!(unsafe { gc_is_tracked(enrolled) });
            assert_eq!(unsafe { (*gc_pointer).ob_refcnt }, 1);
            assert_eq!(state.counts()[0], allocation_count + 2);
            dec_ref_bits(_py, enrolled_bits);
            assert!(!unsafe { gc_is_tracked(enrolled) });
            assert_eq!(unsafe { (*gc_pointer).ob_refcnt }, 1);
            assert_eq!(state.counts()[0], allocation_count + 1);
            drop(gc_owner);
            assert!(!native_gc_is_enrolled(gc_address));
            assert_eq!(state.counts()[0], allocation_count);
        });
    }

    #[test]
    fn allocation_accounting_survives_membership_demotion_and_explicit_untracking() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let state = &crate::runtime_state(py).gc;
            state.set_enabled(false);
            let allocation_count = state.counts()[0];
            let dictionary = alloc_dict_with_pairs(py, &[]);
            let tuple = alloc_tuple(py, &[MoltObject::from_int(7).bits()]);
            let list = alloc_list(py, &[]);
            assert!(!dictionary.is_null() && !tuple.is_null() && !list.is_null());
            assert_eq!(state.counts()[0], allocation_count + 3);
            unsafe {
                gc_untrack(
                    py,
                    dictionary,
                    TYPE_ID_DICT,
                    GcUntrackReason::DynamicProjection,
                );
                gc_untrack(py, tuple, TYPE_ID_TUPLE, GcUntrackReason::DynamicProjection);
                assert!(!gc_is_tracked(dictionary));
                assert!(!gc_is_tracked(tuple));
                for _ in 0..2 {
                    gc_untrack(py, list, TYPE_ID_LIST, GcUntrackReason::ExplicitControl);
                    assert!(!gc_is_tracked(list));
                    assert_eq!(state.counts()[0], allocation_count + 3);
                    assert!(gc_track_existing(py, list));
                    assert!(gc_is_tracked(list));
                    assert_eq!(state.counts()[0], allocation_count + 3);
                }
                gc_untrack(py, list, TYPE_ID_LIST, GcUntrackReason::ExplicitControl);
            }
            for (index, ptr) in [dictionary, tuple, list].into_iter().enumerate() {
                dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                assert!(!unsafe { gc_is_tracked(ptr) });
                assert_eq!(state.counts()[0], allocation_count + 2 - index as i64);
            }
        });
    }

    #[test]
    #[cfg(not(feature = "free-threaded"))]
    fn promoted_scalar_lists_share_gc_lifetime_through_cycle_retirement() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let state = &crate::runtime_state(py).gc;
            state.set_enabled(false);
            for family in ["int", "bool", "wide_int"] {
                let allocation_count = state.counts()[0];
                let ptr = match family {
                    "int" => crate::object::builders::alloc_list_int_from_raw_slice(py, &[]),
                    "bool" => crate::object::builders::alloc_list_bool_from_raw_slice(py, &[]),
                    // The raw constructor calls integer promotion directly when
                    // its first heap-sized value outgrows the scalar prefix.
                    "wide_int" => {
                        crate::object::builders::alloc_list_int_from_raw_slice(py, &[1, i64::MAX])
                    }
                    _ => unreachable!(),
                }
                .expect("scalar list allocation");
                let bits = MoltObject::from_ptr(ptr).bits();
                let already_promoted = family == "wide_int";
                assert_eq!(unsafe { gc_is_tracked(ptr) }, already_promoted, "{family}");
                assert_eq!(
                    state.counts()[0],
                    allocation_count + i64::from(already_promoted),
                    "{family}",
                );
                crate::molt_list_append(bits, bits);
                assert!(!exception_pending(py));
                assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_LIST);
                assert!(unsafe { gc_is_tracked(ptr) }, "{family}");
                assert_eq!(state.counts()[0], allocation_count + 1, "{family}");
                dec_ref_bits(py, bits);
                let outcome = unsafe { collect_cycles(py) };
                assert_eq!(outcome.status, GcCollectStatus::Completed, "{family}");
                // This opaque lookup also proves the collector did not leave
                // the terminal object's old address in membership storage.
                assert!(!unsafe { gc_is_tracked(ptr) }, "{family}");
            }
        });
    }

    #[test]
    #[ignore = "release cyclic-GC workspace allocation/time probe"]
    fn repeated_reachable_collection_workspace_bench() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    #[ignore = "cycle-capable allocation/deallocation registry hot-path probe"]
    fn cycle_capable_registry_hot_path_bench() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn repeated_reachable_collection_workspace_allocations() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        });
    }

    #[test]
    fn generation_control_matches_cpython_312_threshold_and_count_semantics() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        });
    }

    #[test]
    fn automatic_collection_is_deferred_to_the_runtime_safepoint() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let state = &crate::runtime_state(_py).gc;
            state.set_enabled(false);
            let _ = unsafe { collect_cycles(_py) };
            state.set_thresholds([2, 10, 10]);
            state.set_enabled(true);
            assert_eq!(state.counts(), [0, 0, 0]);
            let collections_before = state.generation_stats()[0].collections;

            let roots = (0..3)
                .map(|_| {
                    let ptr = alloc_list(_py, &[]);
                    assert!(!ptr.is_null());
                    MoltObject::from_ptr(ptr).bits()
                })
                .collect::<Vec<_>>();
            assert_eq!(state.counts()[0], 3);
            assert_eq!(state.generation_stats()[0].collections, collections_before);

            let outcome = unsafe { collect_pending(_py) };
            assert_eq!(outcome.status, GcCollectStatus::Completed);
            assert_eq!(outcome.scanned, 3);
            assert_eq!(outcome.survivors, 3);
            assert_eq!(state.counts(), [0, 1, 0]);
            assert_eq!(
                state.generation_stats()[0].collections,
                collections_before + 1
            );

            for bits in roots {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    #[ignore = "generational GC long-lived/young scan and tail-latency probe"]
    fn generational_scan_reduction_bench() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        assert!(may_form_cycle(crate::object::TYPE_ID_STRING));
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
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
            assert!(matches!(
                get_objects(_py, None),
                Err(GcIntrospectionError::UnsupportedConcurrency)
            ));
            let args = alloc_tuple(_py, &[left_bits]);
            assert!(matches!(
                unsafe { get_referents(_py, args) },
                Err(GcIntrospectionError::UnsupportedConcurrency)
            ));
            assert!(matches!(
                unsafe { get_referrers(_py, args) },
                Err(GcIntrospectionError::UnsupportedConcurrency)
            ));
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
            dec_ref_bits(_py, MoltObject::from_ptr(args).bits());
            dec_ref_bits(_py, left_bits);
            dec_ref_bits(_py, right_bits);
        });
    }

    #[test]
    fn lifecycle_visit_is_side_effect_free_and_clear_is_idempotent() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            for minor in [12, 13, 14] {
                _guard.with_target_python_minor(_py, minor, || {
                    check_dynamic_container_tracking(_py, minor);
                });
            }
        });
    }

    fn check_dynamic_container_tracking(_py: &PyToken<'_>, minor: i64) {
        unsafe {
            let always_track_dicts = minor >= 14;
            let empty_dict = alloc_dict_with_pairs(_py, &[]);
            assert_eq!(gc_is_tracked(empty_dict), always_track_dicts);

            let direct_dict_bits = crate::molt_dict_new(16);
            let direct_dict = obj_from_bits(direct_dict_bits)
                .as_ptr()
                .expect("direct dict allocation");
            assert_eq!(gc_is_tracked(direct_dict), always_track_dicts);

            let atomic_dict = alloc_dict_with_pairs(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert_eq!(gc_is_tracked(atomic_dict), always_track_dicts);

            let list = alloc_list(_py, &[]);
            let list_bits = MoltObject::from_ptr(list).bits();
            let container_dict =
                alloc_dict_with_pairs(_py, &[MoltObject::from_int(1).bits(), list_bits]);
            assert!(gc_is_tracked(container_dict));

            let key = MoltObject::from_int(1).bits();
            let atomic = MoltObject::from_int(2).bits();
            let duplicate = alloc_dict_with_pairs(_py, &[key, list_bits, key, atomic]);
            assert!(
                gc_is_tracked(duplicate),
                "construction preserves sticky tracking"
            );

            // The dictionary is allocated and tracked BEFORE its new tuple.
            // Candidate-order demotion would leave it tracked one GC too long.
            let tuple_dict = alloc_dict_with_pairs(_py, &[key, list_bits]);
            let late_tuple = alloc_tuple(_py, &[atomic]);
            dict_set_in_place(
                _py,
                tuple_dict,
                key,
                MoltObject::from_ptr(late_tuple).bits(),
            );
            collect_cycles(_py);
            assert!(!gc_is_tracked(late_tuple));
            assert_eq!(
                gc_is_tracked(tuple_dict),
                always_track_dicts,
                "tuple and dictionary must demote in the same full collection"
            );

            for operation in 0..6 {
                let mapping = alloc_dict_with_pairs(_py, &[key, list_bits]);
                let bits = MoltObject::from_ptr(mapping).bits();
                match operation {
                    0 => dict_set_in_place(_py, mapping, key, atomic),
                    1 => {
                        assert!(crate::object::ops::dict_del_in_place(_py, mapping, key));
                    }
                    2 => {
                        let value = crate::object::ops_dict::molt_dict_pop(
                            bits,
                            key,
                            MoltObject::none().bits(),
                            MoltObject::from_int(0).bits(),
                        );
                        assert_eq!(value, list_bits);
                        dec_ref_bits(_py, value);
                    }
                    3 => {
                        let pair = crate::object::ops_dict::molt_dict_popitem(bits);
                        assert!(obj_from_bits(pair).as_ptr().is_some());
                        dec_ref_bits(_py, pair);
                    }
                    4 => crate::object::ops::dict_clear_in_place(_py, mapping),
                    5 => {
                        crate::object::ops_dict::dict_update_apply(
                            _py,
                            bits,
                            crate::object::ops_dict::dict_update_set_in_place,
                            MoltObject::from_ptr(atomic_dict).bits(),
                        );
                    }
                    _ => unreachable!(),
                }
                assert!(!exception_pending(_py));
                assert!(gc_is_tracked(mapping), "3.{minor}, operation={operation}");
                for generation in [0, 1] {
                    collect_generation(_py, generation);
                    assert!(
                        gc_is_tracked(mapping),
                        "minor collection must not demote dicts"
                    );
                }
                collect_cycles(_py);
                assert_eq!(gc_is_tracked(mapping), always_track_dicts);
                dec_ref_bits(_py, bits);
            }

            let atomic_tuple = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
            assert!(gc_is_tracked(atomic_tuple));
            let _ = collect_cycles(_py);
            assert!(!gc_is_tracked(atomic_tuple));

            let container_tuple = alloc_tuple(_py, &[list_bits]);
            assert!(gc_is_tracked(container_tuple));
            let _ = collect_cycles(_py);
            assert!(gc_is_tracked(container_tuple));

            // An untracked mutable child can acquire a back edge at any time.
            let child = alloc_dict_with_pairs(_py, &[]);
            let child_bits = MoltObject::from_ptr(child).bits();
            let parent = alloc_dict_with_pairs(_py, &[key, child_bits]);
            let parent_bits = MoltObject::from_ptr(parent).bits();
            collect_cycles(_py);
            assert!(gc_is_tracked(parent));
            dict_set_in_place(_py, child, key, parent_bits);
            dec_ref_bits(_py, child_bits);
            dec_ref_bits(_py, parent_bits);
            assert_eq!(
                collect_cycles(_py).collected,
                2,
                "late back edge must be collectible"
            );

            dec_ref_bits(_py, MoltObject::from_ptr(empty_dict).bits());
            dec_ref_bits(_py, direct_dict_bits);
            dec_ref_bits(_py, MoltObject::from_ptr(atomic_dict).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(container_dict).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(atomic_tuple).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(container_tuple).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(duplicate).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_dict).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(late_tuple).bits());
            dec_ref_bits(_py, list_bits);
        }
    }

    #[test]
    fn physical_gc_edges_preserve_native_targets_and_managed_multiplicity() {
        use molt_cpython_abi::abi_types::{PyBaseExceptionObject, PyExc_RuntimeError};
        use molt_cpython_abi::api::refcount::{Py_DECREF, Py_INCREF};

        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::alloc_exception(py, "RuntimeError", "physical GC edge");
            assert!(!owner.is_null());
            let bits = MoltObject::from_ptr(owner).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                py,
                bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                bits,
            )
            .expect("runtime self edge");
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            let owner_node = GcNode::Runtime(PtrSlot(owner));
            let mut children = Vec::new();
            unsafe { traverse_node(py, owner_node, &mut |child| children.push(child)) };
            assert_eq!(
                children
                    .iter()
                    .filter(|&&child| child == owner_node)
                    .count(),
                2,
                "runtime context and its physical C owner are separate edges",
            );

            #[cfg(not(feature = "free-threaded"))]
            unsafe {
                let scalar_bits = MoltObject::from_int(37).bits();
                let scalar =
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(scalar_bits);
                assert!(!scalar.is_null());
                assert!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .managed_handle_for_pyobj(scalar)
                        .is_none()
                );
                (*view.cast::<PyBaseExceptionObject>()).notes = scalar;
                let args = alloc_tuple(py, &[bits]);
                let referents = get_referents(py, args).expect("physical scalar referents");
                assert!(super::super::seq_access::with_borrowed(
                    referents,
                    |values| { values.contains(&scalar_bits) }
                ));
                let targets = alloc_tuple(py, &[scalar_bits]);
                let referrers = get_referrers(py, targets).expect("physical scalar referrers");
                assert!(
                    super::super::seq_access::with_borrowed(referrers, |values| {
                        values.contains(&bits)
                    }),
                    "the physical C carrier must match the inline public target"
                );
                dec_ref_bits(py, MoltObject::from_ptr(referrers).bits());
                dec_ref_bits(py, MoltObject::from_ptr(targets).bits());
                dec_ref_bits(py, MoltObject::from_ptr(referents).bits());
                dec_ref_bits(py, MoltObject::from_ptr(args).bits());
                (*view.cast::<PyBaseExceptionObject>()).notes = std::ptr::null_mut();
                Py_DECREF(scalar);
            }

            // Use the real C allocation/lifetime authority. An assertion unwind
            // must not leave an enrolled address pointing into a dead Rust frame.
            let native_owner = unsafe {
                molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(
                    molt_cpython_abi::api::memory::_PyObject_GC_New(&raw mut PyExc_RuntimeError),
                )
            };
            let native_ptr = native_owner.as_ptr();
            assert!(!native_ptr.is_null());
            let native = native_ptr.cast::<PyBaseExceptionObject>();
            let address = native_ptr.expose_provenance();
            assert!(native_gc_is_enrolled(address));
            let native_bits =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(native_ptr) }
                    .expect("native custody wrapper");
            #[cfg(not(feature = "free-threaded"))]
            unsafe {
                assert!(native_gc_track(address));
                let scalar_bits = MoltObject::from_int(120037).bits();
                let scalar =
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(scalar_bits);
                assert!(!scalar.is_null());
                assert_eq!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(scalar),
                    Some(scalar_bits),
                    "noncached scalar C storage projects its canonical managed identity"
                );
                (*native).notes = scalar;
                let args = alloc_tuple(py, &[native_bits]);
                let referents =
                    get_referents(py, args).expect("native tp_traverse scalar referents");
                assert!(super::super::seq_access::with_borrowed(
                    referents,
                    |values| { values.contains(&scalar_bits) }
                ));
                let targets = alloc_tuple(py, &[scalar_bits]);
                let referrers =
                    get_referrers(py, targets).expect("native tp_traverse scalar referrers");
                assert!(
                    super::super::seq_access::with_borrowed(referrers, |values| {
                        values.contains(&native_bits)
                    }),
                    "native tp_traverse uses the same public target identity"
                );
                dec_ref_bits(py, MoltObject::from_ptr(referrers).bits());
                dec_ref_bits(py, MoltObject::from_ptr(targets).bits());
                dec_ref_bits(py, MoltObject::from_ptr(referents).bits());
                dec_ref_bits(py, MoltObject::from_ptr(args).bits());
                (*native).notes = std::ptr::null_mut();
                Py_DECREF(scalar);
            }
            let list = alloc_list(py, &[native_bits]);
            assert!(!list.is_null());
            let list_bits = MoltObject::from_ptr(list).bits();
            assert!(
                !unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(list_bits)
                }
                .is_null()
            );
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.mirrored_c_refcount(address),
                1
            );
            assert_eq!(
                unsafe { (*native).ob_base.ob_refcnt },
                3,
                "root, wrapper custody, clean list mirror"
            );
            assert_eq!(
                unsafe { effective_node_refcount(GcNode::Native(address)) },
                2
            );
            unsafe {
                let physical = view.cast::<PyBaseExceptionObject>();
                Py_INCREF(native_ptr);
                Py_INCREF(native_ptr);
                let old_context = std::mem::replace(&mut (*physical).context, native_ptr);
                (*physical).cause = native_ptr;
                Py_DECREF(old_context);
            }
            children.clear();
            unsafe { traverse_node(py, owner_node, &mut |child| children.push(child)) };
            assert_eq!(
                children
                    .iter()
                    .filter(|&&child| child == GcNode::Native(address))
                    .count(),
                2,
                "two C fields retain two native edges",
            );
            assert_eq!(
                children
                    .iter()
                    .filter(|&&child| child == owner_node)
                    .count(),
                1
            );
            let mut referents = Vec::new();
            unsafe { visit_api_referents(py, owner_node, &mut |child| referents.push(child)) };
            assert_eq!(
                referents
                    .iter()
                    .filter(|&&child| child == GcApiTarget::Node(GcNode::Native(address)))
                    .count(),
                2,
                "introspection uses the same physical native-edge authority",
            );
            assert_eq!(
                unsafe { (*native).ob_base.ob_refcnt },
                5,
                "visits do not acquire references"
            );
            assert_eq!(
                unsafe { effective_node_refcount(GcNode::Native(address)) },
                4
            );
            unsafe { molt_clear(py, owner) };
            assert_eq!(
                unsafe { (*native).ob_base.ob_refcnt },
                3,
                "clearing releases each physical edge once"
            );
            dec_ref_bits(py, bits);
            dec_ref_bits(py, list_bits);
            dec_ref_bits(py, native_bits);
            assert_eq!(unsafe { (*native).ob_base.ob_refcnt }, 1);
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.mirrored_c_refcount(address),
                0
            );
            drop(native_owner);
            assert!(!native_gc_is_enrolled(address));
        });
    }

    unsafe extern "C" fn physical_module_noop(
        _self: *mut PyObject,
        _args: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::object::Py_NewRef(&raw mut molt_cpython_abi::abi_types::Py_None)
        }
    }

    #[test]
    fn cfunction_self_module_cycle_retires_its_physical_view() {
        use molt_cpython_abi::abi_types::{METH_NOARGS, PyCFunctionObject, PyMethodDef};
        use molt_cpython_abi::api::{errors, object, refcount};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            let mut method = PyMethodDef {
                ml_name: c"self_module_cycle".as_ptr(),
                ml_meth: Some(physical_module_noop),
                ml_flags: METH_NOARGS,
                ml_doc: std::ptr::null(),
            };
            let callable = unsafe {
                object::PyCFunction_NewEx(
                    &raw mut method,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert!(!callable.is_null());
            let bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(callable)
                .unwrap()
                .bits();
            let runtime = crate::obj_from_bits(bits).as_ptr().unwrap();
            assert!(unsafe { gc_is_tracked(runtime) });
            assert_eq!(
                GLOBAL_BRIDGE.set_cfunction_module(bits, Some(bits)),
                Some(true)
            );
            assert_eq!(
                unsafe { (*callable.cast::<PyCFunctionObject>()).m_module },
                callable
            );
            unsafe { refcount::Py_DECREF(callable) };
            let result = unsafe { collect_cycles(py) };
            assert!(
                result.collected > 0,
                "the independent physical self edge is cycle garbage"
            );
            assert!(
                GLOBAL_BRIDGE.managed_handle_for_pyobj(callable).is_none(),
                "the view must actually retire, not just be reported as collected",
            );
            assert!(!crate::exception_pending(py));
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
        });
    }

    #[repr(C)]
    struct ModuleClearWitness {
        object: PyObject,
        callable_bits: u64,
        callable: *mut PyObject,
        receiver: *mut PyObject,
        releases: usize,
        saw_empty_module: bool,
        saw_receiver_mirror: bool,
    }

    unsafe extern "C" fn observe_module_clear(object: *mut PyObject) {
        use molt_cpython_abi::abi_types::{PyCFunctionObject, PyExc_ValueError};
        use molt_cpython_abi::api::errors;
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

        let witness = unsafe { &mut *object.cast::<ModuleClearWitness>() };
        witness.releases += 1;
        // Reenter the bridge: publication must have released its lock before
        // this ordinary C decref invokes the module's finalizer.
        witness.saw_empty_module = GLOBAL_BRIDGE.cfunction_module(witness.callable_bits)
            == Some(Ok(MoltObject::none().bits()));
        witness.saw_receiver_mirror =
            unsafe { (*witness.callable.cast::<PyCFunctionObject>()).m_self == witness.receiver }
                && GLOBAL_BRIDGE.mirrored_c_refcount(witness.receiver.addr()) > 0;
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"module finalizer".as_ptr(),
            )
        };
    }

    #[test]
    fn cfunction_module_clear_defers_release_and_preserves_slot_mirrors() {
        use molt_cpython_abi::abi_types::{
            METH_NOARGS, PyCFunctionObject, PyMethodDef, PyTypeObject,
        };
        use molt_cpython_abi::api::{errors, object, refcount};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            let receiver = alloc_list(py, &[]);
            assert!(!receiver.is_null());
            let receiver_bits = MoltObject::from_ptr(receiver).bits();
            let receiver_view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(receiver_bits) };
            assert!(!receiver_view.is_null());
            let mut method = PyMethodDef {
                ml_name: c"module_clear_witness".as_ptr(),
                ml_meth: Some(physical_module_noop),
                ml_flags: METH_NOARGS,
                ml_doc: std::ptr::null(),
            };
            let callable = unsafe {
                object::PyCFunction_NewEx(&raw mut method, receiver_view, std::ptr::null_mut())
            };
            assert!(!callable.is_null());
            let bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(callable)
                .unwrap()
                .bits();
            let runtime = crate::obj_from_bits(bits).as_ptr().unwrap();
            let mirrors = GLOBAL_BRIDGE.mirrored_c_refcount(receiver_view.addr());
            assert!(mirrors > 0);
            let mut module_type: PyTypeObject = unsafe { std::mem::zeroed() };
            module_type.tp_name = c"ModuleClearWitness".as_ptr();
            module_type.tp_dealloc = Some(observe_module_clear);
            let mut module = ModuleClearWitness {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut module_type,
                },
                callable_bits: bits,
                callable,
                receiver: receiver_view,
                releases: 0,
                saw_empty_module: false,
                saw_receiver_mirror: false,
            };
            // Transfer the fixture's sole ordinary C owner directly into the
            // writable physical field. There is no mirrored ledger entry.
            unsafe { (*callable.cast::<PyCFunctionObject>()).m_module = &raw mut module.object };
            let (edges, resources) =
                unsafe { super::super::heap_lifecycle::terminal_detach_capacity(py, runtime) };
            assert!(
                resources > 0,
                "physical function fields reserve a detached resource"
            );
            let mut sink = super::super::heap_lifecycle::DetachedEdgeSink::terminal_with_capacities(
                edges, resources,
            );
            unsafe {
                super::super::heap_lifecycle::clear_cycle_edges_with_sink(py, runtime, &mut sink)
            };
            assert!(unsafe { (*callable.cast::<PyCFunctionObject>()).m_module }.is_null());
            assert_eq!(module.releases, 0, "publishing NULL must not run callbacks");
            sink.release_all(py);
            assert_eq!(module.releases, 1);
            assert!(module.saw_empty_module && module.saw_receiver_mirror);
            assert_eq!(
                GLOBAL_BRIDGE.mirrored_c_refcount(receiver_view.addr()),
                mirrors
            );
            assert!(
                unsafe { errors::PyErr_Occurred() }.is_null(),
                "module-finalizer errors are drained"
            );
            unsafe {
                super::super::heap_lifecycle::clear_cycle_edges_with_sink(py, runtime, &mut sink)
            };
            sink.release_all(py);
            assert_eq!(module.releases, 1, "repeated clear is idempotent");
            unsafe { refcount::Py_DECREF(callable) };
            assert_eq!(
                module.releases, 1,
                "terminal release cannot release the old module twice"
            );
            assert_eq!(GLOBAL_BRIDGE.mirrored_c_refcount(receiver_view.addr()), 0);
            dec_ref_bits(py, receiver_bits);
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn exception_self_cycle_with_physical_abi_projection_is_collectible() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        let _lock = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        // Force-enable the alloc/dealloc counters so DEALLOC_COUNT is a live signal
        // (otherwise `profile_hit` is a no-op and the deallocation is invisible to the
        // counter, though `gc_is_tracked` below remains an unconditional proof).
        // SAFETY: the runtime test transaction holds process-state custody.
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
                stats.retired, 2,
                "both original allocation identities retired"
            );
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
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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

    #[test]
    #[cfg(not(feature = "free-threaded"))]
    fn retained_saveall_garbage_is_not_retirement_progress() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let state = &crate::runtime_state(py).gc;
            let garbage = state.garbage_bits(py);
            state.set_debug_flags(DEBUG_SAVEALL);
            let list = alloc_list(py, &[]);
            let bits = MoltObject::from_ptr(list).bits();
            crate::molt_list_append(bits, bits);
            dec_ref_bits(py, bits);
            let stats = unsafe { collect_cycles(py) };
            assert_eq!(stats.status, GcCollectStatus::Completed);
            assert_eq!(stats.collected, 1);
            assert_eq!(
                stats.retired, 0,
                "retaining collectable garbage cannot keep teardown busy"
            );
            assert!(unsafe { gc_is_tracked(list) });
            state.set_debug_flags(0);
            crate::molt_list_clear(garbage);
            dec_ref_bits(py, garbage);
            let stats = unsafe { collect_cycles(py) };
            assert_eq!((stats.collected, stats.retired), (1, 1));
            assert!(!unsafe { gc_is_tracked(list) });
        });
    }

    /// Negative case: a cycle that is STILL REACHABLE from a live external root must
    /// NOT be collected (no false reclamation). `outer` holds `a`, and `a -> b -> a`
    /// is a cycle, but `outer` keeps it alive.
    #[test]
    fn collect_spares_externally_reachable_cycle() {
        let _lock = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        // SAFETY: the runtime test transaction holds process-state custody.
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
    fn collector_pin_owns_physical_lifetime_without_becoming_a_gc_root() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            for viewed in [false, true] {
                let ptr = alloc_list(_py, &[]);
                let bits = MoltObject::from_ptr(ptr).bits();
                if viewed {
                    assert!(
                        !unsafe {
                            molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits)
                        }
                        .is_null()
                    );
                }
                let stable_hold = u32::from(viewed);
                let node = GcNode::Runtime(PtrSlot(ptr));
                unsafe {
                    assert_eq!(header_refcount(ptr), 1 + stable_hold);
                    pin_node(node);
                    assert_eq!(header_refcount(ptr), 2 + stable_hold);
                    assert_eq!(effective_gc_refcount(ptr), 1);
                    dec_ref_bits(_py, bits);
                    assert_eq!(header_refcount(ptr), 1 + stable_hold);
                    assert_eq!(effective_gc_refcount(ptr), 0, "pin is not a GC root");
                    assert!(gc_is_tracked(ptr), "pin keeps allocation live");
                    assert!(!molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(bits));
                    crate::inc_ref_bits(_py, bits);
                    assert_eq!(effective_gc_refcount(ptr), 1);
                    release_node_pin(_py, node);
                    assert!(!(*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_GC_PINNED));
                    assert_eq!(header_refcount(ptr), 1 + stable_hold);
                    assert_eq!(effective_gc_refcount(ptr), 1);
                    dec_ref_bits(_py, bits);
                    assert!(
                        !gc_is_tracked(ptr),
                        "final ordinary owner destroys allocation"
                    );
                }
            }
        });
    }

    #[test]
    fn collector_pin_rebases_view_only_bias_and_defers_last_c_owner_destruction() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_list(_py, &[]);
            let bits = MoltObject::from_ptr(ptr).bits();
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let view = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            let node = GcNode::Runtime(PtrSlot(ptr));
            unsafe {
                molt_cpython_abi::api::refcount::Py_INCREF(view);
                dec_ref_bits(_py, bits);
                assert_eq!(header_refcount(ptr), 1, "only stable view hold remains");
                assert_eq!((*view).ob_refcnt, 1, "one direct C owner, no runtime bias");
                pin_node(node);
                assert_eq!(header_refcount(ptr), 2);
                assert_eq!(
                    (*view).ob_refcnt,
                    2,
                    "physical pin restores runtime-owner bias"
                );
                assert_eq!(
                    effective_gc_refcount(ptr),
                    1,
                    "direct C owner is still a root"
                );
                release_node_pin(_py, node);
                assert_eq!(header_refcount(ptr), 1);
                assert_eq!(
                    (*view).ob_refcnt,
                    1,
                    "pin release retires runtime-owner bias"
                );
                assert!(!bridge.has_finalizing_pin(bits));

                pin_node(node);
                molt_cpython_abi::api::refcount::Py_DECREF(view);
                assert_eq!(
                    (*view).ob_refcnt,
                    1,
                    "physical pin retains runtime-owner bias"
                );
                assert_eq!(header_refcount(ptr), 2);
                assert_eq!(effective_gc_refcount(ptr), 0);
                assert!(
                    !bridge.has_finalizing_pin(bits),
                    "collector pin defers terminal claim"
                );
                release_node_pin(_py, node);
                assert!(
                    !gc_is_tracked(ptr),
                    "last physical pin completes view retirement"
                );
            }
        });
    }

    #[test]
    fn collector_pin_keeps_exception_view_live_through_physical_projection_detach() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_exception(_py, "RuntimeError", "collector pin projection");
            let bits = MoltObject::from_ptr(ptr).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                bits,
            )
            .expect("self context edge");
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let view = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            let node = GcNode::Runtime(PtrSlot(ptr));
            unsafe {
                pin_node(node);
                dec_ref_bits(_py, bits);
                super::super::heap_lifecycle::clear_cycle_edges(_py, ptr);
                assert_eq!(
                    header_refcount(ptr),
                    2,
                    "stable view hold plus physical collector pin"
                );
                assert_eq!(effective_gc_refcount(ptr), 0);
                assert!(!bridge.has_finalizing_pin(bits));
                let projection = view.cast::<molt_cpython_abi::abi_types::PyBaseExceptionObject>();
                assert!(
                    (*projection).context.is_null(),
                    "projection publishes cleared state first"
                );
                release_node_pin(_py, node);
                assert!(!gc_is_tracked(ptr));
            }
            assert!(!crate::exception_pending(_py));
        });
    }

    #[test]
    fn weak_registry_upgrade_preserves_collector_owned_abi_view() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type = crate::builtin_classes(_py).reference_type;
            let weak = unsafe {
                crate::alloc_instance_for_class(
                    _py,
                    obj_from_bits(reference_type).as_ptr().unwrap(),
                )
            };
            let ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let bits = MoltObject::from_ptr(ptr).bits();
            assert_eq!(
                crate::molt_weakref_register(weak, bits, MoltObject::none().bits()),
                MoltObject::from_bool(true).bits(),
            );
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let view = unsafe { bridge.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            let node = GcNode::Runtime(PtrSlot(ptr));
            unsafe {
                molt_cpython_abi::api::refcount::Py_INCREF(view);
                dec_ref_bits(_py, bits);
                pin_node(node);
                molt_cpython_abi::api::refcount::Py_DECREF(view);
                let owned = crate::object::weakref::weakref_peek_owned(_py, weak)
                    .expect("registry upgrades the still-live physical collector owner");
                assert_eq!(owned, bits);
                assert_eq!(
                    header_refcount(ptr),
                    3,
                    "stable hold, collector, upgraded owner"
                );
                assert_eq!(
                    (*view).ob_refcnt,
                    1,
                    "one runtime-owner bias regardless of owner count"
                );
                release_node_pin(_py, node);
                assert_eq!(header_refcount(ptr), 2);
                assert!(!bridge.has_finalizing_pin(bits));
                dec_ref_bits(_py, owned);
                assert!(!gc_is_tracked(ptr));
            }
            assert_eq!(crate::object::weakref::weakref_peek_owned(_py, weak), None);
            dec_ref_bits(_py, weak);
        });
    }

    #[test]
    fn abi_view_hold_does_not_root_an_unreachable_cycle() {
        let _lock = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
        let _lock = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
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
