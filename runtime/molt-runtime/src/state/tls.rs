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
    /// The executing Python frame's record base, or zero
    /// (`THREAD_RECORD_FRAME_TOP_OFFSET`, `builtins::frames::records`).
    pub(crate) frame_top: Cell<u64>,
    /// The first free byte and the end of the current frame-arena chunk
    /// (`THREAD_RECORD_ARENA_TOP_OFFSET`, `THREAD_RECORD_ARENA_LIMIT_OFFSET`).
    /// Compiled code bumps `arena_top` at an inline frame push; the runtime
    /// owns the chunk list (`builtins::frames::records`).
    pub(crate) arena_top: Cell<u64>,
    pub(crate) arena_limit: Cell<u64>,
    /// The refcount admission compiled code reads at entry
    /// (`THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET`), recomputed by every setter
    /// of its inputs below.
    pub(crate) refcount_admission: Cell<u64>,
    /// Whether runtime profile counters are on (`THREAD_RECORD_PROFILE_ENABLED_OFFSET`).
    pub(crate) profile_enabled: Cell<u64>,
    /// The Python recursion depth this thread's execution stack holds
    /// (`THREAD_RECORD_RECURSION_DEPTH_OFFSET`, `state::recursion`). Compiled
    /// calls charge it inline (`molt_codegen_abi::thread_record`).
    pub(crate) recursion_depth: Cell<u64>,
    /// Address of the bound runtime's recursion limit, or zero
    /// (`THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET`); published and cleared
    /// with `bound_state`.
    pub(crate) recursion_limit_ptr: Cell<u64>,
    /// Borrowed from the outermost terminal release until its queue drains
    /// (`THREAD_RECORD_TERMINAL_RELEASE_OFFSET`): compiled inline frees read
    /// it. A pointer Cell has no TLS destructor, including during runtime
    /// teardown.
    pub(crate) terminal_release: Cell<*const crate::object::bounded_release::TerminalReleaseState>,
    /// The bound runtime's builtin `object` class, boxed, or zero
    /// (`THREAD_RECORD_OBJECT_CLASS_OFFSET`); published with `bound_state`
    /// and when the builtin classes initialize.
    pub(crate) object_class: Cell<u64>,
    /// The bound runtime's GC record (`THREAD_RECORD_GC_RECORD_OFFSET`), or
    /// zero; published with `bound_state`.
    pub(crate) gc_record: Cell<u64>,
    /// The address of the bound runtime's missing-singleton slot
    /// (`THREAD_RECORD_MISSING_SLOT_OFFSET`), or zero.
    pub(crate) missing_slot: Cell<u64>,
    /// GIL custody depth held by this thread.
    pub(crate) gil_depth: AdmissionInput<usize>,
    /// Runtime execution lease depth, with the service-role bit.
    pub(crate) lease_depth: AdmissionInput<usize>,
    /// Shutdown-drain execution custody: 0 or 1.
    pub(crate) shutdown_drain_depth: AdmissionInput<u8>,
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
    pub(crate) attached_context: molt_codegen_abi::AttachedExecutionContextStorage,
    pub(crate) cext_execution_depth: AdmissionInput<u32>,
    pub(crate) compiled_depth: Cell<usize>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) wasm_execution_depth: AdmissionInput<usize>,
    pub(crate) execution_identity: Cell<u64>,
    pub(crate) compiled_scope_top: Cell<u64>,
    pub(crate) compiled_public_dependencies: Cell<usize>,
    pub(crate) compiled_host_scopes:
        std::mem::ManuallyDrop<RefCell<Vec<crate::concurrency::execution::CompiledScopeFrame>>>,
    pub(crate) persistent_execution_owner: Cell<u64>,
    pub(crate) persistent_execution_owns_gil: Cell<bool>,
    pub(crate) public_gil_depth: Cell<u32>,
    pub(crate) public_gil_owns_persistent: Cell<bool>,
    /// The per-thread exception state (`builtins::exceptions::state`), here so
    /// a runtime path that holds the execution context reaches it with no
    /// thread-local access. No field has a destructor.
    pub(crate) thread_last_exception: crate::builtins::exceptions::state::ThreadExceptionState,
    pub(crate) exception_handler_depth: Cell<usize>,
    pub(crate) exception_stack_baseline: Cell<usize>,
    pub(crate) active_exception_stack: std::mem::ManuallyDrop<RefCell<Vec<u64>>>,
    pub(crate) active_exception_fallback: std::mem::ManuallyDrop<
        RefCell<Vec<crate::builtins::exceptions::state::ExceptionContextFallback>>,
    >,
    pub(crate) generator_raise: Cell<bool>,
    pub(crate) task_raise_active: Cell<bool>,
    /// Explicit mutex and suspended-GIL ownership. Its initialized storage has
    /// no automatic destructor; the GIL authority consumes every acquisition.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) native_gil: crate::concurrency::gil::GilMutexCustody,
}

