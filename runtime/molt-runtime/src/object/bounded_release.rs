//! Per-thread custody for terminal releases, before callbacks or detachment.
//!
//! The active state lives on the outermost release's stack. The execution TLS
//! record only borrows it; it has no destructor and remains usable throughout
//! thread/runtime teardown. No mutable borrow survives a Python callback.
//!
//! Order. CPython releases a dying graph recursively: each release runs with
//! everything it releases in turn before its parent's next release. From 3.13
//! on it defers a release only at the C stack margin (`_Py_Dealloc`,
//! `Py_TRASHCAN_BEGIN`), which no bounded program reaches. Molt bounds the
//! stack depth of nested releases instead ([`RELEASE_DEPTH_LIMIT`]) and keeps
//! CPython's order past the bound: a release at the bound resumes the
//! releases its body deferred, first to last, each with the releases it
//! deferred in turn, at one stack level (a preorder walk). CPython 3.12 also
//! counts the nesting of its trashcan deallocators and deposits a release at
//! `_PyTrash_UNWIND_LEVEL` (50) until the outermost one ends; a 3.12 target
//! keeps that count over the same logical (preorder) nesting.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use super::{
    HEADER_FLAG_SCOPED, ObjectAuxSnapshot, TYPE_ID_CONTEXT_BITMAP_NODE,
    TYPE_ID_CONTEXT_COLLISION_NODE, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_FILTER,
    TYPE_ID_FROZENSET, TYPE_ID_LIST, TYPE_ID_OBJECT, TYPE_ID_SET, TYPE_ID_TRACEBACK_PAYLOAD,
    TYPE_ID_TUPLE, gc,
};
use crate::PyToken;

#[cfg(test)]
#[path = "bounded_release_tests.rs"]
mod tests;

// A segment gets 1/8 of the 1 MiB WASM linear stack. Native release-output
// retained frames total 2112 bytes/level; WASM shadow frames total 912 bytes/level.
// Round up with dispatch/callback headroom to 8 KiB per level, hence 16 levels.
// Family K in findings_prior_art.md owns the derivation and qualified cells.
// This is independent of the Python recursion limit.
const RELEASE_SEGMENT_STACK_BYTES: usize = 128 * 1024;
const RELEASE_LEVEL_STACK_BYTES: usize = 8 * 1024;
pub(super) const RELEASE_DEPTH_LIMIT: usize =
    RELEASE_SEGMENT_STACK_BYTES / RELEASE_LEVEL_STACK_BYTES;

/// CPython 3.12's `_PyTrash_UNWIND_LEVEL` (`Objects/object.c`).
const TRASHCAN_UNWIND_LEVEL: usize = 50;

/// Owns the already-performed zero transition. Resumption must call the
/// terminal body directly, never perform a second reference-count decrement.
pub(super) struct TerminalRelease {
    pub(super) ptr: NonNull<u8>,
    pub(super) type_id: u32,
    pub(super) header_flags: u32,
    pub(super) header_size_class: u16,
    pub(super) header_aux: ObjectAuxSnapshot,
}

/// A zero-reference object whose release waits.
struct HeldRelease {
    release: TerminalRelease,
    restore_tracking: bool,
}

/// One entry of the stack-bound worklist.
enum Pending {
    /// A release the stack bound deferred, and the trashcan nesting its
    /// release starts from (read on 3.12 targets only).
    Release {
        held: HeldRelease,
        trash_nesting: usize,
    },
    /// 3.12: the trashcan end of a resumed release, after the releases it
    /// deferred.
    TrashcanEnd,
}

pub(crate) struct TerminalReleaseState {
    depth: Cell<usize>,
    pending: RefCell<Vec<Pending>>,
    /// The target counts trashcan nesting (CPython 3.12).
    trashcan: bool,
    trash_nesting: Cell<usize>,
    /// 3.12 trashcan deposits, released last first when the outermost
    /// trashcan release ends (`_PyTrash_thread_destroy_chain`).
    trash: RefCell<Vec<HeldRelease>>,
}

impl TerminalReleaseState {
    fn new(trashcan: bool) -> Self {
        Self {
            depth: Cell::new(0),
            pending: RefCell::new(Vec::new()),
            trashcan,
            trash_nesting: Cell::new(0),
            trash: RefCell::new(Vec::new()),
        }
    }

    /// No state or worklist borrow crosses terminal callbacks. Reentrant
    /// releases see the same thread's depth and append to its worklist.
    #[inline(always)]
    unsafe fn release(&self, py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
        // Caller-owned storage must finish before its scope returns. Its
        // children still enter this authority and only heap storage may wait.
        if self.depth.get() >= RELEASE_DEPTH_LIMIT && release.header_flags & HEADER_FLAG_SCOPED == 0
        {
            unsafe { self.defer(py, release) };
            return None;
        }
        unsafe { self.enter(py, release) }
    }

