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
//!   1. `update_refs`:   snapshot each tracked object's refcount into a transient
//!                       `gc_refs` map; mark it COLLECTING.
//!   2. `subtract_refs`: for each tracked object, `traverse` its children and
//!                       decrement `gc_refs` of every child that is itself in the
//!                       candidate set. After this, `gc_refs > 0` ⟺ the object is
//!                       referenced from OUTSIDE the candidate set (a root);
//!                       `gc_refs == 0` ⟺ a cycle candidate.
//!   3. `move_unreachable`: BFS from the roots (`gc_refs > 0`). A root re-marks all
//!                       its transitive referents reachable (`gc_refs := 1`). The
//!                       objects still at `gc_refs == 0` after the BFS are the
//!                       unreachable cycle garbage.
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
//! EXPOSED-PROVENANCE address (Miri strict-provenance clean — NEVER the shared cold
//! header, whose `cold_idx` is shared across sibling instances and would cross-
//! corrupt). The COLLECTING bit lives in the FREE bits of the header `flags` word.
//!
//! ## MayFormCycle (the GREEN bit)
//!
//! The acyclic majority pays ZERO. A type that cannot transitively hold a reference
//! cycle (int/float/bool/str/bytes/None and the runtime's leaf types) is GREEN: it is
//! never registered in the tracked set, never scanned, never `clear`ed. Only the
//! cycle-forming container types — user instances (`TYPE_ID_OBJECT`), `dict`, `list`,
//! `tuple`, `set` — are tracked.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Mutex, OnceLock};

use crate::object::layout::{seq_vec_ptr, seq_vec_ref};
use crate::object::{
    HEADER_FLAG_FINALIZER_RAN, HEADER_FLAG_INSTANCE_HAS_FINALIZER, PtrSlot, TYPE_ID_DICT,
    TYPE_ID_FROZENSET, TYPE_ID_LIST, TYPE_ID_OBJECT, TYPE_ID_SET, TYPE_ID_TUPLE, dec_ref_ptr,
    header_from_obj_ptr, instance_dict_bits, object_class_bits, object_type_id,
};
use crate::{MoltObject, PyToken, obj_from_bits};

/// `flags`-word bit (25, after `HEADER_FLAG_HAS_WEAKREF = 1 << 24`) marking an
/// object as being in the CURRENT collection's candidate set. The bit is
/// transient and never observed outside a single stop-the-world `collect_cycles`
/// call.
pub(crate) const HEADER_FLAG_GC_COLLECTING: u32 = 1 << 25;

/// Side registry of live cycle-capable container objects (CPython's gc-tracked
/// generations, adapted: a flat set, since molt collection is stop-the-world and
/// non-generational in v1). Keyed by `PtrSlot` (the raw object pointer with the
/// runtime's `Send`/`Sync` discipline). Populated at allocation of a non-GREEN
/// container, removed at free. GREEN/atomic objects are never inserted.
///
/// This is its OWN structure, not the provenance pointer registry — the latter is
/// populated only in debug builds (`from_ptr` skips `register_ptr` in release), so
/// it cannot enumerate live objects in the shipped profile.
struct TrackedRegistry {
    set: HashSet<PtrSlot>,
}

fn tracked_registry() -> &'static Mutex<TrackedRegistry> {
    static REGISTRY: OnceLock<Mutex<TrackedRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(TrackedRegistry {
            set: HashSet::new(),
        })
    })
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
/// Tracked (non-GREEN) set, v1: user instances + the resizable ref containers that
/// are the canonical Python cycle formers. `tuple` is included because a tuple can
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
    if let Ok(mut reg) = tracked_registry().lock() {
        reg.set.insert(PtrSlot(ptr));
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
    if let Ok(mut reg) = tracked_registry().lock() {
        reg.set.remove(&PtrSlot(ptr));
    }
}

/// Is this object currently in the tracked set? Backs `gc.is_tracked`.
///
/// # Safety
/// `ptr` is treated as an opaque key; not dereferenced.
pub(crate) unsafe fn gc_is_tracked(ptr: *mut u8) -> bool {
    tracked_registry()
        .lock()
        .map(|reg| reg.set.contains(&PtrSlot(ptr)))
        .unwrap_or(false)
}

