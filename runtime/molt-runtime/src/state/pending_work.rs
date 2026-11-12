//! Runtime-scoped eval breaker. Sources publish their own work before setting
//! a bit; this summary never owns callbacks, objects, or their lifetimes.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum PendingWorkKind {
    Signals = 1,
    GarbageCollection = 2,
    PoolRetirement = 4,
    PendingCalls = 8,
    /// A thread waited a switch interval for the GIL (CPython
    /// `_PY_GIL_DROP_REQUEST_BIT`); the holder's next poll hands it over.
    GilDropRequest = 16,
}

/// One aligned word, shared by a runtime's producers and lent to compiled code
/// by its execution record. The execution lease retains its runtime and hence
/// this allocation across callbacks, isolate switches and shutdown.
#[repr(transparent)]
pub(crate) struct PendingWork(AtomicU32);

impl PendingWork {
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Async-signal-safe; callers must publish the source before notifying.
    #[inline]
    pub(crate) fn request(&self, kind: PendingWorkKind) {
        self.0.fetch_or(kind as u32, Ordering::Release);
    }

    /// Call only after the service attempt, then recheck the source *after*
    /// clearing. A producer before the RMW is acquired here and found by the
    /// source recheck; a producer after it either is found or sets the bit
    /// itself. Never replace this with a load followed by a summary store.
    pub(crate) fn serviced(&self, kind: PendingWorkKind, remains: impl FnOnce() -> bool) {
        self.0.fetch_and(!(kind as u32), Ordering::AcqRel);
        if remains() {
            self.request(kind);
        }
    }

    /// Clear `kind` and report whether it was requested. For a request with
    /// no source to recheck: a producer after the RMW sets the bit again.
    pub(crate) fn take(&self, kind: PendingWorkKind) -> bool {
        self.0.fetch_and(!(kind as u32), Ordering::AcqRel) & kind as u32 != 0
    }

    pub(crate) fn word_ptr(&self) -> *const AtomicU32 {
        &self.0
    }
}

const _: () = {
    assert!(
        std::mem::size_of::<PendingWork>()
            == molt_codegen_abi::THREAD_RECORD_PENDING_WORK_WORD_BYTES as usize
    );
    assert!(std::mem::align_of::<PendingWork>() >= 4);
};

#[cfg(test)]
mod tests;
