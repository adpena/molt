use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use crate::PtrSlot;
use crate::resource::TempArena;
use crate::state::runtime_state::RuntimeState;

/// The per-thread execution facts every runtime entry reads, in one
/// thread-local. Compiled code enters the runtime on every operation, and on
/// macOS each `thread_local!` key costs its own `_tlv_get_addr` call; one key
/// makes an entry, its drop and its runtime-state resolution one lookup.
/// The record has no destructor, so access
/// never fails, even while other thread-locals are destroyed.
///
/// It is also the thread record compiled code holds, as CPython's code holds
/// its `PyThreadState`: the leading fields have the fixed layout
/// `molt_codegen_abi::THREAD_RECORD_*` declares, and compiled code reads and
/// writes them with plain loads and stores at the address
/// the attached context lends.
#[repr(C)]
pub(crate) struct ExecutionTls {
    /// An exception is pending in the active execution context
    /// (`THREAD_RECORD_EXCEPTION_PENDING_OFFSET`).
    pub(crate) exception_pending: Cell<bool>,
    /// The callable an invoker hands to the next compiled frame entry, borrowed
    /// for the call (`THREAD_RECORD_PENDING_INVOCATION_OFFSET`,
    /// `builtins::frames::namespace`).
    pub(crate) pending_invocation: Cell<u64>,
    /// Borrowed eval-breaker address, published and cleared with bound_state
    /// at runtime custody transitions, including runtime-originated callbacks.
    pub(crate) pending_work_ptr: Cell<u64>,
    /// The executing Python frame's record, or zero
    /// (`THREAD_RECORD_FRAME_TOP_OFFSET`, `builtins::frames::records`).
    pub(crate) frame_top: Cell<u64>,
    /// How many records the frame chain holds.
    pub(crate) frame_depth: Cell<usize>,
    /// The Python recursion depth this thread's execution stack holds
    /// (`state::recursion`).
    pub(crate) recursion_depth: Cell<usize>,
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
    /// The runtime state this thread's admission custody runs, resolved once
    /// when the custody begins (`state::runtime_state::bind_execution_state`),
    /// or null. Null is always safe: readers then resolve in full.
    pub(crate) bound_state: Cell<*const RuntimeState>,
    /// Whether this thread ordered its destructor-bearing thread-locals.
    pub(crate) tls_guard_touched: Cell<bool>,
    /// Borrowed from the outermost terminal release until its queue drains.
    /// A pointer Cell has no TLS destructor, including during runtime teardown.
    pub(crate) terminal_release: Cell<*const crate::object::bounded_release::TerminalReleaseState>,
    pub(crate) attached_context: molt_codegen_abi::AttachedExecutionContextStorage,
    pub(crate) cext_execution_depth: Cell<u32>,
    pub(crate) compiled_depth: Cell<usize>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) wasm_execution_depth: Cell<usize>,
    pub(crate) execution_identity: Cell<u64>,
    pub(crate) compiled_scope_top: Cell<u64>,
    pub(crate) compiled_public_dependencies: Cell<usize>,
    pub(crate) compiled_host_scopes:
        std::mem::ManuallyDrop<RefCell<Vec<crate::concurrency::execution::CompiledScopeFrame>>>,
    pub(crate) persistent_execution_owner: Cell<u64>,
    pub(crate) persistent_execution_owns_gil: Cell<bool>,
    pub(crate) public_gil_depth: Cell<u32>,
    pub(crate) public_gil_owns_persistent: Cell<bool>,
    /// Explicit mutex and suspended-GIL ownership. Its initialized storage has
    /// no automatic destructor; the GIL authority consumes every acquisition.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) native_gil: crate::concurrency::gil::GilMutexCustody,
}

