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
//! never registered in the tracked set, never scanned, never `clear`ed. Only the
//! cycle-forming container types — user instances (`TYPE_ID_OBJECT`), exceptions,
//! `dict`, `list`, `tuple`, and `set` — are tracked.

use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Instant;

use crate::builtins::exceptions::{
    exception_detach_owned_edges, exception_release_detached_edges, exception_visit_owned_edges,
};
use crate::object::layout::{
    iter_cached_tuple, iter_expected_version, iter_set_cached_tuple, iter_set_expected_version,
    iter_set_target_bits, iter_target_bits, seq_vec_ptr,
};
use crate::object::{
    HEADER_FLAG_FINALIZER_RAN, HEADER_FLAG_GC_COLLECTING, HEADER_FLAG_GC_PINNED,
    HEADER_FLAG_HAS_ABI_VIEW, PtrSlot, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_FROZENSET,
    TYPE_ID_ITER, TYPE_ID_LIST, TYPE_ID_OBJECT, TYPE_ID_SET, TYPE_ID_TUPLE,
    TYPE_ID_WEAK_CONTAINER_STATE, dec_ref_ptr, header_from_obj_ptr, instance_dict_bits,
    object_class_bits, object_class_has_finalizer, object_type_id,
};
use crate::{
    GC_REGISTRY_LOCK_CONTENTION_COUNT, GC_REGISTRY_LOCK_WAIT_NS, GC_SNAPSHOT_ALLOC_FAILURE_COUNT,
    GC_TRACK_COUNT, GC_TRACKED_HIGH_WATER, GC_TRACKED_LIVE, GC_UNTRACK_COUNT, MoltObject, PyToken,
    obj_from_bits, profile_enabled_unchecked, profile_hit_bytes_unchecked, profile_hit_unchecked,
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
/// Tracked (non-GREEN) set, v1: user instances, exceptions, and the resizable ref
/// containers that are the canonical Python cycle formers. `tuple` is included because a tuple can
/// hold a reference to a mutable container that points back (`l = []; t = (l,);
/// l.append(t)`), exactly as CPython tracks tuples. All other ref-holding runtime
/// types (function/code/bound-method/...) are conservatively GREEN in v1: a cycle
/// routed exclusively through them is not collected (a documented v1 limitation,
/// never a double-free — they are simply not `clear`ed), and extending coverage is a
/// one-line addition to this match plus the `traverse`/`clear` authorities below.
#[inline]
pub(crate) fn may_form_cycle(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_OBJECT
            | TYPE_ID_DICT
            | TYPE_ID_LIST
            | TYPE_ID_TUPLE
            | TYPE_ID_SET
            | TYPE_ID_FROZENSET
            | TYPE_ID_ITER
            | TYPE_ID_EXCEPTION
            | TYPE_ID_WEAK_CONTAINER_STATE
    )
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

/// Remove an object from the tracked set as it is freed. Called from the
/// deallocator for every freed object; a no-op (cheap set miss) for GREEN types and
/// untracked objects.
///
/// # Safety
/// `ptr` identifies the object being freed.
#[inline]
pub(crate) unsafe fn gc_untrack_on_free(ptr: *mut u8, type_id: u32) {
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
        entries.extend(
            shard
                .entries
                .iter()
                .map(|(slot, allocation_id)| (*allocation_id, slot.0)),
        );
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
/// double-free (cleared an edge the dealloc also frees). The drift gate
/// `gc_traverse_matches_dealloc` (unit test) pins this equivalence.
///
/// Primitive children (int/float/bool/None/str/bytes — anything that is not a heap
/// pointer, or a GREEN leaf) are skipped: only TAG_PTR values reach `visit`.
///
/// # Safety
/// `ptr` must be a live object of a `may_form_cycle` type. The GIL is held (the
/// `TYPE_ID_OBJECT` arm reads class metadata through the shared inline-field walker).
pub(crate) unsafe fn molt_traverse(py: &PyToken<'_>, ptr: *mut u8, visit: &mut dyn FnMut(*mut u8)) {
    unsafe {
        let type_id = object_type_id(ptr);
        let flags = (*header_from_obj_ptr(ptr)).flags;
        if (flags & super::HEADER_FLAG_IS_WEAKREF) != 0
            && let Some(bits) = super::weakref::weakref_object_callback_bits(py, ptr)
        {
            if let Some(child) = obj_from_bits(bits).as_ptr() {
                visit(child);
            }
            crate::dec_ref_bits(py, bits);
        }
        match type_id {
            TYPE_ID_LIST => {
                let Some(heap_edge_count) = crate::object::seq_access::tracked_heap_edge_count(ptr)
                else {
                    return;
                };
                #[cfg(debug_assertions)]
                debug_assert_eq!(
                    heap_edge_count,
                    crate::object::seq_access::with_borrowed(ptr, |items| {
                        items
                            .iter()
                            .copied()
                            .filter(|bits| crate::object::refcount_opt::is_heap_ref(*bits))
                            .count()
                    }),
                    "generic sequence heap-edge count drifted from canonical storage"
                );
                #[cfg(not(debug_assertions))]
                let _ = heap_edge_count;
                crate::object::seq_access::with_borrowed(ptr, |items| {
                    for &bits in items {
                        if let Some(child) = obj_from_bits(bits).as_ptr() {
                            visit(child);
                        }
                    }
                });
                if (flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
                    let self_bits = MoltObject::from_ptr(ptr).bits();
                    for child_bits in
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE.list_view_handles_for_gc(self_bits)
                    {
                        if let Some(child) = obj_from_bits(child_bits).as_ptr() {
                            visit(child);
                        }
                    }
                }
            }
            TYPE_ID_TUPLE => {
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                    for &bits in items {
                        if let Some(child) = obj_from_bits(bits).as_ptr() {
                            visit(child);
                        }
                    }
                });
            }
            TYPE_ID_DICT => {
                // `order` is the [key0, val0, key1, val1, ...] interleaved Vec, the
                // SAME backing the dealloc cascade releases via
                // `release_dealloc_tracked_bits_vec(dict_order_ptr)`.
                let order_ptr = crate::builtins::containers::dict_order_ptr(ptr);
                if order_ptr.is_null() {
                    return;
                }
                for &bits in (*order_ptr).iter() {
                    if let Some(child) = obj_from_bits(bits).as_ptr() {
                        visit(child);
                    }
                }
            }
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let order_ptr = crate::builtins::containers::set_order_ptr(ptr);
                if order_ptr.is_null() {
                    return;
                }
                for &bits in (*order_ptr).iter() {
                    if let Some(child) = obj_from_bits(bits).as_ptr() {
                        visit(child);
                    }
                }
            }
            TYPE_ID_EXCEPTION => {
                exception_visit_owned_edges(ptr, |bits| {
                    if let Some(child) = obj_from_bits(bits).as_ptr() {
                        visit(child);
                    }
                });
                if (flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
                    let self_bits = MoltObject::from_ptr(ptr).bits();
                    for child_bits in molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .exception_view_handles_for_gc(self_bits)
                    {
                        if let Some(child) = obj_from_bits(child_bits).as_ptr() {
                            visit(child);
                        }
                    }
                }
            }
            TYPE_ID_OBJECT => {
                // Inline typed attribute fields (the `__slots__` / folded-attr
                // storage) + the trailing `__dict__`. This mirrors the
                // `TYPE_ID_OBJECT` dealloc arm: `dec_ref_object_inline_fields`
                // (inline slots) + `instance_dict_bits` (__dict__). We do NOT
                // traverse the class as a cycle edge here for the same reason CPython
                // does not collect type objects in the common path — but we DO
                // traverse the instance dict and inline fields, which is where user
                // reference cycles live.
                let class_bits = object_class_bits(ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    crate::builtins::attr::for_each_object_inline_field_ptr(
                        py,
                        ptr,
                        class_ptr,
                        &mut |_slot, val| {
                            if let Some(child) = obj_from_bits(val).as_ptr() {
                                visit(child);
                            }
                        },
                    );
                }
                let dict_bits = instance_dict_bits(ptr);
                if let Some(child) = obj_from_bits(dict_bits).as_ptr() {
                    visit(child);
                }
            }
            TYPE_ID_WEAK_CONTAINER_STATE => {
                super::weak_container::weakcontainer_traverse(ptr, visit);
            }
            TYPE_ID_ITER => {
                if let Some(child) = obj_from_bits(iter_target_bits(ptr)).as_ptr() {
                    visit(child);
                }
                let cached = iter_cached_tuple(ptr);
                if !cached.is_null() {
                    visit(cached);
                }
            }
            _ => {}
        }
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
pub(crate) unsafe fn molt_clear(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        let type_id = object_type_id(ptr);
        let flags = (*header_from_obj_ptr(ptr)).flags;
        let weakref_registration = if (flags & super::HEADER_FLAG_IS_WEAKREF) != 0 {
            super::weakref::weakref_object_detach(py, ptr)
        } else {
            None
        };
        super::weakref::weakref_object_release(py, weakref_registration);
        match type_id {
            TYPE_ID_LIST => {
                let vec_ptr = seq_vec_ptr(ptr);
                if vec_ptr.is_null() {
                    return;
                }
                let mutation_guard = crate::object::backing::tracked_vec_mutation_lock(vec_ptr);
                let detached = crate::object::backing::tracked_vec_take_contents(vec_ptr);
                let clear_list_projection = (flags & HEADER_FLAG_HAS_ABI_VIEW) != 0;
                (*header_from_obj_ptr(ptr)).flags &= !super::HEADER_FLAG_CONTAINS_REFS;
                crate::object::backing::tracked_vec_bump_mutation_epoch(vec_ptr);
                drop(mutation_guard);
                if clear_list_projection {
                    // `clear_list_view` retires C edges and may run finalizers;
                    // never retain the per-list mutation lock across it.
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_list_view(MoltObject::from_ptr(ptr).bits());
                }
                for &bits in detached.iter() {
                    crate::dec_ref_bits(py, bits);
                }
                drop(detached);
            }
            // CPython tuples have no tp_clear. Their immutable edges remain
            // stable; a mutable peer (list, dict, set, or instance) breaks every
            // collectable cycle in which a tuple participates.
            TYPE_ID_TUPLE => {}
            TYPE_ID_DICT => {
                let order_ptr = crate::builtins::containers::dict_order_ptr(ptr);
                let table_ptr = crate::builtins::containers::dict_table_ptr(ptr);
                let hashes_ptr = crate::builtins::containers::dict_hashes_ptr(ptr);
                let detached = if order_ptr.is_null() {
                    Vec::new()
                } else {
                    std::mem::take(&mut *order_ptr)
                };
                // Publish one valid empty state before releasing any Python edge.
                // A child finalizer may re-enter every dict operation.
                if !table_ptr.is_null() {
                    (*table_ptr).clear();
                }
                if !hashes_ptr.is_null() {
                    (*hashes_ptr).clear();
                }
                for bits in detached {
                    crate::dec_ref_bits(py, bits);
                }
            }
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let order_ptr = crate::builtins::containers::set_order_ptr(ptr);
                let table_ptr = crate::builtins::containers::set_table_ptr(ptr);
                let hashes_ptr = crate::builtins::containers::set_hashes_ptr(ptr);
                let detached = if order_ptr.is_null() {
                    Vec::new()
                } else {
                    std::mem::take(&mut *order_ptr)
                };
                // Publish one valid empty state before releasing any Python edge.
                // A child finalizer may re-enter membership/hash-table operations.
                if !table_ptr.is_null() {
                    (*table_ptr).clear();
                }
                if !hashes_ptr.is_null() {
                    (*hashes_ptr).clear();
                }
                for bits in detached {
                    crate::dec_ref_bits(py, bits);
                }
            }
            TYPE_ID_EXCEPTION => {
                let detached = exception_detach_owned_edges(ptr);
                if (flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .clear_exception_view_fields(MoltObject::from_ptr(ptr).bits());
                }
                exception_release_detached_edges(py, detached);
            }
            TYPE_ID_OBJECT => {
                // Mirror the `TYPE_ID_OBJECT` dealloc child set: inline typed fields,
                // then `__dict__`. `dec_ref_object_inline_fields` zeroes each slot
                // BEFORE dec-ref (so a re-entrant access never sees a stale pointer).
                // We do NOT drop the class reference here — `clear` breaks the cycle
                // through DATA edges; the class is released by the normal dealloc when
                // the instance is actually freed (clearing the class while the
                // instance is still on the unreachable list would desync the dealloc
                // arm and risk a double-decref of the class).
                let class_bits = object_class_bits(ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    crate::builtins::attr::dec_ref_object_inline_fields(py, ptr, class_ptr);
                }
                let dict_ptr = crate::object::instance_dict_bits_ptr(ptr);
                if !dict_ptr.is_null() {
                    let dict_bits = *dict_ptr;
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        *dict_ptr = MoltObject::none().bits();
                        crate::dec_ref_bits(py, dict_bits);
                    }
                }
            }
            TYPE_ID_WEAK_CONTAINER_STATE => {
                super::weak_container::weakcontainer_clear_state(py, ptr);
            }
            TYPE_ID_ITER => {
                let target_bits = iter_target_bits(ptr);
                let cached = iter_cached_tuple(ptr);
                iter_set_target_bits(ptr, MoltObject::none().bits());
                iter_set_cached_tuple(ptr, std::ptr::null_mut());
                if let Some(target_ptr) = obj_from_bits(target_bits).as_ptr()
                    && object_type_id(target_ptr) == TYPE_ID_WEAK_CONTAINER_STATE
                {
                    let version = iter_expected_version(ptr);
                    if version != super::weak_container::WEAK_ITER_VERSION_UNSTARTED
                        && version != super::weak_container::WEAK_ITER_VERSION_FINISHED
                    {
                        super::weak_container::weakcontainer_iter_finish(py, target_ptr);
                    }
                    iter_set_expected_version(
                        ptr,
                        super::weak_container::WEAK_ITER_VERSION_FINISHED,
                    );
                }
                if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                    crate::dec_ref_bits(py, target_bits);
                }
                if !cached.is_null() {
                    dec_ref_ptr(py, cached);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// The collector — deduce_unreachable + CPython 6-step destruction
// ---------------------------------------------------------------------------

/// Result of one `collect_cycles` invocation: the number of objects reclaimed (the
/// `m` that `gc.collect()` returns; `n` = uncollectable = 0 for molt since
/// `gc.garbage` is always empty under PEP 442).
pub(crate) struct CollectStats {
    pub(crate) collected: usize,
}

#[inline]
unsafe fn header_refcount(ptr: *mut u8) -> u32 {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).ref_count.load(AtomicOrdering::Acquire)
    }
}