    /// Run one release in a new stack frame of this authority.
    #[inline(always)]
    unsafe fn enter(&self, py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
        let level = self.depth.get() + 1;
        self.depth.set(level);
        let _frame = ReleaseFrame(self);
        if self.trashcan {
            return unsafe { self.enter_counted(py, release, level) };
        }
        if level < RELEASE_DEPTH_LIMIT {
            // Every heap release this body starts runs inline.
            return unsafe { run_body(py, release) };
        }
        unsafe { self.run_at_bound(py, release) }
    }

    /// [`Self::enter`] for a target that counts trashcan nesting (3.12).
    #[cold]
    #[inline(never)]
    unsafe fn enter_counted(
        &self,
        py: &PyToken<'_>,
        release: TerminalRelease,
        level: usize,
    ) -> Option<u64> {
        let counted = uses_trashcan(release.type_id);
        if counted {
            let nesting = self.trash_nesting.get();
            if nesting >= TRASHCAN_UNWIND_LEVEL {
                let held = unsafe { hold(py, release) };
                self.trash.borrow_mut().push(held);
                return None;
            }
            self.trash_nesting.set(nesting + 1);
        }
        let successor = if level < RELEASE_DEPTH_LIMIT {
            unsafe { run_body(py, release) }
        } else {
            unsafe { self.run_at_bound(py, release) }
        };
        if counted {
            unsafe { self.trashcan_end(py) };
        }
        successor
    }

    /// A body at the bound defers every heap release it starts. Resume them
    /// in CPython's recursive order (module docs) before this frame returns:
    /// the worklist is a stack, so reversing what one body deferred puts its
    /// first deferred release on top.
    #[cold]
    #[inline(never)]
    unsafe fn run_at_bound(&self, py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
        let base = self.pending.borrow().len();
        let successor = unsafe { run_body(py, release) };
        let nesting = self.trash_nesting.get();
        self.pending.borrow_mut()[base..].reverse();
        loop {
            let next = {
                let mut pending = self.pending.borrow_mut();
                if pending.len() == base {
                    break;
                }
                pending.pop()
            };
            match next {
                Some(Pending::Release {
                    held,
                    trash_nesting,
                }) => unsafe { self.resume(py, held, trash_nesting) },
                Some(Pending::TrashcanEnd) => unsafe { self.trashcan_end(py) },
                None => unreachable!("terminal release worklist shrank below its frame"),
            }
        }
        self.trash_nesting.set(nesting);
        successor
    }

    /// Run a deferred release at the current stack level. The releases its
    /// body defers land on top of the worklist, ahead of its siblings.
    unsafe fn resume(&self, py: &PyToken<'_>, held: HeldRelease, trash_nesting: usize) {
        let release = unsafe { reclaim(py, held) };
        let mut pending_end = false;
        if self.trashcan {
            self.trash_nesting.set(trash_nesting);
            if uses_trashcan(release.type_id) {
                if trash_nesting >= TRASHCAN_UNWIND_LEVEL {
                    let held = unsafe { hold(py, release) };
                    self.trash.borrow_mut().push(held);
                    return;
                }
                self.trash_nesting.set(trash_nesting + 1);
                pending_end = true;
            }
        }
        let mark = {
            let mut pending = self.pending.borrow_mut();
            if pending_end {
                // The trashcan end waits below the releases this body defers.
                reserve_one(&mut pending);
                pending.push(Pending::TrashcanEnd);
            }
            pending.len()
        };
        if let Some(next_tee_link) = unsafe { run_body(py, release) } {
            // Preserve tee's iterative successor path. The transferred edge
            // has not had its refcount decremented yet; at this depth its
            // release waits as the body's last one.
            crate::dec_ref_bits(py, next_tee_link);
        }
        self.pending.borrow_mut()[mark..].reverse();
    }

    /// `_PyTrash_end`: the outermost trashcan release releases the deposits.
    unsafe fn trashcan_end(&self, py: &PyToken<'_>) {
        let nesting = self.trash_nesting.get() - 1;
        self.trash_nesting.set(nesting);
        if nesting > 0 || self.trash.borrow().is_empty() {
            return;
        }
        // `_PyTrash_thread_destroy_chain`: each deposit runs at nesting 1, so
        // a deposit made meanwhile joins this chain.
        self.trash_nesting.set(1);
        loop {
            let next = self.trash.borrow_mut().pop();
            let Some(held) = next else {
                break;
            };
            let release = unsafe { reclaim(py, held) };
            if let Some(next_tee_link) = unsafe { self.enter(py, release) } {
                crate::dec_ref_bits(py, next_tee_link);
            }
        }
        self.trash_nesting.set(0);
    }