const _: () = {
    use molt_codegen_abi::{
        THREAD_RECORD_ARENA_LIMIT_OFFSET, THREAD_RECORD_ARENA_TOP_OFFSET,
        THREAD_RECORD_EXCEPTION_PENDING_OFFSET, THREAD_RECORD_FRAME_TOP_OFFSET,
        THREAD_RECORD_OBJECT_CLASS_OFFSET, THREAD_RECORD_PENDING_INVOCATION_OFFSET,
        THREAD_RECORD_PENDING_WORK_PTR_OFFSET, THREAD_RECORD_PROFILE_ENABLED_OFFSET,
        THREAD_RECORD_RECURSION_DEPTH_OFFSET, THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET,
        THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET, THREAD_RECORD_TERMINAL_RELEASE_OFFSET,
    };
    use molt_codegen_abi::{THREAD_RECORD_GC_RECORD_OFFSET, THREAD_RECORD_MISSING_SLOT_OFFSET};
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
    assert!(offset_of!(ExecutionTls, arena_top) as i32 == THREAD_RECORD_ARENA_TOP_OFFSET);
    assert!(offset_of!(ExecutionTls, arena_limit) as i32 == THREAD_RECORD_ARENA_LIMIT_OFFSET);
    assert!(
        offset_of!(ExecutionTls, refcount_admission) as i32
            == THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, profile_enabled) as i32 == THREAD_RECORD_PROFILE_ENABLED_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, recursion_depth) as i32 == THREAD_RECORD_RECURSION_DEPTH_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, recursion_limit_ptr) as i32
            == THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET
    );
    assert!(
        offset_of!(ExecutionTls, terminal_release) as i32 == THREAD_RECORD_TERMINAL_RELEASE_OFFSET
    );
    assert!(offset_of!(ExecutionTls, object_class) as i32 == THREAD_RECORD_OBJECT_CLASS_OFFSET);
    assert!(offset_of!(ExecutionTls, gc_record) as i32 == THREAD_RECORD_GC_RECORD_OFFSET);
    assert!(offset_of!(ExecutionTls, missing_slot) as i32 == THREAD_RECORD_MISSING_SLOT_OFFSET);
};

/// An input of the refcount admission word. Any code reads it; only the
/// `ExecutionTls` setters below write it, and each recomputes the word, so the
/// word compiled code loads at entry is current by construction.
pub(crate) struct AdmissionInput<T: Copy>(Cell<T>);

impl<T: Copy> AdmissionInput<T> {
    const fn new(value: T) -> Self {
        Self(Cell::new(value))
    }

    #[inline]
    pub(crate) fn get(&self) -> T {
        self.0.get()
    }
}

impl ExecutionTls {
    #[inline]
    pub(crate) fn set_gil_depth(&self, depth: usize) {
        self.gil_depth.0.set(depth);
        self.refresh_refcount_admission();
    }

    #[inline]
    pub(crate) fn set_lease_depth(&self, depth: usize) {
        self.lease_depth.0.set(depth);
        self.refresh_refcount_admission();
    }

    #[inline]
    pub(crate) fn set_shutdown_drain_depth(&self, depth: u8) {
        self.shutdown_drain_depth.0.set(depth);
        self.refresh_refcount_admission();
    }