#[inline]
unsafe fn effective_gc_refcount(ptr: *mut u8) -> isize {
    let mut raw = unsafe { header_refcount(ptr) } as isize;
    let header = unsafe { header_from_obj_ptr(ptr) };
    if unsafe { (*header).flags } & HEADER_FLAG_GC_PINNED != 0 {
        raw -= 1;
    }
    if unsafe { (*header).flags } & HEADER_FLAG_HAS_ABI_VIEW == 0 {
        return raw;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let adjusted = raw + molt_cpython_abi::bridge::GLOBAL_BRIDGE.gc_ref_adjustment(bits);
    if molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(bits) {
        adjusted.max(1)
    } else {
        adjusted
    }
}

unsafe fn pin_unreachable(ptrs: &[*mut u8]) {
    for &ptr in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe {
            (*header).ref_count.fetch_add(1, AtomicOrdering::Relaxed);
            (*header).flags |= HEADER_FLAG_GC_PINNED;
        }
    }
}

unsafe fn release_unreachable_pins(py: &PyToken<'_>, ptrs: &[*mut u8]) {
    for &ptr in ptrs {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).flags &= !HEADER_FLAG_GC_PINNED };
        unsafe { dec_ref_ptr(py, ptr) };
    }
}

#[inline]
unsafe fn header_set_collecting(ptr: *mut u8, on: bool) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        if on {
            (*header).flags |= HEADER_FLAG_GC_COLLECTING;
        } else {
            (*header).flags &= !HEADER_FLAG_GC_COLLECTING;
        }
    }
}