/// Drop the entire tracked set without touching the objects. Used at runtime
/// teardown AFTER the heap has been reclaimed, so the static does not dangle into
/// the next embedded runtime instance.
pub(crate) fn gc_reset_registry() {
    if let Ok(mut reg) = tracked_registry().lock() {
        reg.set.clear();
    }
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
        match type_id {
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                let vec_ptr = seq_vec_ptr(ptr);
                if vec_ptr.is_null() {
                    return;
                }
                for &bits in seq_vec_ref(ptr).iter() {
                    if let Some(child) = obj_from_bits(bits).as_ptr() {
                        visit(child);
                    }
                }
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
        match type_id {
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                // Tuples are immutable to Python, but a cyclic tuple's backing is ours
                // to clear during collection (CPython's tuple participates in cycle
                // breaking). Detach the elements FIRST, then dec-ref, so a re-entrant
                // dec_ref (a child's `__del__`, or the cascade reaching back into this
                // container) sees an empty container, never a stale element.
                let vec_ptr = seq_vec_ptr(ptr);
                if vec_ptr.is_null() {
                    return;
                }
                let detached: Vec<u64> = std::mem::take(&mut *vec_ptr);
                for bits in detached {
                    crate::dec_ref_bits(py, bits);
                }
            }
            TYPE_ID_DICT => {
                let order_ptr = crate::builtins::containers::dict_order_ptr(ptr);
                let table_ptr = crate::builtins::containers::dict_table_ptr(ptr);
                let hashes_ptr = crate::builtins::containers::dict_hashes_ptr(ptr);
                if !order_ptr.is_null() {
                    let detached: Vec<u64> = std::mem::take(&mut *order_ptr);
                    for bits in detached {
                        crate::dec_ref_bits(py, bits);
                    }
                }
                // Empty the index/hash side-tables so the dict is a valid empty dict.
                if !table_ptr.is_null() {
                    (*table_ptr).clear();
                }
                if !hashes_ptr.is_null() {
                    (*hashes_ptr).clear();
                }
            }
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let order_ptr = crate::builtins::containers::set_order_ptr(ptr);
                let table_ptr = crate::builtins::containers::set_table_ptr(ptr);
                let hashes_ptr = crate::builtins::containers::set_hashes_ptr(ptr);
                if !order_ptr.is_null() {
                    let detached: Vec<u64> = std::mem::take(&mut *order_ptr);
                    for bits in detached {
                        crate::dec_ref_bits(py, bits);
                    }
                }
                if !table_ptr.is_null() {
                    (*table_ptr).clear();
                }
                if !hashes_ptr.is_null() {
                    (*hashes_ptr).clear();
                }
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
            let rc = header_refcount(ptr) as isize;
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
        let candidates: Vec<*mut u8> = match tracked_registry().lock() {
            Ok(reg) => reg.set.iter().map(|slot| slot.0).collect(),
            Err(_) => return CollectStats { collected: 0 },
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
        let still_tracked: Vec<*mut u8> = match tracked_registry().lock() {
            Ok(reg) => unreachable
                .iter()
                .copied()
                .filter(|p| reg.set.contains(&PtrSlot(*p)))
                .collect(),
            Err(_) => return CollectStats { collected: 0 },
        };
        if still_tracked.is_empty() {
            return CollectStats { collected: 0 };
        }
        let final_unreachable = deduce_unreachable(py, still_tracked);
        if final_unreachable.is_empty() {
            return CollectStats { collected: 0 };
        }

        // The final set is confirmed garbage. Count it as collected (CPython's
        // `m += gc_list_size(&final_unreachable)`).
        let collected = final_unreachable.len();
        if gc_trace_enabled() {
            eprintln!("molt gc: delete_garbage collected={collected}");
        }

        // STEP (delete_garbage): clear each still-unreachable object IN PLACE. The
        // dec_ref cascade collapses the cycle. We first PIN every member with an extra
        // refcount so a mid-cascade free of one member cannot free another we still
        // need to clear; then we clear all; then we release the pins, letting RC drive
        // each member to its real free through the normal dealloc cascade (which also
        // `gc_untrack_on_free`s it). This is molt's analogue of CPython holding the
        // gc_list as the pin across `delete_garbage`.
        for &ptr in &final_unreachable {
            let header = header_from_obj_ptr(ptr);
            (*header).ref_count.fetch_add(1, AtomicOrdering::Relaxed);
            header_set_collecting(ptr, false);
        }
        for &ptr in &final_unreachable {
            molt_clear(py, ptr);
        }
        for &ptr in &final_unreachable {
            dec_ref_ptr(py, ptr);
        }

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
        if (flags & HEADER_FLAG_INSTANCE_HAS_FINALIZER) == 0 {
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
}