    #[cfg(target_arch = "wasm32")]
    #[inline]
    pub(crate) fn set_wasm_execution_depth(&self, depth: usize) {
        self.wasm_execution_depth.0.set(depth);
        self.refresh_refcount_admission();
    }

    #[inline]
    pub(crate) fn set_cext_execution_depth(&self, depth: u32) {
        self.cext_execution_depth.0.set(depth);
        self.refresh_refcount_admission();
    }

    /// Plain inline refcounts need the full execution attachment, lifecycle
    /// lease and GIL with transition diagnostics off; atomic builds never use
    /// them (`inline_refcount::CompiledRefcountAdmission`).
    fn refresh_refcount_admission(&self) {
        use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission;
        let admission = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC
            || !crate::concurrency::execution::runtime_execution_custody_in(self)
            || self.cext_execution_depth.get() == 0
            || crate::object::rc_transition_trace_enabled()
            || (cfg!(debug_assertions) && molt_obj_model::trace_ptr_registry())
        {
            CompiledRefcountAdmission::Runtime
        } else if cfg!(debug_assertions) {
            CompiledRefcountAdmission::AttachedFrameOwners
        } else {
            CompiledRefcountAdmission::AttachedPlain
        };
        self.refcount_admission.set(admission as u64);
    }
}

thread_local! {
    static EXECUTION_TLS: ExecutionTls = const {
        ExecutionTls {
            exception_pending: Cell::new(false),
            pending_invocation: Cell::new(0),
            pending_work_ptr: Cell::new(0),
            frame_top: Cell::new(0),
            arena_top: Cell::new(0),
            arena_limit: Cell::new(0),
            refcount_admission: Cell::new(0),
            profile_enabled: Cell::new(0),
            recursion_depth: Cell::new(0),
            recursion_limit_ptr: Cell::new(0),
            gil_depth: AdmissionInput::new(0),
            lease_depth: AdmissionInput::new(0),
            shutdown_drain_depth: AdmissionInput::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            retained_worker_origin: Cell::new(None),
            runtime_state: Cell::new(std::ptr::null_mut()),
            bound_state: Cell::new(std::ptr::null()),
            tls_guard_touched: Cell::new(false),
            terminal_release: Cell::new(std::ptr::null()),
            object_class: Cell::new(0),
            gc_record: Cell::new(0),
            missing_slot: Cell::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            native_gil: crate::concurrency::gil::GilMutexCustody::new(),
            attached_context: molt_codegen_abi::AttachedExecutionContextStorage::new(),
            cext_execution_depth: AdmissionInput::new(0),
            compiled_depth: Cell::new(0),
            #[cfg(target_arch = "wasm32")]
            wasm_execution_depth: AdmissionInput::new(0),
            execution_identity: Cell::new(0),
            compiled_scope_top: Cell::new(0),
            compiled_public_dependencies: Cell::new(0),
            compiled_host_scopes: std::mem::ManuallyDrop::new(RefCell::new(Vec::new())),
            persistent_execution_owner: Cell::new(0),
            persistent_execution_owns_gil: Cell::new(false),
            public_gil_depth: Cell::new(0),
            public_gil_owns_persistent: Cell::new(false),
            thread_last_exception:
                crate::builtins::exceptions::state::ThreadExceptionState::new(),
            exception_handler_depth: Cell::new(0),
            exception_stack_baseline: Cell::new(0),
            active_exception_stack: std::mem::ManuallyDrop::new(RefCell::new(Vec::new())),
            active_exception_fallback: std::mem::ManuallyDrop::new(RefCell::new(Vec::new())),
            generator_raise: Cell::new(false),
            task_raise_active: Cell::new(false),
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
    pub(crate) static REPR_STACK: std::mem::ManuallyDrop<RefCell<Vec<PtrSlot>>> = const { std::mem::ManuallyDrop::new(RefCell::new(Vec::new())) };
    pub(crate) static REPR_SET: std::mem::ManuallyDrop<RefCell<HashSet<PtrSlot>>> = std::mem::ManuallyDrop::new(RefCell::new(HashSet::new()));
    pub(crate) static REPR_DEPTH: Cell<usize> = const { Cell::new(0) };
}