const _: () = {
    use molt_codegen_abi::{
        THREAD_RECORD_EXCEPTION_PENDING_OFFSET, THREAD_RECORD_FRAME_TOP_OFFSET,
        THREAD_RECORD_PENDING_INVOCATION_OFFSET, THREAD_RECORD_PENDING_WORK_PTR_OFFSET,
    };
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Cell<bool>>() == 1);
    assert!(!std::mem::needs_drop::<ExecutionTls>());
    assert!(
        offset_of!(ExecutionTls, exception_pending) as i32
            == THREAD_RECORD_EXCEPTION_PENDING_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, pending_invocation) as i32
            == THREAD_RECORD_PENDING_INVOCATION_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, pending_work_ptr) as i32 == THREAD_RECORD_PENDING_WORK_PTR_OFFSET
    );
    assert!(offset_of!(ExecutionTls, frame_top) as i32 == THREAD_RECORD_FRAME_TOP_OFFSET);
};

thread_local! {
    static EXECUTION_TLS: ExecutionTls = const {
        ExecutionTls {
            exception_pending: Cell::new(false),
            pending_invocation: Cell::new(0),
            pending_work_ptr: Cell::new(0),
            frame_top: Cell::new(0),
            frame_depth: Cell::new(0),
            recursion_depth: Cell::new(0),
            gil_depth: Cell::new(0),
            lease_depth: Cell::new(0),
            shutdown_drain_depth: Cell::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            retained_worker_origin: Cell::new(None),
            runtime_state: Cell::new(std::ptr::null_mut()),
            bound_state: Cell::new(std::ptr::null()),
            tls_guard_touched: Cell::new(false),
            terminal_release: Cell::new(std::ptr::null()),
            #[cfg(not(target_arch = "wasm32"))]
            native_gil: crate::concurrency::gil::GilMutexCustody::new(),
            attached_context: molt_codegen_abi::AttachedExecutionContextStorage::new(),
            cext_execution_depth: Cell::new(0),
            compiled_depth: Cell::new(0),
            #[cfg(target_arch = "wasm32")]
            wasm_execution_depth: Cell::new(0),
            execution_identity: Cell::new(0),
            compiled_scope_top: Cell::new(0),
            compiled_public_dependencies: Cell::new(0),
            compiled_host_scopes: std::mem::ManuallyDrop::new(RefCell::new(Vec::new())),
            persistent_execution_owner: Cell::new(0),
            persistent_execution_owns_gil: Cell::new(false),
            public_gil_depth: Cell::new(0),
            public_gil_owns_persistent: Cell::new(false),
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

/// Lend this thread's record to compiled code, which takes it once per
/// invocation and then reads and writes its `THREAD_RECORD_*` fields with
/// plain loads and stores (the pending-exception byte of every inline
/// exception check, the invocation handoff of every direct call). Never null;
/// the record lives as long as the thread.
#[unsafe(no_mangle)]
pub extern "C" fn molt_attached_context() -> u64 {
    let tls = execution_tls();
    super::runtime_state::bind_execution_state(tls);
    assert!(
        !tls.bound_state.get().is_null(),
        "compiled thread record requires an attached runtime"
    );
    unsafe { crate::PyToken::with_custody(tls) }.attached_context()
}

thread_local! {
    pub(crate) static PARSE_ARENA: std::mem::ManuallyDrop<RefCell<TempArena>> = std::mem::ManuallyDrop::new(RefCell::new(TempArena::new(8 * 1024)));
    pub(crate) static CONTEXT_STACK: std::mem::ManuallyDrop<RefCell<Vec<u64>>> = const { std::mem::ManuallyDrop::new(RefCell::new(Vec::new())) };
    pub(crate) static TRACE_FRAME_PUSH_STACK: std::mem::ManuallyDrop<RefCell<Vec<bool>>> = const { std::mem::ManuallyDrop::new(RefCell::new(Vec::new())) };
    pub(crate) static REPR_STACK: std::mem::ManuallyDrop<RefCell<Vec<PtrSlot>>> = const { std::mem::ManuallyDrop::new(RefCell::new(Vec::new())) };
    pub(crate) static REPR_SET: std::mem::ManuallyDrop<RefCell<HashSet<PtrSlot>>> = std::mem::ManuallyDrop::new(RefCell::new(HashSet::new()));
    pub(crate) static REPR_DEPTH: Cell<usize> = const { Cell::new(0) };
}