    #[cold]
    #[inline(never)]
    unsafe fn defer(&self, py: &PyToken<'_>, release: TerminalRelease) {
        let held = unsafe { hold(py, release) };
        let mut pending = self.pending.borrow_mut();
        reserve_one(&mut pending);
        pending.push(Pending::Release {
            held,
            trash_nesting: self.trash_nesting.get(),
        });
    }
}

/// The kinds whose CPython 3.12 deallocator opens the trashcan
/// (`Py_TRASHCAN_BEGIN`): list, tuple, dict, set and frozenset, heap-class
/// instances (`subtype_dealloc`), exceptions, tracebacks, `filter` and
/// context (HAMT) nodes.
#[inline]
fn uses_trashcan(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | TYPE_ID_TUPLE
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_FROZENSET
            | TYPE_ID_OBJECT
            | TYPE_ID_EXCEPTION
            | TYPE_ID_TRACEBACK_PAYLOAD
            | TYPE_ID_FILTER
            | TYPE_ID_CONTEXT_BITMAP_NODE
            | TYPE_ID_CONTEXT_COLLISION_NODE
    )
}

fn reserve_one<T>(items: &mut Vec<T>) {
    if items.try_reserve(1).is_err() {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<T>());
    }
}

/// A pending zero-reference object still owns its intact payload. It must
/// not appear as collectible trash during a sibling's finalizer. Keep
/// allocation accounting until actual retirement.
unsafe fn hold(py: &PyToken<'_>, release: TerminalRelease) -> HeldRelease {
    let ptr = release.ptr.as_ptr();
    let restore_tracking = unsafe { gc::gc_is_tracked(ptr) };
    if restore_tracking {
        unsafe {
            gc::gc_untrack(
                py,
                ptr,
                release.type_id,
                gc::GcUntrackReason::ExplicitControl,
            )
        };
    }
    HeldRelease {
        release,
        restore_tracking,
    }
}

unsafe fn reclaim(py: &PyToken<'_>, held: HeldRelease) -> TerminalRelease {
    let HeldRelease {
        mut release,
        restore_tracking,
    } = held;
    let ptr = release.ptr.as_ptr();
    if restore_tracking {
        assert!(unsafe { gc::gc_track_existing(py, ptr) });
    }
    // A sibling callback may have published a weakref or C view while this
    // object waited. Immutable allocation/layout facts stay with terminal
    // custody, but lifecycle flags are always read afresh.
    release.header_flags = unsafe { (*super::header_from_obj_ptr(ptr)).load_synchronized_flags() };
    release
}

#[inline(always)]
unsafe fn run_body(py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
    unsafe {
        super::dec_ref_ptr_terminal_body(
            py,
            release.ptr.as_ptr(),
            release.type_id,
            release.header_flags,
            release.header_size_class,
            release.header_aux,
        )
    }
}

struct ReleaseFrame<'a>(&'a TerminalReleaseState);

impl Drop for ReleaseFrame<'_> {
    fn drop(&mut self) {
        self.0.depth.set(
            self.0
                .depth
                .get()
                .checked_sub(1)
                .expect("terminal release depth underflow"),
        );
    }
}

/// The one managed terminal entry, used by every generated heap kind and by
/// ordinary, collector-pin, C-view and runtime-teardown owner releases.
#[inline(always)]
pub(super) unsafe fn release(py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
    let active = &py.execution_tls().terminal_release;
    let state = active.get();
    if !state.is_null() {
        // SAFETY: only the current thread can read this Cell. Its outermost
        // release owns the state until its worklists are empty.
        return unsafe { (*state).release(py, release) };
    }
    unsafe { release_outermost(py, release, active) }
}

/// Only a root release initializes the state. Keep its stack frame and setup
/// out of the nested shallow path, where the body is still hot.
#[inline(never)]
unsafe fn release_outermost(
    py: &PyToken<'_>,
    release: TerminalRelease,
    active: &Cell<*const TerminalReleaseState>,
) -> Option<u64> {
    let state = TerminalReleaseState::new(crate::runtime_state(py).gc.counts_trashcan_nesting());
    active.set(&state);
    let _active = std::mem::DropGuard::new(active, |active| {
        active.set(std::ptr::null());
    });
    let mut successor = None;
    super::terminal_resource_drop_no_unwind(|| unsafe {
        successor = state.enter(py, release);
    });
    debug_assert_eq!(state.depth.get(), 0);
    debug_assert_eq!(state.trash_nesting.get(), 0);
    debug_assert!(state.pending.borrow().is_empty());
    debug_assert!(state.trash.borrow().is_empty());
    successor
}
