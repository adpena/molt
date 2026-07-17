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
//!     `__del__`, so `gc.garbage` is ALWAYS empty (every `__del__`-bearing cycle is
//!     collectable). These two steps collapse but their POSITION (before weakrefs)
//!     is documented here so the surviving order matches CPython.
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
//! `gc_refs` scratch, and the unreachable set are TRANSIENT Rust structures built at
//! collection entry and dropped at exit — sound because collection is stop-the-world
//! under the GIL. Per-object `gc_refs` lives in a `HashMap` keyed by the object's
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

use std::collections::{HashMap, hash_map::Entry};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
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

/// Side registry of live cycle-capable objects (CPython's gc-tracked
/// generations, adapted to one generation in v1). Each pointer receives a
/// monotonic allocation ordinal, and collection snapshots sort by that ordinal;
/// allocator addresses and randomized hash iteration therefore cannot change
/// finalizer/clear order across identical runs. Populated at allocation of a
/// non-GREEN object and removed at free. GREEN/atomic objects are never inserted.
///
/// This is its OWN structure, not the provenance pointer registry — the latter is
/// populated only in debug builds (`from_ptr` skips `register_ptr` in release), so
/// it cannot enumerate live objects in the shipped profile.
struct TrackedRegistryShard {
    entries: HashMap<PtrSlot, u64>,
}

const TRACKED_REGISTRY_SHARDS: usize = 64;
const _: () = assert!(TRACKED_REGISTRY_SHARDS.is_power_of_two());

struct TrackedRegistry {
    shards: [Mutex<TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS],
    next_allocation_id: AtomicU64,
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
    })
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
            entry.insert(allocation_id);
            profile_gc_track();
        }
    } else if shard.entries.remove(&slot).is_some() {
        profile_gc_untrack(1);
    }
}

unsafe fn reproject_immutable_tuples(
    py: &PyToken<'_>,
    mut candidates: Vec<*mut u8>,
) -> Vec<*mut u8> {
    candidates.retain(|&ptr| {
        if super::heap_track_projection(unsafe { object_type_id(ptr) })
            != Some(super::HeapTrackProjection::TupleDynamic)
            || unsafe { super::heap_lifecycle::projected_track_state(py, ptr) }
        {
            return true;
        }
        unsafe {
            gc_untrack(
                ptr,
                super::TYPE_ID_TUPLE,
                GcUntrackReason::DynamicProjection,
            )
        };
        false
    });
    candidates
}

/// Register a freshly-allocated object in the tracked set IFF it can form a cycle.
/// Called from the allocator for every heap object; GREEN types return immediately.
///
/// # Safety
/// `ptr` must be a live object pointer (data pointer, past the header).
#[inline]
pub(crate) unsafe fn gc_track_if_cyclic(ptr: *mut u8, type_id: u32) {
    if !may_form_cycle(type_id) {
        return;
    }
    let registry = tracked_registry();
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    let slot = PtrSlot(ptr);
    if let Entry::Vacant(entry) = shard.entries.entry(slot) {
        let allocation_id = registry
            .next_allocation_id
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |next| {
                next.checked_add(1)
            })
            .expect("GC allocation ordinal exhausted");
        entry.insert(allocation_id);
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

pub(crate) unsafe fn gc_untrack(ptr: *mut u8, type_id: u32, _reason: GcUntrackReason) {
    if !may_form_cycle(type_id) {
        return;
    }
    let shard_index = tracked_registry_shard_index(ptr);
    let mut shard = lock_tracked_registry_shard(shard_index);
    if shard.entries.remove(&PtrSlot(ptr)).is_some() {
        profile_gc_untrack(1);
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
pub(crate) fn gc_reset_registry() {
    let registry = tracked_registry();
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
}

fn snapshot_tracked_registry() -> Option<Vec<*mut u8>> {
    // Freeze the entire registry while taking the snapshot. Object graph
    // traversal still requires the runtime's stop-the-world/GIL collection
    // boundary, but tracking metadata itself is now coherent under a future
    // free-threaded allocator rather than a per-shard temporal patchwork.
    let shards: [MutexGuard<'static, TrackedRegistryShard>; TRACKED_REGISTRY_SHARDS] =
        std::array::from_fn(lock_tracked_registry_shard);
    let mut entries: Vec<(u64, *mut u8)> = Vec::new();
    let entry_count = shards
        .iter()
        .map(|shard| shard.entries.len())
        .sum::<usize>();
    if entries.try_reserve_exact(entry_count).is_err() {
        profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
        return None;
    }
    for shard in &shards {
        entries.extend(shard.entries.iter().filter_map(|(slot, allocation_id)| {
            // Acquire pairs with constructor publication. A concurrent
            // collector may observe registry insertion first, but never
            // traverses a partially initialized payload.
            unsafe { (*header_from_obj_ptr(slot.0)).gc_is_published() }
                .then_some((*allocation_id, slot.0))
        }));
    }
    drop(shards);
    entries.sort_unstable_by_key(|(allocation_id, _)| *allocation_id);
    let mut candidates = Vec::new();
    if candidates.try_reserve_exact(entries.len()).is_err() {
        profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
        return None;
    }
    candidates.extend(entries.into_iter().map(|(_, ptr)| ptr));
    Some(candidates)
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
    fn completed(collected: usize) -> Self {
        Self {
            collected,
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

unsafe fn pin_unreachable(ptrs: &[*mut u8]) {
    for &ptr in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).pin_for_gc() };
    }
}

unsafe fn release_unreachable_pins(py: &PyToken<'_>, ptrs: &[*mut u8]) {
    for &ptr in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED) };
        unsafe { dec_ref_ptr(py, ptr) };
    }
}

unsafe fn release_index_pins(py: &PyToken<'_>, candidates: &[*mut u8], indices: &[usize]) {
    for &index in indices {
        let ptr = candidates[index];
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED) };
        unsafe { dec_ref_ptr(py, ptr) };
    }
}