#[inline]
unsafe fn header_is_collecting(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        ((*header).flags & HEADER_FLAG_GC_COLLECTING) != 0
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
unsafe fn deduce_unreachable(py: &PyToken<'_>, candidates: Vec<*mut u8>) -> Vec<*mut u8> {
    unsafe {
        let mut gc_refs: HashMap<u64, isize> = HashMap::with_capacity(candidates.len());

        // update_refs: gc_refs := refcount; mark COLLECTING.
        for &ptr in &candidates {
            let rc = effective_gc_refcount(ptr);
            gc_refs.insert(addr_key(ptr), rc);
            header_set_collecting(ptr, true);
        }

        // subtract_refs: for each candidate, traverse children; decrement gc_refs of
        // each child that is itself COLLECTING (in the candidate set).
        for &ptr in &candidates {
            let gc_refs_ptr: *mut HashMap<u64, isize> = &mut gc_refs;
            molt_traverse(py, ptr, &mut |child| {
                if header_is_collecting(child)
                    && let Some(slot) = (*gc_refs_ptr).get_mut(&addr_key(child))
                {
                    *slot -= 1;
                }
            });
        }

        // move_unreachable: BFS. Objects with gc_refs > 0 are roots; mark them and
        // their transitive referents reachable. Remaining gc_refs == 0 objects are
        // the unreachable cycle garbage. We model CPython's `visit_reachable`
        // pull-back with a `reachable` set + a work queue; an object is reachable iff
        // its key is in `reachable`.
        let mut reachable: HashSet<u64> = HashSet::with_capacity(candidates.len());
        let mut queue: Vec<*mut u8> = Vec::new();
        for &ptr in &candidates {
            let key = addr_key(ptr);
            if gc_refs.get(&key).copied().unwrap_or(0) > 0 && reachable.insert(key) {
                queue.push(ptr);
            }
        }
        while let Some(ptr) = queue.pop() {
            let reachable_ptr: *mut HashSet<u64> = &mut reachable;
            let queue_ptr: *mut Vec<*mut u8> = &mut queue;
            molt_traverse(py, ptr, &mut |child| {
                if !header_is_collecting(child) {
                    return; // not a candidate
                }
                let key = addr_key(child);
                if (*reachable_ptr).insert(key) {
                    (*queue_ptr).push(child);
                }
            });
        }

        // Partition: reachable objects clear COLLECTING; the rest are unreachable
        // garbage (COLLECTING stays SET, so the weakref pass can detect a weakref
        // object that is itself collecting). Insertion order of `candidates` is
        // preserved for deterministic finalization order.
        let mut unreachable: Vec<*mut u8> = Vec::new();
        for &ptr in &candidates {
            if reachable.contains(&addr_key(ptr)) {
                header_set_collecting(ptr, false);
            } else {
                unreachable.push(ptr);
            }
        }
        unreachable
    }
}

/// The full cyclic collection. Stop-the-world under the GIL. Returns the number of
/// objects reclaimed.
///
/// # Safety
/// The GIL must be held (asserted). Reentrancy is prevented by `GC_RUNNING`.
pub(crate) unsafe fn collect_cycles(py: &PyToken<'_>) -> CollectStats {
    unsafe {
        crate::gil_assert();

        // Reentrancy guard: a `__del__` run during finalization must not recursively
        // launch another collection (CPython sets `gcstate->collecting`).
        if GC_RUNNING.swap(true, AtomicOrdering::AcqRel) {
            return CollectStats { collected: 0 };
        }
        let _guard = GcRunningGuard;

        // Snapshot the candidate set (a stable Vec; the registry mutex is released
        // before traversal so re-entrant dec_ref during finalize/clear can update it).
        let Some(candidates) = snapshot_tracked_registry() else {
            return CollectStats { collected: 0 };
        };
        if gc_trace_enabled() {
            eprintln!("molt gc: collect_cycles candidates={}", candidates.len());
        }
        if candidates.is_empty() {
            return CollectStats { collected: 0 };
        }

        // STEP 1-3: deduce_unreachable → the cycle garbage.
        let unreachable = deduce_unreachable(py, candidates);
        if gc_trace_enabled() {
            eprintln!(
                "molt gc: deduce_unreachable unreachable={}",
                unreachable.len()
            );
        }
        if unreachable.is_empty() {
            return CollectStats { collected: 0 };
        }
        // Pin the entire set before the first callback/finalizer. Arbitrary
        // re-entry may release edges between later members; no raw candidate
        // pointer may become dangling before the resurrection partition.
        pin_unreachable(&unreachable);

        // move_legacy_finalizers / move_legacy_finalizer_reachable: NO-OP (molt has no
        // legacy tp_del; every __del__ is PEP-442 tp_finalize-class). gc.garbage stays
        // empty. Their POSITION — before handle_weakrefs — is why weakref clearing runs
        // next.

        // STEP (handle_weakrefs): clear weakrefs into the unreachable set + fire the
        // surviving callbacks, BEFORE any finalizer runs. STRICTLY precedes finalizers.
        crate::object::weakref::weakref_handle_cycle_unreachable(py, &unreachable, |wr_ptr| {
            header_is_collecting(wr_ptr)
        });

        // STEP (finalize_garbage): run each unreachable object's __del__ ONCE, in
        // unreachable-list order. The finalizer may resurrect (re-root) an object.
        for &ptr in &unreachable {
            run_finalizer_once(py, ptr);
        }

        // STEP (handle_resurrected_objects): re-run deduce_unreachable over the
        // post-finalization set. Anything a __del__ resurrected (now reachable / rc
        // explained by an external ref) leaves the collectable set. MANDATORY — frees
        // only what is STILL unreachable, never a resurrected object.
        //
        // Clear COLLECTING on the current unreachable set first (deduce_unreachable
        // re-marks from scratch). A weakref callback may have freed some members; the
        // tracked-registry membership re-probe drops those.
        for &ptr in &unreachable {
            header_set_collecting(ptr, false);
        }
        let still_tracked = unreachable.clone();
        let final_unreachable = deduce_unreachable(py, still_tracked);
        if final_unreachable.is_empty() {
            release_unreachable_pins(py, &unreachable);
            return CollectStats { collected: 0 };
        }
        let final_keys: HashSet<u64> = final_unreachable.iter().map(|ptr| addr_key(*ptr)).collect();
        let resurrected: Vec<*mut u8> = unreachable
            .iter()
            .copied()
            .filter(|ptr| !final_keys.contains(&addr_key(*ptr)))
            .collect();
        release_unreachable_pins(py, &resurrected);

        // The final set is confirmed garbage. Count it as collected (CPython's
        // `m += gc_list_size(&final_unreachable)`).
        let collected = final_unreachable.len();
        if gc_trace_enabled() {
            eprintln!("molt gc: delete_garbage collected={collected}");
        }

        // STEP (delete_garbage): clear each still-unreachable object IN PLACE. The
        // dec_ref cascade collapses the cycle. The whole set has remained pinned since
        // before weakref callbacks/finalizers; we clear all, then release the pins, letting RC drive
        // each member to its real free through the normal dealloc cascade (which also
        // `gc_untrack_on_free`s it). This is molt's analogue of CPython holding the
        // gc_list as the pin across `delete_garbage`.
        for &ptr in &final_unreachable {
            header_set_collecting(ptr, false);
        }
        for &ptr in &final_unreachable {
            molt_clear(py, ptr);
        }
        release_unreachable_pins(py, &final_unreachable);

        CollectStats { collected }
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
        let flags = (*header).flags;
        if !object_class_has_finalizer(ptr) {
            return;
        }
        if (flags & HEADER_FLAG_FINALIZER_RAN) != 0 {
            return;
        }
        crate::object::maybe_run_object_finalizer_for_cycle(py, ptr);
    }
}

/// Reentrancy flag for `collect_cycles` (CPython `gcstate->collecting`).
static GC_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct GcRunningGuard;
impl Drop for GcRunningGuard {
    fn drop(&mut self) {
        GC_RUNNING.store(false, AtomicOrdering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEALLOC_COUNT;
    use crate::object::builders::alloc_list;
    use crate::object::dec_ref_bits;
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

    #[test]
    fn exception_self_cycle_with_physical_abi_projection_is_collectible() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
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
        let _guard = crate::TEST_MUTEX.lock().unwrap();
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
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Force-enable the alloc/dealloc counters so DEALLOC_COUNT is a live signal
        // (otherwise `profile_hit` is a no-op and the deallocation is invisible to the
        // counter, though `gc_is_tracked` below remains an unconditional proof).
        // SAFETY: single-threaded test serialized by TEST_MUTEX.
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

    /// Negative case: a cycle that is STILL REACHABLE from a live external root must
    /// NOT be collected (no false reclamation). `outer` holds `a`, and `a -> b -> a`
    /// is a cycle, but `outer` keeps it alive.
    #[test]
    fn collect_spares_externally_reachable_cycle() {
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test serialized by TEST_MUTEX.
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
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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
