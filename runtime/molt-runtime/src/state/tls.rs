use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use crate::PtrSlot;
use crate::builtins::frames::FrameEntry;
use crate::resource::TempArena;
use crate::state::runtime_state::RuntimeState;

/// The per-thread execution facts every runtime entry reads, in one
/// thread-local. Compiled code enters the runtime on every operation, and on
/// macOS each `thread_local!` key costs its own `_tlv_get_addr` call; one key
/// makes an entry, its drop and its runtime-state resolution one lookup.
/// Every field is a plain `Cell` and the struct has no destructor, so access
/// never fails, even while other thread-locals are destroyed.
pub(crate) struct ExecutionTls {
    /// GIL custody depth held by this thread.
    pub(crate) gil_depth: Cell<usize>,
    /// Runtime execution lease depth, with the service-role bit.
    pub(crate) lease_depth: Cell<usize>,
    /// Shutdown-drain execution custody: 0 or 1.
    pub(crate) shutdown_drain_depth: Cell<u8>,
    /// The worker origin a retained shutdown worker holds.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) retained_worker_origin:
        Cell<Option<crate::state::runtime_state::RuntimeWorkerOrigin>>,
    /// The runtime state this thread initialized or attached to.
    pub(crate) runtime_state: Cell<*mut RuntimeState>,
    /// Whether this thread ordered its destructor-bearing thread-locals.
    pub(crate) tls_guard_touched: Cell<bool>,
}

thread_local! {
    static EXECUTION_TLS: ExecutionTls = const {
        ExecutionTls {
            gil_depth: Cell::new(0),
            lease_depth: Cell::new(0),
            shutdown_drain_depth: Cell::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            retained_worker_origin: Cell::new(None),
            runtime_state: Cell::new(std::ptr::null_mut()),
            tls_guard_touched: Cell::new(false),
        }
    };
}

/// This thread's execution facts.
#[inline(always)]
pub(crate) fn execution_tls() -> &'static ExecutionTls {
    // SAFETY: the key has a const initializer and no destructor, so its
    // storage lives as long as this thread and access never fails. The
    // struct holds `Cell`s, so it is not `Sync` and the reference cannot leave
    // this thread.
    EXECUTION_TLS.with(|tls| unsafe { &*std::ptr::from_ref(tls) })
}

thread_local! {
    pub(crate) static PARSE_ARENA: RefCell<TempArena> = RefCell::new(TempArena::new(8 * 1024));
    pub(crate) static CONTEXT_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static FRAME_STACK: RefCell<Vec<FrameEntry>> = const { RefCell::new(Vec::new()) };
    pub(crate) static TRACE_FRAME_PUSH_STACK: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    pub(crate) static REPR_STACK: RefCell<Vec<PtrSlot>> = const { RefCell::new(Vec::new()) };
    pub(crate) static REPR_SET: RefCell<HashSet<PtrSlot>> = RefCell::new(HashSet::new());
    pub(crate) static REPR_DEPTH: Cell<usize> = const { Cell::new(0) };
    pub(crate) static TRACEBACK_SUPPRESS: Cell<usize> = const { Cell::new(0) };
}