unsafe fn detach_requirements(
    py: &PyToken<'_>,
    candidates: &[*mut u8],
    indices: &[usize],
) -> (usize, usize) {
    let mut edges = 0usize;
    let mut resources = 0usize;
    for &index in indices {
        let ptr = candidates[index];
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
struct GcScratch {
    candidates: Vec<*mut u8>,
    index: HashMap<u64, usize>,
    refs: Vec<i64>,
    marks: Vec<u8>,
    queue: Vec<usize>,
    first_unreachable: Vec<usize>,
    first_unreachable_ptrs: Vec<*mut u8>,
    final_unreachable: Vec<usize>,
}

impl GcScratch {
    fn try_new(candidates: Vec<*mut u8>) -> Option<Self> {
        let len = candidates.len();
        let mut index = HashMap::new();
        index.try_reserve(len).ok()?;
        for (candidate_index, &ptr) in candidates.iter().enumerate() {
            index.insert(addr_key(ptr), candidate_index);
        }

        let mut refs = Vec::new();
        refs.try_reserve_exact(len).ok()?;
        refs.resize(len, 0);
        let mut marks = Vec::new();
        marks.try_reserve_exact(len).ok()?;
        marks.resize(len, 0);
        let mut queue = Vec::new();
        queue.try_reserve_exact(len).ok()?;
        let mut first_unreachable = Vec::new();
        first_unreachable.try_reserve_exact(len).ok()?;
        let mut final_unreachable = Vec::new();
        final_unreachable.try_reserve_exact(len).ok()?;
        let mut first_unreachable_ptrs = Vec::new();
        first_unreachable_ptrs.try_reserve_exact(len).ok()?;
        Some(Self {
            candidates,
            index,
            refs,
            marks,
            queue,
            first_unreachable,
            first_unreachable_ptrs,
            final_unreachable,
        })
    }
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
    candidates: &[*mut u8],
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
        let ptr = candidates[candidate_index];
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
        molt_traverse(py, candidates[candidate_index], &mut |child| {
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
            molt_traverse(py, candidates[candidate_index], &mut |child| {
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
            unsafe { header_set_collecting(candidates[candidate_index], false) };
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
        first_unreachable_ptrs.push(candidates[candidate_index]);
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

/// The full cyclic collection. Stop-the-world under the GIL. Returns the number of
/// objects reclaimed.
///
/// # Safety
/// The GIL must be held (asserted). Reentrancy is prevented by `GC_RUNNING`.
pub(crate) unsafe fn collect_cycles(py: &PyToken<'_>) -> CollectStats {
    unsafe {
        crate::gil_assert();

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

        // Snapshot the candidate set (a stable Vec; the registry mutex is released
        // before traversal so re-entrant dec_ref during finalize/clear can update it).
        let Some(candidates) = snapshot_tracked_registry() else {
            return CollectStats::failure(
                py,
                GcCollectStatus::ResourceError("tracked-registry snapshot allocation failed"),
            );
        };
        let candidates = reproject_immutable_tuples(py, candidates);
        if gc_trace_enabled() {
            eprintln!("molt gc: collect_cycles candidates={}", candidates.len());
        }
        if candidates.is_empty() {
            return CollectStats::completed(0);
        }

        let Some(mut scratch) = GcScratch::try_new(candidates) else {
            profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
            return CollectStats::failure(
                py,
                GcCollectStatus::ResourceError("cycle-collector scratch allocation failed"),
            );
        };

        // STEP 1-3: trial-deletion partition using one preallocated index/mark arena.
        deduce_all(py, &mut scratch);
        if gc_trace_enabled() {
            eprintln!(
                "molt gc: deduce_unreachable unreachable={}",
                scratch.first_unreachable.len()
            );
        }
        if scratch.first_unreachable.is_empty() {
            return CollectStats::completed(0);
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
            for &ptr in &scratch.first_unreachable_ptrs {
                header_set_collecting(ptr, false);
            }
            profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
            return CollectStats::failure(
                py,
                GcCollectStatus::ResourceError("detached-edge reservation failed"),
            );
        };

        // Pin the entire set before the first callback/finalizer.
        pin_unreachable(&scratch.first_unreachable_ptrs);

        crate::object::weakref::weakref_handle_cycle_unreachable(
            py,
            &scratch.first_unreachable_ptrs,
            |wr_ptr| header_is_collecting(wr_ptr),
        );

        for &ptr in &scratch.first_unreachable_ptrs {
            run_finalizer_once(py, ptr);
        }

        // Reuse the exact same index, refs, mark, queue, and output storage.
        // No allocation is permitted in the post-callback resurrection partition.
        deduce_after_finalizers(py, &mut scratch);
        if scratch.final_unreachable.is_empty() {
            release_unreachable_pins(py, &scratch.first_unreachable_ptrs);
            return CollectStats::completed(0);
        }

        // Marks == 2 are resurrected/reachable after the second partition.
        for &candidate_index in &scratch.first_unreachable {
            if scratch.marks[candidate_index] == 2 {
                let ptr = scratch.candidates[candidate_index];
                let header = header_from_obj_ptr(ptr);
                (*header).fetch_and_flags(!HEADER_FLAG_GC_PINNED);
                dec_ref_ptr(py, ptr);
            }
        }

        let collected = scratch.final_unreachable.len();
        if gc_trace_enabled() {
            eprintln!("molt gc: delete_garbage collected={collected}");
        }

        let (required_edges, required_resources) =
            detach_requirements(py, &scratch.candidates, &scratch.final_unreachable);
        if !detached.try_ensure_capacities(required_edges, required_resources) {
            for &candidate_index in &scratch.final_unreachable {
                header_set_collecting(scratch.candidates[candidate_index], false);
            }
            release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);
            profile_hit_unchecked(&GC_SNAPSHOT_ALLOC_FAILURE_COUNT);
            return CollectStats::failure(
                py,
                GcCollectStatus::ResourceError("post-finalizer detached-edge reservation failed"),
            );
        }

        for &candidate_index in &scratch.final_unreachable {
            header_set_collecting(scratch.candidates[candidate_index], false);
        }
        for &candidate_index in &scratch.final_unreachable {
            super::heap_lifecycle::clear_cycle_edges_with_sink(
                py,
                scratch.candidates[candidate_index],
                &mut detached,
            );
        }
        detached.release_all(py);
        release_index_pins(py, &scratch.candidates, &scratch.final_unreachable);

        crate::runtime_state(py)
            .gc_last_failure
            .store(0, AtomicOrdering::Release);
        CollectStats::completed(collected)
    }
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
                2,
                "releasing the direct C root exposes the exception and its tracked args tuple"
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
