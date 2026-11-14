//! Per-thread custody for terminal releases, before callbacks or detachment.
//!
//! The active state lives on the outermost release's stack. The execution TLS
//! record only borrows it; it has no destructor and remains usable throughout
//! thread/runtime teardown. No mutable borrow survives a Python callback.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use super::{HEADER_FLAG_SCOPED, ObjectAuxSnapshot, gc};
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

/// Owns the already-performed zero transition. Resumption must call the
/// terminal body directly, never perform a second reference-count decrement.
pub(super) struct TerminalRelease {
    pub(super) ptr: NonNull<u8>,
    pub(super) type_id: u32,
    pub(super) header_flags: u32,
    pub(super) header_size_class: u16,
    pub(super) header_aux: ObjectAuxSnapshot,
}

struct DeferredRelease {
    release: TerminalRelease,
    restore_tracking: bool,
}

#[derive(Default)]
pub(crate) struct TerminalReleaseState {
    depth: Cell<usize>,
    pending: RefCell<Vec<DeferredRelease>>,
}

impl TerminalReleaseState {
    /// No state or worklist borrow crosses terminal callbacks. Reentrant
    /// releases see the same thread's depth and append to its LIFO worklist.
    #[inline(always)]
    unsafe fn release(&self, py: &PyToken<'_>, release: TerminalRelease) -> Option<u64> {
        let depth = self.depth.get();
        // Caller-owned storage must finish before its scope returns. Its
        // children still enter this authority and only heap storage may wait.
        if depth >= RELEASE_DEPTH_LIMIT && release.header_flags & HEADER_FLAG_SCOPED == 0 {
            unsafe { self.defer(py, release) };
            return None;
        }
        self.depth.set(depth + 1);
        let _frame = ReleaseFrame(self);
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

    #[cold]
    #[inline(never)]
    unsafe fn defer(&self, py: &PyToken<'_>, release: TerminalRelease) {
        let ptr = release.ptr.as_ptr();
        let mut pending = self.pending.borrow_mut();
        if pending.try_reserve(1).is_err() {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<DeferredRelease>());
        }
        // A pending zero-reference object still owns its intact payload.
        // It must not appear as collectible trash during a sibling's
        // finalizer. Keep allocation accounting until actual retirement.
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
        pending.push(DeferredRelease {
            release,
            restore_tracking,
        });
    }

    unsafe fn drain(&self, py: &PyToken<'_>) {
        loop {
            let next = self.pending.borrow_mut().pop();
            let Some(mut next) = next else {
                break;
            };
            let ptr = next.release.ptr.as_ptr();
            if next.restore_tracking {
                assert!(unsafe { gc::gc_track_existing(py, ptr) });
            }
            // A sibling callback may have published a weakref or C view while
            // this object waited. Immutable allocation/layout facts stay with
            // terminal custody, but lifecycle flags are always read afresh.
            next.release.header_flags =
                unsafe { (*super::header_from_obj_ptr(ptr)).load_synchronized_flags() };
            if let Some(next_tee_link) = unsafe { self.release(py, next.release) } {
                // Preserve tee's existing iterative successor fast path. The
                // transferred edge has not had its refcount decremented yet.
                crate::dec_ref_bits(py, next_tee_link);
            }
        }
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
        // release owns the state until the worklist has been drained.
        return unsafe { (*state).release(py, release) };
    }
    unsafe { release_outermost(py, release, active) }
}

/// Only a root release initializes/drains the worklist. Keep its stack frame
/// and setup out of the nested shallow path, where the body is still hot.
#[inline(never)]
unsafe fn release_outermost(
    py: &PyToken<'_>,
    release: TerminalRelease,
    active: &Cell<*const TerminalReleaseState>,
) -> Option<u64> {
    let state = TerminalReleaseState::default();
    active.set(&state);
    let _active = std::mem::DropGuard::new(active, |active| {
        active.set(std::ptr::null());
    });
    let mut successor = None;
    super::terminal_resource_drop_no_unwind(|| unsafe {
        successor = state.release(py, release);
        state.drain(py);
    });
    debug_assert_eq!(state.depth.get(), 0);
    debug_assert!(state.pending.borrow().is_empty());
    successor
}
