use crate::BuiltinExc;
use crate::PyToken;
#[cfg(test)]
use crate::missing_bits;
use crate::runtime_flags::{self, RuntimeFlag};

use molt_codegen_abi::GENERATOR_RESUME_RETURNED;
use molt_obj_model::MoltObject;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::generators_async::molt_future_new;
use crate::builtins::exceptions::{ActivationExceptionScope, ActivationKind};
use crate::object::accessors::resolve_obj_ptr;
use crate::object::{
    HEADER_FLAG_COROUTINE, ObjectAuxPreselection, object_init_poll_fn_unpublished,
    object_init_shape_unpublished, object_init_state_unpublished, object_state,
    task_shape_for_poll_fn,
};
use crate::{
    ASYNCGEN_CONTROL_SIZE, ASYNCGEN_FINALIZER_OFFSET, ASYNCGEN_FIRSTITER_OFFSET,
    ASYNCGEN_GEN_OFFSET, ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT, ASYNCGEN_OP_ASEND,
    ASYNCGEN_OP_ATHROW, ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET, GEN_CONTROL_SIZE,
    GEN_SEND_OFFSET, GEN_STATE_OFFSET, GEN_THROW_OFFSET, GEN_YIELD_FROM_OFFSET,
    HEADER_FLAG_CORO_RUNNING, HEADER_FLAG_CORO_STARTED, HEADER_FLAG_TASK_DONE, MoltHeader,
    TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, TYPE_ID_ASYNC_GENERATOR,
    TYPE_ID_GENERATOR, TYPE_ID_OBJECT, TYPE_ID_TUPLE, alloc_exception, alloc_object,
    alloc_object_with_aux, alloc_tuple, asyncgen_poll_fn_addr, call_callable1, call_poll_fn,
    clear_exception, current_task_ptr, dec_ref_bits, exception_class_bits,
    exception_clear_reason_set, exception_pending, generator_raise_active, header_from_obj_ptr,
    inc_ref_bits, maybe_ptr_from_bits, molt_exception_clear, molt_exception_last_pending,
    molt_exception_set_last, molt_raise, obj_from_bits, object_mark_has_ptrs, object_type_id,
    pending_bits_i64, ptr_from_bits, raise_exception, register_task_execution, resolve_task_ptr,
    runtime_state, set_generator_raise, to_i64, token_id_from_bits, type_name,
};

pub(crate) fn promise_trace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_PROMISE)
}

pub(crate) fn sleep_trace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_SLEEP)
}

#[cfg(molt_net)]
pub(crate) fn asyncio_connect_trace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_ASYNCIO_CONNECT)
}

#[inline]
pub(crate) fn debug_current_task() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_CURRENT_TASK)
}

unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    unsafe { ptr.add(offset) as *mut u64 }
}

/// Store an owned edge slot of the control region (send, throw, yield-from).
unsafe fn generator_set_slot(_py: &PyToken<'_>, ptr: *mut u8, offset: usize, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, offset, bits);
    }
}

/// A stateful activation's lifecycle, ordered as CPython's frame state
/// (`Include/internal/pycore_frame.h`, `gi_frame_state`): every state at or
/// above `Completed` is terminal and never left.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FrameState {
    /// Made and never entered.
    Created = 0,
    /// Stopped at a yield or an await; its frame survives.
    Suspended = 1,
    /// Its body runs; no other resume may enter it.
    Executing = 2,
    /// Its body returned or raised, or it closed.
    Completed = 3,
    /// Cycle collection detached its frame.
    Cleared = 4,
}

impl FrameState {
    #[inline]
    fn from_word(word: u64) -> Self {
        match word {
            0 => Self::Created,
            1 => Self::Suspended,
            2 => Self::Executing,
            3 => Self::Completed,
            4 => Self::Cleared,
            _ => corrupt_generator_state(word),
        }
    }

    #[inline]
    pub(crate) fn is_terminal(self) -> bool {
        self >= Self::Completed
    }
}

#[cold]
#[inline(never)]
fn corrupt_generator_state(word: u64) -> ! {
    eprintln!("molt fatal: generator state word holds {word:#x}");
    std::process::abort()
}

/// The generator's state word: a raw atomic in the control region, never a
/// boxed value and never an edge (`GEN_STATE_OFFSET`).
#[inline]
unsafe fn generator_state_word<'a>(ptr: *mut u8) -> &'a AtomicU64 {
    unsafe { &*(ptr.add(GEN_STATE_OFFSET) as *const AtomicU64) }
}

/// The one lifecycle authority of a generator.
#[inline]
pub(crate) unsafe fn generator_state(ptr: *mut u8) -> FrameState {
    FrameState::from_word(unsafe { generator_state_word(ptr) }.load(AtomicOrdering::Acquire))
}

/// The frame state of a stateful activation: a generator's state word, or a
/// native coroutine's projection of its execution flags and task completion.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task.
pub(crate) unsafe fn activation_frame_state(ptr: *mut u8) -> FrameState {
    unsafe {
        if object_type_id(ptr) == TYPE_ID_GENERATOR {
            return generator_state(ptr);
        }
        let flags = (*header_from_obj_ptr(ptr)).load_synchronized_flags();
        if flags & HEADER_FLAG_TASK_DONE != 0 {
            FrameState::Completed
        } else if flags & HEADER_FLAG_CORO_RUNNING != 0 {
            FrameState::Executing
        } else if flags & HEADER_FLAG_CORO_STARTED != 0 {
            FrameState::Suspended
        } else {
            FrameState::Created
        }
    }
}

pub(crate) unsafe fn generator_closed(ptr: *mut u8) -> bool {
    unsafe { generator_state(ptr).is_terminal() }
}

pub(crate) unsafe fn generator_running(ptr: *mut u8) -> bool {
    unsafe { generator_state(ptr) == FrameState::Executing }
}

pub(crate) unsafe fn generator_started(ptr: *mut u8) -> bool {
    unsafe { generator_state(ptr) != FrameState::Created }
}

pub(crate) unsafe fn generator_suspended(ptr: *mut u8) -> bool {
    unsafe { generator_state(ptr) == FrameState::Suspended }
}

/// The terminal transition. The first one hands the frame's bindings to a
/// frame object that shares them, or releases them; `started` says whether
/// the body ever ran.
unsafe fn generator_complete(py: &PyToken<'_>, ptr: *mut u8, started: bool) {
    unsafe {
        crate::gil_assert();
        let previous = generator_state_word(ptr)
            .fetch_max(FrameState::Completed as u64, AtomicOrdering::AcqRel);
        if !FrameState::from_word(previous).is_terminal() {
            crate::builtins::frames::activation_exit_bindings(py, ptr, started);
        }
    }
}

/// Cycle collection detaches the frame: the state becomes `Cleared` and no
/// resume or finalizer re-enters the body. Bindings retire with the edges.
pub(crate) unsafe fn generator_clear(ptr: *mut u8) {
    unsafe {
        generator_state_word(ptr).fetch_max(FrameState::Cleared as u64, AtomicOrdering::AcqRel);
    }
}

/// Put a generator in `state` directly, as a body or the runtime would.
#[cfg(test)]
pub(crate) unsafe fn generator_set_state_for_test(ptr: *mut u8, state: FrameState) {
    unsafe { generator_state_word(ptr).store(state as u64, AtomicOrdering::Release) }
}

/// Why a resume did not enter the body.
enum EntryRefusal {
    Executing,
    Terminal,
}

/// Execution custody of one entry into a generator body, taken with one
/// compare-and-swap (CPython's free-threaded `gen_send_ex2`). Dropping it
/// restores the state it entered from if the body never ran, and otherwise
/// suspends the generator unless the body completed.
struct GeneratorExecution {
    ptr: *mut u8,
    from: FrameState,
    ran: bool,
}

impl GeneratorExecution {
    unsafe fn enter(ptr: *mut u8) -> Result<Self, EntryRefusal> {
        let word = unsafe { generator_state_word(ptr) };
        let mut current = word.load(AtomicOrdering::Acquire);
        loop {
            match FrameState::from_word(current) {
                from @ (FrameState::Created | FrameState::Suspended) => {
                    match word.compare_exchange_weak(
                        current,
                        FrameState::Executing as u64,
                        AtomicOrdering::AcqRel,
                        AtomicOrdering::Acquire,
                    ) {
                        Ok(_) => {
                            return Ok(Self {
                                ptr,
                                from,
                                ran: false,
                            });
                        }
                        Err(actual) => current = actual,
                    }
                }
                FrameState::Executing => return Err(EntryRefusal::Executing),
                FrameState::Completed | FrameState::Cleared => {
                    return Err(EntryRefusal::Terminal);
                }
            }
        }
    }

    /// The body is about to run.
    fn run(&mut self) {
        self.ran = true;
    }
}

impl Drop for GeneratorExecution {
    fn drop(&mut self) {
        let after = if self.ran {
            FrameState::Suspended
        } else {
            self.from
        };
        // A completed body already left Executing; that state stays.
        let _ = unsafe { generator_state_word(self.ptr) }.compare_exchange(
            FrameState::Executing as u64,
            after as u64,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        );
    }
}

pub(crate) unsafe fn generator_yieldfrom_bits(ptr: *mut u8) -> u64 {
    unsafe { *generator_slot_ptr(ptr, GEN_YIELD_FROM_OFFSET) }
}

pub(crate) fn resolve_sleep_target(py: &PyToken<'_>, future_ptr: *mut u8) -> *mut u8 {
    super::scheduler::await_chain_terminal(py, future_ptr).unwrap_or(std::ptr::null_mut())
}

unsafe fn generator_pending_throw(ptr: *mut u8) -> bool {
    unsafe {
        let bits = *generator_slot_ptr(ptr, GEN_THROW_OFFSET);
        !obj_from_bits(bits).is_none()
    }
}

/// What one resume of a generator body produced. Each value is owned.
pub(crate) enum GeneratorStep {
    /// The body yielded the value; the generator is suspended.
    Yielded(u64),
    /// The body returned the value; the generator is completed.
    Returned(u64),
    /// The body of an async generator awaits a pending future.
    Pending,
}

/// `Err` means an exception is pending.
pub(crate) type GeneratorResult = Result<GeneratorStep, ()>;

pub(crate) fn generator_done_tuple(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    let done_bits = MoltObject::from_bool(true).bits();
    let tuple_ptr = alloc_tuple(_py, &[value_bits, done_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

/// Project a step onto the `(value, done)` pair ABI that compiled `gen_send`,
/// `gen_throw` and the yield-from delegation read.
fn generator_step_pair(py: &PyToken<'_>, result: GeneratorResult) -> u64 {
    let (value, done) = match result {
        Err(()) => return MoltObject::none().bits(),
        Ok(GeneratorStep::Pending) => return pending_bits_i64() as u64,
        Ok(GeneratorStep::Yielded(value)) => (value, false),
        Ok(GeneratorStep::Returned(value)) => (value, true),
    };
    let tuple = alloc_tuple(py, &[value, MoltObject::from_bool(done).bits()]);
    dec_ref_bits(py, value);
    if tuple.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple).bits()
    }
}

pub(crate) unsafe fn raise_stop_iteration_from_value(py: &PyToken<'_>, value: u64) -> u64 {
    let values = if obj_from_bits(value).is_none() {
        &[][..]
    } else {
        std::slice::from_ref(&value)
    };
    let args = alloc_tuple(py, values);
    if args.is_null() {
        return MoltObject::none().bits();
    }
    let args = MoltObject::from_ptr(args).bits();
    let exception = crate::builtins::exceptions::molt_exception_new_from_class(
        exception_class_bits(py, BuiltinExc::StopIteration),
        args,
    );
    dec_ref_bits(py, args);
    if exception_pending(py) {
        dec_ref_bits(py, exception);
        return MoltObject::none().bits();
    }
    let raised = molt_raise(exception);
    dec_ref_bits(py, exception);
    raised
}

/// Project a step onto the method protocol: `send()`, `throw()` and
/// `__next__` return a yielded value and raise `StopIteration(value)` for a
/// returned one.
pub(crate) fn generator_step_value(py: &PyToken<'_>, result: GeneratorResult) -> u64 {
    match result {
        Err(()) => MoltObject::none().bits(),
        Ok(GeneratorStep::Pending) => pending_bits_i64() as u64,
        Ok(GeneratorStep::Yielded(value)) => value,
        Ok(GeneratorStep::Returned(value)) => {
            let raised = unsafe { raise_stop_iteration_from_value(py, value) };
            dec_ref_bits(py, value);
            raised
        }
    }
}
/// Allocate every task kind with explicitly selected code and namespace custody.
fn task_new_with_context(
    _py: &PyToken<'_>,
    poll_fn_addr: u64,
    closure_size: u64,
    kind_bits: u64,
    context: [u64; 3],
) -> u64 {
    let Some(closure_size) = crate::provenance::abi::address(closure_size) else {
        return raise_exception::<_>(
            _py,
            BuiltinExc::MemoryError,
            "task closure size exceeds the active address space",
        );
    };
    let trace_alloc = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_GENERATOR_ALLOC);
    if trace_alloc {
        eprintln!(
            "molt_task_new enter poll_fn=0x{:x} closure_size={} kind={}",
            poll_fn_addr, closure_size, kind_bits
        );
    }
    let (type_id, is_coroutine) = match kind_bits {
        TASK_KIND_FUTURE => (TYPE_ID_OBJECT, false),
        TASK_KIND_COROUTINE => (TYPE_ID_OBJECT, true),
        TASK_KIND_GENERATOR => (TYPE_ID_GENERATOR, false),
        _ => {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "unknown task kind");
        }
    };
    if type_id == TYPE_ID_GENERATOR && closure_size < GEN_CONTROL_SIZE {
        return raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "generator task closure too small",
        );
    }
    let Some(total_size) = std::mem::size_of::<MoltHeader>().checked_add(closure_size) else {
        return raise_exception::<_>(
            _py,
            BuiltinExc::MemoryError,
            "task allocation size overflow",
        );
    };
    let ptr = if type_id == TYPE_ID_OBJECT {
        alloc_object_with_aux(_py, total_size, type_id, ObjectAuxPreselection::Sidecar)
    } else {
        // Generator kinds select a sidecar from their type authority.
        alloc_object(_py, total_size, type_id)
    };
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let slots = closure_size / std::mem::size_of::<u64>();
        if slots > 0 {
            let payload_ptr = ptr as *mut u64;
            for idx in 0..slots {
                *payload_ptr.add(idx) = MoltObject::none().bits();
            }
        }
        let header = header_from_obj_ptr(ptr);
        // Compiled task constructors populate tagged capture slots directly
        // after this call. Their layout permits heap owners even while the
        // current words are None; exclude non-owning field fast paths before
        // any backend publishes captures into the payload.
        crate::object::object_mark_has_ptrs(_py, ptr);
        if !object_init_poll_fn_unpublished(ptr, poll_fn_addr)
            || !object_init_shape_unpublished(ptr, task_shape_for_poll_fn(poll_fn_addr))
            || !object_init_state_unpublished(ptr, 0)
            || !crate::object::aux_header::object_init_frame_context_unpublished(
                _py, ptr, context[0], context[1], context[2],
            )
        {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            if !crate::exception_pending(_py) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::MemoryError,
                    "task context allocation failed",
                );
            }
            return MoltObject::none().bits();
        }
        if is_coroutine {
            (*header).fetch_or_flags(HEADER_FLAG_COROUTINE);
        }
        if type_id == TYPE_ID_GENERATOR && closure_size >= GEN_CONTROL_SIZE {
            *generator_slot_ptr(ptr, GEN_SEND_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_THROW_OFFSET) = MoltObject::none().bits();
            // The raw control words start their lifetimes before publication.
            std::ptr::write(
                ptr.add(GEN_STATE_OFFSET).cast::<AtomicU64>(),
                AtomicU64::new(FrameState::Created as u64),
            );
            for offset in crate::GEN_RESERVED_OFFSETS {
                *generator_slot_ptr(ptr, offset) = 0;
            }
        }
    }
    if !crate::builtins::exceptions::task_raised_slot_admit(_py, ptr) {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        return MoltObject::none().bits();
    }
    if trace_alloc {
        eprintln!(
            "molt_task_new ok ptr=0x{:x} bits=0x{:x} type_id={}",
            ptr as usize,
            MoltObject::from_ptr(ptr).bits(),
            type_id
        );
    }
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_task_new(poll_fn_addr: u64, closure_size: u64, kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pending =
            crate::builtins::frames::acquire_pending_invocation_context(_py, poll_fn_addr);
        // Generated callable trampolines carry the exact invocation context.
        // Runtime-native tasks have no Python code owner; ambient caller frames
        // are not a substitute for an actual callable handoff.
        let context = pending.unwrap_or([0; 3]);
        let result = task_new_with_context(_py, poll_fn_addr, closure_size, kind_bits, context);
        if let Some(context) = pending {
            for bits in context {
                dec_ref_bits(_py, bits);
            }
        }
        result
    })
}

/// Native coroutine identity is a runtime fact, never a public attribute.
pub(crate) fn is_native_coroutine_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && ((*header_from_obj_ptr(ptr)).load_metadata_flags() & HEADER_FLAG_COROUTINE) != 0
    })
}

/// Internal poll future adapter admission is a physical runtime invariant.
/// A generator's poll function never grants the __await__ protocol.
pub(crate) fn is_native_poll_future_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && ((*header_from_obj_ptr(ptr)).load_metadata_flags() & HEADER_FLAG_COROUTINE) == 0
            && crate::object_class_bits(ptr) == 0
            && crate::object::object_poll_fn(ptr) != 0
    })
}

/// types.coroutine publishes its protocol flag on the captured code object.
/// Reading gi_code on an arbitrary object admits spoofed attributes and hooks.
pub(crate) fn is_iterable_coroutine_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return false;
        }
        let code = crate::object::aux_header::object_frame_code_bits(ptr);
        maybe_ptr_from_bits(code).is_some_and(|code| {
            object_type_id(code) == crate::TYPE_ID_CODE
                && (crate::object::layout::code_protocol_flags(code)
                    & crate::object::layout::CO_ITERABLE_COROUTINE)
                    != 0
        })
    })
}

pub(crate) fn is_native_python_awaitable_bits(bits: u64) -> bool {
    is_native_coroutine_bits(bits) || is_iterable_coroutine_bits(bits)
}

/// The async-generator operation awaitables (`async_generator_asend` and
/// `async_generator_athrow`) are classed native poll futures. Their poll
/// function is their identity; the class edge names them for Python.
pub(crate) fn is_asyncgen_awaitable_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && crate::object::object_poll_fn(ptr) == asyncgen_poll_fn_addr()
    })
}

/// `await` polls these futures directly: classless internal poll futures and
/// the async-generator operation awaitables, whose `__await__` returns self.
pub(crate) fn is_direct_await_future_bits(bits: u64) -> bool {
    is_native_poll_future_bits(bits) || is_asyncgen_awaitable_bits(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_native_awaitable(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(
            is_native_python_awaitable_bits(val_bits) || is_direct_await_future_bits(val_bits),
        )
        .bits()
    })
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
/// - `token_bits` must be an integer cancel token id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_task_register_execution(
    task_bits: u64,
    token_bits: u64,
    context_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "object is not awaitable");
        };
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "cancel token id must be int",
                );
            }
        };
        let context = if obj_from_bits(context_bits).is_none() {
            super::cancellation::TaskContextBinding::Inherited
        } else if crate::builtins::contextvars::is_context(context_bits) {
            super::cancellation::TaskContextBinding::Owned(context_bits)
        } else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "context must be a Context");
        };
        register_task_execution(_py, task_ptr, id, context);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let is_gen = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
        MoltObject::from_bool(is_gen).bits()
    })
}

/// The generator a protocol entry receives, or `TypeError`.
fn generator_receiver(py: &PyToken<'_>, bits: u64) -> Option<*mut u8> {
    match maybe_ptr_from_bits(bits) {
        Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_GENERATOR => Some(ptr),
        _ => {
            raise_exception::<()>(py, BuiltinExc::TypeError, "expected generator");
            None
        }
    }
}

/// Run the body once under `execution`, on the generator's own exception and
/// context-manager state (`ActivationExceptionScope`), and read how it stopped. The poll returns the yielded or
/// returned value itself; a completed body stored `GENERATOR_RESUME_RETURNED`
/// in its resume label first (the frontend's normal-return boundary). The
/// runtime turns that report into `FrameState::Completed` here, and nothing
/// else reads the label for lifecycle.
unsafe fn generator_run_body(
    py: &PyToken<'_>,
    ptr: *mut u8,
    mut execution: GeneratorExecution,
) -> GeneratorResult {
    unsafe {
        let poll_fn_addr = crate::object::object_poll_fn(ptr);
        if poll_fn_addr == 0 {
            // A body-less generator has nothing to run: it is exhausted.
            generator_complete(py, ptr, execution.from != FrameState::Created);
            return Ok(GeneratorStep::Returned(MoltObject::none().bits()));
        }
        let exceptions = ActivationExceptionScope::enter(py, ptr, ActivationKind::Generator);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        execution.run();
        let state_before = object_state(ptr);
        let result = call_poll_fn(py, poll_fn_addr, ptr) as u64;
        let state_after = object_state(ptr);
        set_generator_raise(prev_raise);
        if runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_GENERATOR_STATE) {
            eprintln!(
                "[molt generator resume] label_before={} label_after={} result_type={} result_bits=0x{:x}",
                state_before,
                state_after,
                type_name(py, obj_from_bits(result)),
                result
            );
        }
        let pending = exception_pending(py);
        let exc_bits = if pending {
            let bits = molt_exception_last_pending();
            clear_exception(py);
            bits
        } else {
            MoltObject::none().bits()
        };
        exceptions.exit(py);
        if pending {
            return generator_raise_from_pending(py, ptr, exc_bits);
        }
        if result as i64 == pending_bits_i64() {
            return Ok(GeneratorStep::Pending);
        }
        if state_after == GENERATOR_RESUME_RETURNED {
            generator_complete(py, ptr, true);
            return Ok(GeneratorStep::Returned(result));
        }
        Ok(GeneratorStep::Yielded(result))
    }
}

/// The body raised: the generator completes, and the exception propagates
/// with CPython's StopIteration conversion.
unsafe fn generator_raise_from_pending(
    py: &PyToken<'_>,
    ptr: *mut u8,
    exception: u64,
) -> GeneratorResult {
    unsafe {
        generator_complete(py, ptr, true);
        super::throw_protocol::raise_body_exception(py, ptr, exception);
    }
    Err(())
}

fn refuse_entry(py: &PyToken<'_>) -> GeneratorResult {
    raise_exception::<()>(py, BuiltinExc::ValueError, "generator already executing");
    Err(())
}

/// `send(value)`; `None` is `__next__` (CPython's `gen_send_ex2`).
pub(crate) unsafe fn generator_send_step(
    py: &PyToken<'_>,
    ptr: *mut u8,
    send_bits: u64,
) -> GeneratorResult {
    unsafe {
        if !obj_from_bits(send_bits).is_none() && generator_state(ptr) == FrameState::Created {
            raise_exception::<()>(
                py,
                BuiltinExc::TypeError,
                "can't send non-None value to a just-started generator",
            );
            return Err(());
        }
        let execution = match GeneratorExecution::enter(ptr) {
            Ok(execution) => execution,
            Err(EntryRefusal::Executing) => return refuse_entry(py),
            Err(EntryRefusal::Terminal) => {
                return Ok(GeneratorStep::Returned(MoltObject::none().bits()));
            }
        };
        generator_set_slot(py, ptr, GEN_SEND_OFFSET, send_bits);
        generator_set_slot(py, ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
        generator_run_body(py, ptr, execution)
    }
}

/// `throw()`: `arguments` is a throw carrier (an exception, a class, or a
/// delegated argument tuple).
pub(crate) unsafe fn generator_throw_step(
    py: &PyToken<'_>,
    ptr: *mut u8,
    arguments: u64,
) -> GeneratorResult {
    unsafe {
        // Normalize a leaf before activation: exception constructors and
        // subclass checks may legally resume or close this generator.
        // Re-read the state afterward. Delegated arguments stay raw in the
        // existing tuple carrier; an exception instance in the suspension
        // slot means direct injection into its continuation.
        let pending = if generator_closed(ptr)
            || !generator_started(ptr)
            || obj_from_bits(generator_yieldfrom_bits(ptr)).is_none()
        {
            let Some(exception) = super::throw_protocol::normalize_throw_argument(py, arguments)
            else {
                return Err(());
            };
            exception
        } else if maybe_ptr_from_bits(arguments)
            .is_some_and(|carrier| object_type_id(carrier) == TYPE_ID_TUPLE)
        {
            inc_ref_bits(py, arguments);
            arguments
        } else {
            let carrier = alloc_tuple(py, &[arguments]);
            if carrier.is_null() {
                return Err(());
            }
            MoltObject::from_ptr(carrier).bits()
        };
        let execution = match GeneratorExecution::enter(ptr) {
            Ok(execution) => execution,
            Err(EntryRefusal::Executing) => {
                dec_ref_bits(py, pending);
                return refuse_entry(py);
            }
            Err(EntryRefusal::Terminal) => {
                // A finished activation owns no frame; the exception simply
                // propagates from the caller.
                crate::molt_exception_trace_prepend(pending);
                molt_raise(pending);
                dec_ref_bits(py, pending);
                return Err(());
            }
        };
        if execution.from == FrameState::Created {
            // Raise at the created activation's first instruction: one real
            // target frame, entered and left without running the body, under
            // this call's execution custody.
            let frame = crate::builtins::frames::ActivationFrameScope::enter(py, ptr);
            generator_complete(py, ptr, false);
            let Ok(_frame) = frame else {
                dec_ref_bits(py, pending);
                return Err(());
            };
            crate::molt_exception_trace_prepend(pending);
            molt_raise(pending);
            dec_ref_bits(py, pending);
            return Err(());
        }
        generator_set_slot(py, ptr, GEN_THROW_OFFSET, pending);
        dec_ref_bits(py, pending);
        generator_set_slot(py, ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
        generator_run_body(py, ptr, execution)
    }
}

/// Resume with the send and throw slots as they stand (an async generator
/// re-polling its body or delivering a primed throw).
unsafe fn generator_resume_step(py: &PyToken<'_>, ptr: *mut u8) -> GeneratorResult {
    unsafe {
        match GeneratorExecution::enter(ptr) {
            Ok(execution) => generator_run_body(py, ptr, execution),
            Err(EntryRefusal::Executing) => refuse_entry(py),
            Err(EntryRefusal::Terminal) => Ok(GeneratorStep::Returned(MoltObject::none().bits())),
        }
    }
}

/// The compiled `gen_send` operation: the `(value, done)` pair projection.
#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_send(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = generator_receiver(py, gen_bits) else {
            return MoltObject::none().bits();
        };
        generator_step_pair(py, unsafe { generator_send_step(py, ptr, send_bits) })
    })
}

/// The compiled `gen_throw` operation: the `(value, done)` pair projection.
#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_throw(gen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = generator_receiver(py, gen_bits) else {
            return MoltObject::none().bits();
        };
        generator_step_pair(py, unsafe { generator_throw_step(py, ptr, exc_bits) })
    })
}

/// `close()` (CPython's `gen_close`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_close(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = generator_receiver(py, gen_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            match generator_state(ptr) {
                FrameState::Executing => {
                    return raise_exception::<_>(
                        py,
                        BuiltinExc::ValueError,
                        "generator already executing",
                    );
                }
                FrameState::Created => {
                    generator_complete(py, ptr, false);
                    return MoltObject::none().bits();
                }
                FrameState::Completed | FrameState::Cleared => {
                    return MoltObject::none().bits();
                }
                FrameState::Suspended => {}
            }
            let exit = alloc_exception(py, BuiltinExc::GeneratorExit, "");
            if exit.is_null() {
                return MoltObject::none().bits();
            }
            let exit = MoltObject::from_ptr(exit).bits();
            // Throw owns the activation, its execution custody and a delegated
            // close. Its iterator protocol consumes a delegated StopIteration
            // as a return, while the body boundary converts an escaping one.
            let step = generator_throw_step(py, ptr, exit);
            dec_ref_bits(py, exit);
            match step {
                Err(()) => {
                    let exception = molt_exception_last_pending();
                    if crate::builtins::exceptions::exception_matches_kind(
                        py,
                        exception,
                        crate::BuiltinExc::GeneratorExit,
                    ) {
                        clear_exception(py);
                        generator_complete(py, ptr, true);
                    }
                    dec_ref_bits(py, exception);
                    MoltObject::none().bits()
                }
                Ok(GeneratorStep::Yielded(value)) => {
                    dec_ref_bits(py, value);
                    raise_exception::<_>(
                        py,
                        BuiltinExc::RuntimeError,
                        "generator ignored GeneratorExit",
                    )
                }
                Ok(GeneratorStep::Returned(value)) => {
                    if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                        value
                    } else {
                        dec_ref_bits(py, value);
                        MoltObject::none().bits()
                    }
                }
                Ok(GeneratorStep::Pending) => {
                    generator_complete(py, ptr, true);
                    MoltObject::none().bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_next_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = generator_receiver(py, gen_bits) else {
            return MoltObject::none().bits();
        };
        generator_step_value(py, unsafe {
            generator_send_step(py, ptr, MoltObject::none().bits())
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_send_method(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = generator_receiver(py, gen_bits) else {
            return MoltObject::none().bits();
        };
        generator_step_value(py, unsafe { generator_send_step(py, ptr, send_bits) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_throw_method(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((receiver, arguments)) = super::throw_protocol::parse_throw_call(py, args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        let result = match generator_receiver(py, receiver) {
            Some(ptr) => {
                generator_step_value(py, unsafe { generator_throw_step(py, ptr, arguments) })
            }
            None => MoltObject::none().bits(),
        };
        dec_ref_bits(py, arguments);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_close_method(gen_bits: u64) -> u64 {
    molt_generator_close(gen_bits)
}

unsafe fn asyncgen_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    unsafe { ptr.add(offset) as *mut u64 }
}

pub(crate) unsafe fn asyncgen_gen_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_GEN_OFFSET) }
}

pub(crate) unsafe fn asyncgen_running_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET) }
}

pub(crate) unsafe fn asyncgen_pending_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET) }
}

pub(crate) unsafe fn asyncgen_finalizer_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_FINALIZER_OFFSET) }
}

/// A generator's finalizer is captured at first iteration, independent of the
/// thread's later hook changes. The snapshot owns both references across calls.
fn asyncgen_hooks_snapshot(py: &PyToken<'_>) -> [u64; 2] {
    let hooks = runtime_state(py).asyncgen_hooks.lock().unwrap();
    let values = hooks
        .get(&std::thread::current().id())
        .map(|hooks| [hooks.firstiter, hooks.finalizer])
        .unwrap_or([MoltObject::none().bits(); 2]);
    for bits in values {
        inc_ref_bits(py, bits);
    }
    values
}

/// A generator whose frame started and has not finished closes when its last
/// reference dies, as CPython's `_PyGen_Finalize` calls `close()`. A generator
/// that never started has nothing to finalize.
pub(crate) unsafe fn generator_needs_finalizer(ptr: *mut u8) -> bool {
    unsafe { generator_suspended(ptr) }
}

/// Close a dying generator; an error it raises is written unraisable.
pub(crate) unsafe fn generator_call_finalizer(py: &PyToken<'_>, ptr: *mut u8) {
    let generator = MoltObject::from_ptr(ptr).bits();
    crate::builtins::exceptions::run_unraisable_with_policy(
        py,
        || (generator, None),
        || {
            let _scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
            let result = molt_generator_close(generator);
            dec_ref_bits(py, result);
        },
    );
}

pub(crate) unsafe fn asyncgen_needs_finalizer(ptr: *mut u8) -> bool {
    unsafe {
        maybe_ptr_from_bits(asyncgen_gen_bits(ptr)).is_some_and(|generator| {
            object_type_id(generator) == TYPE_ID_GENERATOR && !generator_closed(generator)
        })
    }
}

pub(crate) unsafe fn asyncgen_firstiter_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_FIRSTITER_OFFSET) }
}

pub(crate) unsafe fn asyncgen_visit_owned_edges(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    unsafe {
        visit(asyncgen_pending_bits(ptr));
        visit(asyncgen_running_bits(ptr));
        visit(asyncgen_gen_bits(ptr));
        visit(asyncgen_finalizer_bits(ptr));
    }
}

pub(crate) unsafe fn asyncgen_detach_owned_edges(
    ptr: *mut u8,
    sink: &mut crate::object::heap_lifecycle::DetachedEdgeSink,
) {
    unsafe {
        let pending = crate::object::payload_refs::take(ptr, ASYNCGEN_PENDING_OFFSET);
        let running = crate::object::payload_refs::take(ptr, ASYNCGEN_RUNNING_OFFSET);
        let generator_bits = crate::object::payload_refs::take(ptr, ASYNCGEN_GEN_OFFSET);
        let finalizer = crate::object::payload_refs::take(ptr, ASYNCGEN_FINALIZER_OFFSET);
        sink.detach_if_heap(pending);
        sink.detach_if_heap(running);
        sink.detach_if_heap(generator_bits);
        sink.detach_if_heap(finalizer);
    }
}

unsafe fn asyncgen_set_firstiter_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_FIRSTITER_OFFSET, bits);
    }
}

unsafe fn asyncgen_firstiter_called(ptr: *mut u8) -> bool {
    unsafe {
        obj_from_bits(asyncgen_firstiter_bits(ptr))
            .as_bool()
            .unwrap_or(false)
    }
}

unsafe fn asyncgen_set_running_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_RUNNING_OFFSET, bits);
    }
}

unsafe fn asyncgen_set_pending_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_PENDING_OFFSET, bits);
    }
}

unsafe fn asyncgen_clear_pending_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        asyncgen_set_pending_bits(_py, ptr, MoltObject::none().bits());
    }
}

unsafe fn asyncgen_clear_running_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        asyncgen_set_running_bits(_py, ptr, MoltObject::none().bits());
    }
}

pub(crate) unsafe fn asyncgen_running(ptr: *mut u8) -> bool {
    unsafe { !obj_from_bits(asyncgen_running_bits(ptr)).is_none() }
}

pub(crate) unsafe fn asyncgen_await_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let Some(running_ptr) = maybe_ptr_from_bits(asyncgen_running_bits(ptr)) else {
            return MoltObject::none().bits();
        };
        let Some(gen_ptr) = maybe_ptr_from_bits(asyncgen_gen_bits(ptr)) else {
            return MoltObject::none().bits();
        };
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR || generator_running(gen_ptr) {
            return MoltObject::none().bits();
        }
        // The asyncgen operation owns CURRENT_TASK while its generator body polls.
        // Its semantic await edge survives scheduler wakeups and retains the exact
        // acquired Python awaitable independently of public local variable names.
        let awaited = crate::object::aux_header::object_frame_awaited_bits(running_ptr);
        if awaited == 0 {
            return MoltObject::none().bits();
        }
        crate::async_rt::awaitable::python_awaited_bits(_py, awaited)
    }
}

pub(crate) unsafe fn asyncgen_code_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let gen_bits = asyncgen_gen_bits(ptr);
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return MoltObject::none().bits();
        };
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
            return MoltObject::none().bits();
        }
        let code_bits = crate::object::aux_header::object_frame_code_bits(gen_ptr);
        if code_bits == 0 {
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, code_bits);
        code_bits
    }
}

unsafe fn asyncgen_call_firstiter_if_needed(
    _py: &PyToken<'_>,
    asyncgen_bits: u64,
    asyncgen_ptr: *mut u8,
) -> Option<u64> {
    unsafe {
        if asyncgen_firstiter_called(asyncgen_ptr) {
            return None;
        }
        asyncgen_set_firstiter_bits(_py, asyncgen_ptr, MoltObject::from_bool(true).bits());
        let [hook_bits, finalizer_bits] = asyncgen_hooks_snapshot(_py);
        crate::object::payload_refs::store_owned(
            _py,
            asyncgen_ptr,
            ASYNCGEN_FINALIZER_OFFSET,
            finalizer_bits,
        );
        if obj_from_bits(hook_bits).is_none() {
            return None;
        }
        let res_bits = call_callable1(_py, hook_bits, asyncgen_bits);
        dec_ref_bits(_py, hook_bits);
        if res_bits != 0 {
            dec_ref_bits(_py, res_bits);
        }
        if exception_pending(_py) {
            let exc_bits = molt_exception_last_pending();
            clear_exception(_py);
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return Some(raised);
        }
        None
    }
}

/// Called only inside the shared object-finalization revival window.
/// The object's captured finalizer participates in ordinary GC edge traversal.
pub(crate) unsafe fn asyncgen_call_finalizer(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        if !asyncgen_needs_finalizer(ptr) {
            return;
        }
        let hook = asyncgen_finalizer_bits(ptr);
        inc_ref_bits(py, hook);
        let _hook_owner = obj_from_bits(hook).as_ptr().map(crate::PtrDropGuard::new);
        let generator = MoltObject::from_ptr(ptr).bits();
        crate::builtins::exceptions::run_unraisable_with_policy(
            py,
            || (generator, None),
            || {
                let _scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
                let result = if obj_from_bits(hook).is_none() {
                    molt_generator_close(asyncgen_gen_bits(ptr))
                } else {
                    call_callable1(py, hook, generator)
                };
                dec_ref_bits(py, result);
            },
        );
    }
}

fn asyncgen_running_message(op: i64) -> &'static str {
    // CPython's asend type reports both of its operations as anext().
    match op {
        ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND => {
            "anext(): asynchronous generator is already running"
        }
        ASYNCGEN_OP_ATHROW => "athrow(): asynchronous generator is already running",
        ASYNCGEN_OP_ACLOSE => "aclose(): asynchronous generator is already running",
        _ => "asynchronous generator is already running",
    }
}

fn asyncgen_close_trace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_ASYNCGEN_CLOSE)
}

// Async-generator operation awaitable payload: the generator, the operation
// word and the operation argument (send value, thrown exception or None).
const ASYNCGEN_AWAITABLE_GEN: usize = 0;
const ASYNCGEN_AWAITABLE_OP: usize = 1;
const ASYNCGEN_AWAITABLE_ARG: usize = 2;
const ASYNCGEN_AWAITABLE_SLOTS: usize = 3;
/// Set on the operation word when a Python `throw()` arrives before the first
/// step. The first poll then throws the stored argument into the generator in
/// place of the operation's own first step, and keeps the operation's result
/// rules (CPython's INIT-state `gen_throw`).
const ASYNCGEN_OP_THROWN: i64 = 1 << 2;

unsafe fn asyncgen_awaitable_slot(ptr: *mut u8, index: usize) -> u64 {
    unsafe { *(ptr as *mut u64).add(index) }
}

unsafe fn asyncgen_awaitable_op(ptr: *mut u8) -> i64 {
    unsafe {
        to_i64(obj_from_bits(asyncgen_awaitable_slot(
            ptr,
            ASYNCGEN_AWAITABLE_OP,
        )))
        .unwrap_or(-1)
    }
}

unsafe fn asyncgen_awaitable_set_op(ptr: *mut u8, op: i64) {
    unsafe {
        *(ptr as *mut u64).add(ASYNCGEN_AWAITABLE_OP) = MoltObject::from_int(op).bits();
    }
}

unsafe fn asyncgen_awaitable_replace_arg(py: &PyToken<'_>, ptr: *mut u8, value: u64) {
    inc_ref_bits(py, value);
    unsafe {
        crate::object::payload_refs::store_owned(
            py,
            ptr,
            ASYNCGEN_AWAITABLE_ARG * std::mem::size_of::<u64>(),
            value,
        );
    }
}

/// CPython's reuse diagnostic for a finished operation awaitable, named by
/// its class: `async_generator_asend` or `async_generator_athrow`.
pub(crate) fn asyncgen_awaitable_reuse_message(py: &PyToken<'_>, ptr: *mut u8) -> &'static str {
    if unsafe { crate::object_class_bits(ptr) } == crate::builtin_classes(py).async_generator_asend
    {
        "cannot reuse already awaited __anext__()/asend()"
    } else {
        "cannot reuse already awaited aclose()/athrow()"
    }
}

/// A Python `send(value)` with a non-None value before the first step. The
/// asend family sends `value` in place of its own argument; the athrow family
/// refuses it unless the first step will only report a closed or running
/// generator, which CPython checks first. Returns false with an exception
/// pending.
///
/// # Safety
/// `ptr` must be a live async-generator operation awaitable.
pub(crate) unsafe fn asyncgen_awaitable_prime_send(
    py: &PyToken<'_>,
    ptr: *mut u8,
    value: u64,
) -> bool {
    unsafe {
        let op = asyncgen_awaitable_op(ptr);
        if matches!(op, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND) {
            asyncgen_awaitable_replace_arg(py, ptr, value);
            asyncgen_awaitable_set_op(ptr, ASYNCGEN_OP_ASEND);
            return true;
        }
        let reports_state =
            maybe_ptr_from_bits(asyncgen_awaitable_slot(ptr, ASYNCGEN_AWAITABLE_GEN))
                .filter(|&asyncgen| object_type_id(asyncgen) == TYPE_ID_ASYNC_GENERATOR)
                .is_some_and(|asyncgen| {
                    !obj_from_bits(asyncgen_running_bits(asyncgen)).is_none()
                        || maybe_ptr_from_bits(asyncgen_gen_bits(asyncgen))
                            .is_some_and(|generator| generator_closed(generator))
                });
        if reports_state {
            return true;
        }
        raise_exception::<()>(
            py,
            BuiltinExc::RuntimeError,
            "can't send non-None value to a just-started coroutine",
        );
        false
    }
}

/// A Python `throw()` before the first step: the first poll throws
/// `arguments` (a throw carrier) into the generator.
///
/// # Safety
/// `ptr` must be a live async-generator operation awaitable.
pub(crate) unsafe fn asyncgen_awaitable_prime_throw(
    py: &PyToken<'_>,
    ptr: *mut u8,
    arguments: u64,
) {
    unsafe {
        let op = asyncgen_awaitable_op(ptr) & !ASYNCGEN_OP_THROWN;
        asyncgen_awaitable_replace_arg(py, ptr, arguments);
        asyncgen_awaitable_set_op(ptr, op | ASYNCGEN_OP_THROWN);
    }
}

unsafe fn asyncgen_future_new(
    _py: &PyToken<'_>,
    asyncgen_bits: u64,
    op_kind: i64,
    arg_bits: u64,
) -> u64 {
    unsafe {
        let payload = (ASYNCGEN_AWAITABLE_SLOTS * std::mem::size_of::<u64>()) as u64;
        let obj_bits = molt_future_new(asyncgen_poll_fn_addr(), payload);
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let classes = crate::builtin_classes(_py);
        let class_bits = if matches!(op_kind, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND) {
            classes.async_generator_asend
        } else {
            classes.async_generator_athrow
        };
        if !crate::object::object_init_class_edge_unpublished(
            _py,
            obj_ptr,
            class_bits,
            crate::object::ClassEdgeOwnership::Owned,
        ) {
            dec_ref_bits(_py, obj_bits);
            return raise_exception::<_>(
                _py,
                BuiltinExc::SystemError,
                "async generator awaitable class initialization failed",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr.add(ASYNCGEN_AWAITABLE_GEN) = asyncgen_bits;
        *payload_ptr.add(ASYNCGEN_AWAITABLE_OP) = MoltObject::from_int(op_kind).bits();
        *payload_ptr.add(ASYNCGEN_AWAITABLE_ARG) = arg_bits;
        inc_ref_bits(_py, asyncgen_bits);
        inc_ref_bits(_py, arg_bits);
        obj_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_new(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected generator");
        };
        unsafe {
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected generator");
            }
            let total = std::mem::size_of::<MoltHeader>() + ASYNCGEN_CONTROL_SIZE;
            let ptr = alloc_object(_py, total, TYPE_ID_ASYNC_GENERATOR);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            let payload_ptr = ptr as *mut u64;
            *payload_ptr = gen_bits;
            inc_ref_bits(_py, gen_bits);
            // The async generator finalizes the frame it drives (its hook, or
            // close()); CPython has one object here, so the inner generator
            // never runs a finalizer of its own.
            (*header_from_obj_ptr(gen_ptr))
                .fetch_or_flags(crate::object::HEADER_FLAG_FINALIZER_RAN);
            *payload_ptr.add(1) = MoltObject::none().bits();
            *payload_ptr.add(2) = MoltObject::none().bits();
            *payload_ptr.add(3) = MoltObject::from_bool(false).bits();
            *payload_ptr.add(4) = MoltObject::none().bits();
            object_mark_has_ptrs(_py, ptr);
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_hooks_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hooks = asyncgen_hooks_snapshot(_py);
        let ptr = alloc_tuple(_py, &hooks);
        for bits in hooks {
            dec_ref_bits(_py, bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[derive(Clone, Copy)]
enum AsyncGenHook {
    Firstiter,
    Finalizer,
}

fn validate_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook, bits: u64) -> Result<(), u64> {
    let name = match slot {
        AsyncGenHook::Firstiter => "firstiter",
        AsyncGenHook::Finalizer => "finalizer",
    };
    let object = obj_from_bits(bits);
    if !object.is_none() {
        let callable = crate::builtins::callable::is_callable_impl(py, bits);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if !callable {
            let label = type_name(py, object);
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                &format!("callable {name} expected, got {label}"),
            ));
        }
    }
    Ok(())
}

fn audit_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook) -> bool {
    let event = match slot {
        AsyncGenHook::Firstiter => "sys.set_asyncgen_hook_firstiter",
        AsyncGenHook::Finalizer => "sys.set_asyncgen_hook_finalizer",
    };
    crate::builtins::sys_ext::audit_event_noargs(py, event)
}

fn replace_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook, bits: u64) {
    // Audit callbacks observe the old value. Finalization observes the newly
    // published value, and may reenter or modify either per-thread hook.
    inc_ref_bits(py, bits);
    let old = {
        let mut threads = runtime_state(py).asyncgen_hooks.lock().unwrap();
        let hooks = threads
            .entry(std::thread::current().id())
            .or_insert_with(|| crate::state::runtime_state::AsyncGenHooks {
                firstiter: MoltObject::none().bits(),
                finalizer: MoltObject::none().bits(),
            });
        let target = match slot {
            AsyncGenHook::Firstiter => &mut hooks.firstiter,
            AsyncGenHook::Finalizer => &mut hooks.finalizer,
        };
        std::mem::replace(target, bits)
    };
    dec_ref_bits(py, old);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_hooks_set(
    firstiter_bits: u64,
    finalizer_bits: u64,
    omitted_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let transactional = crate::object::ops_sys::runtime_target_minor(py) >= 13;
        if transactional {
            // CPython 3.13+ validates both arguments before any audit/update.
            for (slot, bits) in [
                (AsyncGenHook::Finalizer, finalizer_bits),
                (AsyncGenHook::Firstiter, firstiter_bits),
            ] {
                if bits != omitted_bits
                    && let Err(raised) = validate_asyncgen_hook(py, slot, bits)
                {
                    return raised;
                }
            }
        }
        let previous = if transactional && firstiter_bits != omitted_bits {
            let bits = {
                let threads = runtime_state(py).asyncgen_hooks.lock().unwrap();
                threads
                    .get(&std::thread::current().id())
                    .map_or(MoltObject::none().bits(), |hooks| hooks.finalizer)
            };
            // This thread owns the hook slot, and registration cannot call
            // Python. Observe its lifetime without retaining it across audits.
            match crate::object::weakref::WeakBorrow::new(py, bits) {
                Ok(observation) => Some(observation),
                Err(raised) => return raised,
            }
        } else {
            None
        };
        for (slot, bits) in [
            (AsyncGenHook::Finalizer, finalizer_bits),
            (AsyncGenHook::Firstiter, firstiter_bits),
        ] {
            if bits == omitted_bits {
                continue;
            }
            if !transactional && let Err(raised) = validate_asyncgen_hook(py, slot, bits) {
                return raised;
            }
            if !audit_asyncgen_hook(py, slot) {
                if matches!(slot, AsyncGenHook::Firstiter)
                    && let Some(previous) = previous
                {
                    // Rollback is audited even when finalizer was omitted.
                    // Audit saves the firstiter failure, restoring it only
                    // on success; a rollback audit failure replaces it.
                    if audit_asyncgen_hook(py, AsyncGenHook::Finalizer)
                        && let Some(bits) = previous.upgrade_owned()
                    {
                        replace_asyncgen_hook(py, AsyncGenHook::Finalizer, bits);
                        dec_ref_bits(py, bits);
                    }
                    // CPython's borrowed rollback pointer may already
                    // be dead. Never dereference/revive such a pointer:
                    // leave the current hook and original failure intact.
                }
                return MoltObject::none().bits();
            }
            replace_asyncgen_hook(py, slot, bits);
        }
        MoltObject::none().bits()
    })
}

/// `inspect.getasyncgenlocals`: the async generator's activation is its inner
/// generator task, projected through the shared layout authority.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_locals(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(asyncgen_ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "object is not a Python async generator",
            );
        };
        unsafe {
            if object_type_id(asyncgen_ptr) != TYPE_ID_ASYNC_GENERATOR {
                let name = type_name(_py, obj_from_bits(asyncgen_bits));
                let msg = format!("{name} is not a Python async generator");
                return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
            }
            let activation = maybe_ptr_from_bits(asyncgen_gen_bits(asyncgen_ptr))
                .filter(|gen_ptr| object_type_id(*gen_ptr) == TYPE_ID_GENERATOR);
            let locals = match activation {
                Some(gen_ptr) => crate::builtins::frames::activation_locals_bits(_py, gen_ptr),
                None => crate::builtins::frames::empty_locals_bits(_py),
            };
            locals.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

/// `inspect.getgeneratorlocals`, projected through the shared layout authority.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gen_locals(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "object is not a Python generator",
            );
        };
        unsafe {
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                let name = type_name(_py, obj_from_bits(gen_bits));
                let msg = format!("{name} is not a Python generator");
                return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
            }
            crate::builtins::frames::activation_locals_bits(_py, gen_ptr)
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_aiter(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected async generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
        }
        inc_ref_bits(_py, asyncgen_bits);
        asyncgen_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_anext(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(
                _py,
                asyncgen_bits,
                ASYNCGEN_OP_ANEXT,
                MoltObject::none().bits(),
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_asend(asyncgen_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ASEND, val_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_athrow(asyncgen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ATHROW, exc_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_aclose(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected async generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
        }
        let exc_ptr = alloc_exception(_py, BuiltinExc::GeneratorExit, "");
        if exc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        let future_bits =
            unsafe { asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ACLOSE, exc_bits) };
        dec_ref_bits(_py, exc_bits);
        future_bits
    })
}

/// # Safety
/// Caller must pass a valid async-generator awaitable object bits value.
/// The runtime must be initialized and the thread must be allowed to enter
/// the GIL-guarded runtime state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncgen_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            struct PendingExceptionGuard<'a> {
                py: &'a PyToken<'a>,
                prior_bits: Option<u64>,
            }

            impl<'a> PendingExceptionGuard<'a> {
                fn new(py: &'a PyToken<'a>) -> Self {
                    let prior_bits = if exception_pending(py) {
                        let bits = molt_exception_last_pending();
                        exception_clear_reason_set("asyncgen_poll_guard_prior");
                        molt_exception_clear();
                        Some(bits)
                    } else {
                        None
                    };
                    Self { py, prior_bits }
                }

                fn restore(&mut self) {
                    let Some(prior_bits) = self.prior_bits.take() else {
                        return;
                    };
                    if exception_pending(self.py) {
                        exception_clear_reason_set("asyncgen_poll_guard_restore");
                        molt_exception_clear();
                    }
                    let _ = molt_exception_set_last(prior_bits);
                    dec_ref_bits(self.py, prior_bits);
                }
            }

            impl Drop for PendingExceptionGuard<'_> {
                fn drop(&mut self) {
                    self.restore();
                }
            }

            let _pending_guard = PendingExceptionGuard::new(_py);
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < ASYNCGEN_AWAITABLE_SLOTS * std::mem::size_of::<u64>() {
                return MoltObject::none().bits() as i64;
            }
            let asyncgen_bits = asyncgen_awaitable_slot(obj_ptr, ASYNCGEN_AWAITABLE_GEN);
            let arg_bits = asyncgen_awaitable_slot(obj_ptr, ASYNCGEN_AWAITABLE_ARG);
            let op_word = asyncgen_awaitable_op(obj_ptr);
            // A primed throw keeps the operation's result rules.
            let thrown = op_word >= 0 && (op_word & ASYNCGEN_OP_THROWN) != 0;
            let op = if thrown {
                op_word & !ASYNCGEN_OP_THROWN
            } else {
                op_word
            };
            let Some(asyncgen_ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<i64>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            };
            if object_type_id(asyncgen_ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<i64>(
                    _py,
                    BuiltinExc::TypeError,
                    "expected async generator",
                );
            }
            let gen_bits = asyncgen_gen_bits(asyncgen_ptr);
            let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                return raise_exception::<i64>(_py, BuiltinExc::TypeError, "expected generator");
            };
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<i64>(_py, BuiltinExc::TypeError, "expected generator");
            }
            let task_ptr = current_task_ptr();
            let task_bits = if task_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(task_ptr).bits()
            };
            // The active operation owns the semantic await edge published by
            // its generator body; retain that owner for ag_running/ag_await.
            let running_marker_bits = if task_bits == MoltObject::none().bits() {
                obj_bits
            } else {
                task_bits
            };
            let running_bits = asyncgen_running_bits(asyncgen_ptr);
            let running_obj = obj_from_bits(running_bits);
            if !running_obj.is_none() && running_bits != running_marker_bits {
                return raise_exception::<i64>(
                    _py,
                    BuiltinExc::RuntimeError,
                    asyncgen_running_message(op),
                );
            }
            if generator_running(gen_ptr) {
                return raise_exception::<i64>(
                    _py,
                    BuiltinExc::RuntimeError,
                    asyncgen_running_message(op),
                );
            }
            let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
            if !thrown
                && !obj_from_bits(pending_bits).is_none()
                && matches!(op, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND)
            {
                inc_ref_bits(_py, pending_bits);
                asyncgen_clear_pending_bits(_py, asyncgen_ptr);
                let raised = molt_raise(pending_bits);
                dec_ref_bits(_py, pending_bits);
                return raised as i64;
            }

            let step = if crate::object::object_state(obj_ptr) != 0 {
                generator_resume_step(_py, gen_ptr)
            } else if thrown {
                // A Python throw() before the first step goes into the
                // generator itself; the operation's own step never runs.
                generator_throw_step(_py, gen_ptr, arg_bits)
            } else {
                match op {
                    ASYNCGEN_OP_ANEXT => {
                        if generator_closed(gen_ptr) {
                            if generator_pending_throw(gen_ptr) {
                                let throw_bits = *generator_slot_ptr(gen_ptr, GEN_THROW_OFFSET);
                                inc_ref_bits(_py, throw_bits);
                                generator_set_slot(
                                    _py,
                                    gen_ptr,
                                    GEN_THROW_OFFSET,
                                    MoltObject::none().bits(),
                                );
                                let raised = molt_raise(throw_bits);
                                dec_ref_bits(_py, throw_bits);
                                return raised as i64;
                            }
                            return raise_exception::<i64>(_py, BuiltinExc::StopAsyncIteration, "");
                        }
                        if generator_pending_throw(gen_ptr) {
                            generator_resume_step(_py, gen_ptr)
                        } else {
                            generator_send_step(_py, gen_ptr, MoltObject::none().bits())
                        }
                    }
                    ASYNCGEN_OP_ASEND => {
                        if generator_closed(gen_ptr) {
                            if generator_pending_throw(gen_ptr) {
                                // Bug-compatible projection: a closed body
                                // reports its exhaustion pair.
                                return generator_step_pair(_py, generator_resume_step(_py, gen_ptr))
                                    as i64;
                            }
                            return raise_exception::<i64>(_py, BuiltinExc::StopAsyncIteration, "");
                        }
                        if !generator_started(gen_ptr) && !obj_from_bits(arg_bits).is_none() {
                            return raise_exception::<i64>(
                                _py,
                                BuiltinExc::TypeError,
                                "can't send non-None value to a just-started async generator",
                            );
                        }
                        if generator_pending_throw(gen_ptr) {
                            generator_resume_step(_py, gen_ptr)
                        } else {
                            generator_send_step(_py, gen_ptr, arg_bits)
                        }
                    }
                    ASYNCGEN_OP_ATHROW => {
                        if generator_closed(gen_ptr) {
                            if generator_pending_throw(gen_ptr) {
                                return raise_exception::<i64>(
                                    _py,
                                    BuiltinExc::StopAsyncIteration,
                                    "",
                                );
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        generator_throw_step(_py, gen_ptr, arg_bits)
                    }
                    ASYNCGEN_OP_ACLOSE => {
                        if asyncgen_close_trace_enabled() {
                            let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
                            eprintln!(
                                "asyncgen_aclose gen=0x{:x} state={:?} pending={}",
                                gen_ptr as usize,
                                generator_state(gen_ptr),
                                !obj_from_bits(pending_bits).is_none()
                            );
                        }
                        if generator_closed(gen_ptr) {
                            return MoltObject::none().bits() as i64;
                        }
                        if !generator_started(gen_ptr) {
                            generator_complete(_py, gen_ptr, false);
                            return MoltObject::none().bits() as i64;
                        }
                        generator_throw_step(_py, gen_ptr, arg_bits)
                    }
                    _ => {
                        return raise_exception::<i64>(
                            _py,
                            BuiltinExc::TypeError,
                            "invalid async generator op",
                        );
                    }
                }
            };

            let step = match step {
                Ok(step) => step,
                Err(()) => {
                    if running_bits == running_marker_bits {
                        asyncgen_clear_running_bits(_py, asyncgen_ptr);
                    }
                    crate::object::object_set_state(obj_ptr, 0);
                    let exc_bits = molt_exception_last_pending();
                    if op == ASYNCGEN_OP_ACLOSE {
                        let terminal = obj_from_bits(exc_bits).as_ptr().is_some_and(|exc_ptr| {
                            crate::builtins::exceptions::exception_matches_kind(
                                _py,
                                MoltObject::from_ptr(exc_ptr).bits(),
                                crate::BuiltinExc::GeneratorExit,
                            ) || crate::builtins::exceptions::exception_matches_kind(
                                _py,
                                MoltObject::from_ptr(exc_ptr).bits(),
                                crate::BuiltinExc::StopAsyncIteration,
                            )
                        });
                        if terminal {
                            exception_clear_reason_set("asyncgen_aclose_swallow");
                            molt_exception_clear();
                            dec_ref_bits(_py, exc_bits);
                            generator_complete(_py, gen_ptr, true);
                            return MoltObject::none().bits() as i64;
                        }
                    }
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits() as i64;
                }
            };

            if let GeneratorStep::Pending = step {
                asyncgen_set_running_bits(_py, asyncgen_ptr, running_marker_bits);
                crate::object::object_set_state(obj_ptr, 1);
                return pending_bits_i64();
            }

            if running_bits == running_marker_bits {
                asyncgen_clear_running_bits(_py, asyncgen_ptr);
            }
            crate::object::object_set_state(obj_ptr, 0);

            match step {
                GeneratorStep::Yielded(value) => {
                    if op != ASYNCGEN_OP_ACLOSE {
                        return value as i64;
                    }
                    dec_ref_bits(_py, value);
                    if !thrown && !obj_from_bits(arg_bits).is_none() {
                        asyncgen_set_pending_bits(_py, asyncgen_ptr, arg_bits);
                        generator_set_slot(
                            _py,
                            gen_ptr,
                            GEN_THROW_OFFSET,
                            MoltObject::none().bits(),
                        );
                    }
                    raise_exception::<i64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "async generator ignored GeneratorExit",
                    )
                }
                GeneratorStep::Returned(value) => {
                    dec_ref_bits(_py, value);
                    if op == ASYNCGEN_OP_ACLOSE {
                        return MoltObject::none().bits() as i64;
                    }
                    // A generator that returns ends asend and athrow alike
                    // with StopAsyncIteration (CPython's async_gen_unwrap_value).
                    raise_exception::<i64>(_py, BuiltinExc::StopAsyncIteration, "")
                }
                GeneratorStep::Pending => unreachable!("handled above"),
            }
        })
    }
}

#[cfg(test)]
mod asyncgen_lifetime_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static RESURRECTED: AtomicU64 = AtomicU64::new(0);
    static ORIGINAL_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REPLACEMENT_CALLS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn original_finalizer(generator: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            ORIGINAL_CALLS.fetch_add(1, Ordering::Relaxed);
            // Models loop.call_soon_threadsafe retaining an aclose awaitable.
            // This must occur before committed death, in the shared revival window.
            inc_ref_bits(py, generator);
            assert_eq!(RESURRECTED.swap(generator, Ordering::Relaxed), 0);
            MoltObject::none().bits()
        })
    }

    extern "C" fn replacement_finalizer(_generator: u64) -> u64 {
        REPLACEMENT_CALLS.fetch_add(1, Ordering::Relaxed);
        MoltObject::none().bits()
    }

    fn callback(py: &PyToken<'_>, address: *const ()) -> u64 {
        let pointer = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!pointer.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(pointer, address) };
        MoltObject::from_ptr(pointer).bits()
    }

    static HOOK_AUDIT_MODE: AtomicUsize = AtomicUsize::new(0);
    static HOOK_AUDIT_EVENTS: std::sync::Mutex<Vec<&'static str>> =
        std::sync::Mutex::new(Vec::new());

    extern "C" fn hook_audit(event: u64, _args: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let mode = HOOK_AUDIT_MODE.load(Ordering::Relaxed);
            match crate::string_obj_to_owned(obj_from_bits(event)).as_deref() {
                Some("sys.set_asyncgen_hook_firstiter") => {
                    HOOK_AUDIT_EVENTS.lock().unwrap().push("firstiter");
                    if mode != 0 {
                        return raise_exception::<u64>(
                            py,
                            BuiltinExc::RuntimeError,
                            "firstiter veto",
                        );
                    }
                }
                Some("sys.set_asyncgen_hook_finalizer") => {
                    let rollback = {
                        let mut events = HOOK_AUDIT_EVENTS.lock().unwrap();
                        let rollback = events.contains(&"firstiter");
                        events.push("finalizer");
                        rollback
                    };
                    if mode == 2 && rollback {
                        return raise_exception::<u64>(
                            py,
                            BuiltinExc::LookupError,
                            "rollback veto",
                        );
                    }
                }
                _ => {}
            }
            MoltObject::none().bits()
        })
    }

    #[test]
    fn asyncgen_hook_audited_rollback_obeys_target_version_and_dead_identity_safety() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let omitted = missing_bits(py);
            let first = callback(py, original_finalizer as *const ());
            let old_finalizer = callback(py, replacement_finalizer as *const ());
            let replacement = callback(py, replacement_finalizer as *const ());
            let audit_ptr = crate::object::builders::alloc_function_obj(
                py,
                crate::provenance::abi::expose_function_address(hook_audit as *const ()),
                2,
            );
            assert!(!audit_ptr.is_null());
            unsafe {
                crate::object::layout::function_set_call_target_ptr(
                    audit_ptr,
                    hook_audit as *const (),
                );
            }
            let audit = MoltObject::from_ptr(audit_ptr).bits();
            crate::builtins::sys_ext::molt_sys_addaudithook(audit);
            let saved_version =
                crate::object::ops_sys::runtime_target_python_info(runtime_state(py));
            for minor in [12, 13, 14] {
                let mut version = saved_version.clone();
                version.minor = minor;
                *runtime_state(py).sys_version_info.lock().unwrap() = Some(version);
                HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                molt_asyncgen_hooks_set(first, old_finalizer, omitted);
                HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                molt_asyncgen_hooks_set(MoltObject::from_int(1).bits(), replacement, omitted);
                assert!(exception_pending(py));
                clear_exception(py);
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(
                    hooks,
                    [
                        first,
                        if minor >= 13 {
                            old_finalizer
                        } else {
                            replacement
                        }
                    ]
                );
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
                assert_eq!(
                    *HOOK_AUDIT_EVENTS.lock().unwrap(),
                    if minor >= 13 {
                        vec![]
                    } else {
                        vec!["finalizer"]
                    }
                );
                for (mode, expected_finalizer, expected_error) in [
                    (
                        1,
                        if minor >= 13 {
                            old_finalizer
                        } else {
                            replacement
                        },
                        crate::BuiltinExc::RuntimeError,
                    ),
                    (
                        2,
                        replacement,
                        if minor >= 13 {
                            crate::BuiltinExc::LookupError
                        } else {
                            crate::BuiltinExc::RuntimeError
                        },
                    ),
                ] {
                    HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                    molt_asyncgen_hooks_set(first, old_finalizer, omitted);
                    HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                    HOOK_AUDIT_MODE.store(mode, Ordering::Relaxed);
                    molt_asyncgen_hooks_set(replacement, replacement, omitted);
                    assert!(exception_pending(py));
                    let exception = crate::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_kind(
                        py,
                        exception,
                        expected_error
                    ));
                    clear_exception(py);
                    dec_ref_bits(py, exception);
                    let hooks = asyncgen_hooks_snapshot(py);
                    assert_eq!(hooks, [first, expected_finalizer]);
                    for bits in hooks {
                        dec_ref_bits(py, bits);
                    }
                    assert_eq!(
                        *HOOK_AUDIT_EVENTS.lock().unwrap(),
                        if minor >= 13 {
                            vec!["finalizer", "firstiter", "finalizer"]
                        } else {
                            vec!["finalizer", "firstiter"]
                        }
                    );
                }
                // The old callable's only owner is the slot. Replacement must
                // really destroy it before firstiter audit, and rollback must
                // never upgrade that dead identity or leak its borrow entry.
                HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                let ephemeral = callback(py, replacement_finalizer as *const ());
                molt_asyncgen_hooks_set(first, ephemeral, omitted);
                dec_ref_bits(py, ephemeral);
                let registrations = runtime_state(py)
                    .weakrefs
                    .lock()
                    .unwrap()
                    .borrowed_targets
                    .len();
                HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                HOOK_AUDIT_MODE.store(1, Ordering::Relaxed);
                molt_asyncgen_hooks_set(replacement, replacement, omitted);
                assert!(exception_pending(py));
                clear_exception(py);
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(hooks, [first, replacement]);
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
                assert_eq!(
                    runtime_state(py)
                        .weakrefs
                        .lock()
                        .unwrap()
                        .borrowed_targets
                        .len(),
                    registrations
                );
            }
            HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
            molt_asyncgen_hooks_set(none, none, omitted);
            *runtime_state(py).sys_version_info.lock().unwrap() = Some(saved_version);
            crate::builtins::sys_ext::sys_ext_clear_state(py, runtime_state(py));
            for bits in [first, old_finalizer, replacement, audit] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn asyncgen_hook_setter_preserves_omission_and_ordered_failures() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let omitted = missing_bits(py);
            let first = callback(py, original_finalizer as *const ());
            let finalizer = callback(py, replacement_finalizer as *const ());
            let check = |expected| {
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(hooks, expected);
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
            };
            molt_asyncgen_hooks_set(first, finalizer, omitted);
            molt_asyncgen_hooks_set(omitted, omitted, omitted);
            check([first, finalizer]);
            let invalid = MoltObject::from_int(1).bits();
            molt_asyncgen_hooks_set(finalizer, invalid, omitted);
            assert!(exception_pending(py));
            clear_exception(py);
            check([first, finalizer]);
            molt_asyncgen_hooks_set(invalid, first, omitted);
            assert!(exception_pending(py));
            clear_exception(py);
            check([
                first,
                if crate::object::ops_sys::runtime_target_minor(py) >= 13 {
                    finalizer
                } else {
                    first
                },
            ]);
            molt_asyncgen_hooks_set(omitted, none, omitted);
            check([first, none]);
            molt_asyncgen_hooks_set(none, none, omitted);
            dec_ref_bits(py, first);
            dec_ref_bits(py, finalizer);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn asyncgen_captures_finalizer_and_allows_one_safe_resurrection() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            ORIGINAL_CALLS.store(0, Ordering::Relaxed);
            REPLACEMENT_CALLS.store(0, Ordering::Relaxed);
            RESURRECTED.store(0, Ordering::Relaxed);
            let original = callback(py, original_finalizer as *const ());
            let replacement = callback(py, replacement_finalizer as *const ());
            molt_asyncgen_hooks_set(none, original, missing_bits(py));
            let inner = molt_task_new(0, GEN_CONTROL_SIZE as u64, TASK_KIND_GENERATOR);
            assert!(!obj_from_bits(inner).is_none());
            let generator = molt_asyncgen_new(inner);
            assert!(!obj_from_bits(generator).is_none());
            dec_ref_bits(py, inner);
            let pointer = obj_from_bits(generator).as_ptr().unwrap();
            assert!(unsafe { asyncgen_call_firstiter_if_needed(py, generator, pointer) }.is_none());
            molt_asyncgen_hooks_set(none, replacement, missing_bits(py));
            dec_ref_bits(py, original);
            // Captured hook is a real GC-visible owner after hook replacement.
            let mut edges = Vec::new();
            unsafe { asyncgen_visit_owned_edges(pointer, |bits| edges.push(bits)) };
            assert!(edges.contains(&original));
            let observation = crate::object::weakref::WeakBorrow::new(py, generator).unwrap();
            dec_ref_bits(py, generator);
            assert_eq!(observation.upgrade_owned(), Some(generator));
            dec_ref_bits(py, generator);
            assert_eq!(ORIGINAL_CALLS.load(Ordering::Relaxed), 1);
            assert_eq!(REPLACEMENT_CALLS.load(Ordering::Relaxed), 0);
            assert_eq!(RESURRECTED.load(Ordering::Relaxed), generator);
            assert_eq!(unsafe { object_type_id(pointer) }, TYPE_ID_ASYNC_GENERATOR);
            assert!(!unsafe {
                (*header_from_obj_ptr(pointer)).has_flag(crate::object::HEADER_FLAG_DEALLOCATING)
            });
            molt_asyncgen_hooks_set(none, none, missing_bits(py));
            dec_ref_bits(py, replacement);
            dec_ref_bits(py, RESURRECTED.swap(0, Ordering::Relaxed));
            assert!(observation.upgrade_owned().is_none());
            assert_eq!(ORIGINAL_CALLS.load(Ordering::Relaxed), 1);
            assert!(!exception_pending(py));
        });
    }
}

#[cfg(test)]
mod suspension_reference_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static CALLBACK_OWNER: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_OFFSET: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_OBSERVED: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_EMPTY_PREFIX: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_PREFIX: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

    struct CallbackScope;

    impl CallbackScope {
        fn new() -> Self {
            let scope = Self;
            scope.reset();
            scope
        }

        fn reset(&self) {
            CALLBACK_OWNER.store(0, Ordering::Relaxed);
            CALLBACK_OFFSET.store(0, Ordering::Relaxed);
            CALLBACK_OBSERVED.store(0, Ordering::Relaxed);
            CALLBACK_EMPTY_PREFIX.store(0, Ordering::Relaxed);
            for slot in &CALLBACK_PREFIX {
                slot.store(0, Ordering::Relaxed);
            }
        }
    }

    impl Drop for CallbackScope {
        fn drop(&mut self) {
            self.reset();
        }
    }

    extern "C" fn payload(_value: u64) -> u64 {
        MoltObject::none().bits()
    }

    extern "C" fn on_release(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let bits = CALLBACK_OWNER.swap(0, Ordering::Relaxed);
            if bits == 0 {
                return MoltObject::none().bits();
            }
            let owner = ptr_from_bits(bits);
            let offset = CALLBACK_OFFSET.load(Ordering::Relaxed);
            unsafe {
                let words = CALLBACK_EMPTY_PREFIX.load(Ordering::Relaxed);
                for (index, observed) in CALLBACK_PREFIX.iter().take(words).enumerate() {
                    observed.store(*owner.cast::<u64>().add(index), Ordering::Relaxed);
                }
                CALLBACK_OBSERVED.store(*owner.add(offset).cast::<u64>(), Ordering::Relaxed);
                crate::object::payload_refs::store_borrowed(
                    py,
                    owner,
                    offset,
                    MoltObject::from_int(77).bits(),
                );
            }
            MoltObject::none().bits()
        })
    }

    fn callback(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!ptr.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(ptr, address) };
        MoltObject::from_ptr(ptr).bits()
    }

    fn watched(py: &PyToken<'_>, value: u64, callback: u64) -> u64 {
        let class = crate::molt_weakref_reference_type();
        let weak = crate::molt_weakref_new(class, value, callback);
        dec_ref_bits(py, class);
        assert!(!exception_pending(py));
        weak
    }

    #[derive(Clone, Copy)]
    enum Reference {
        Send,
        Throw,
        Pending,
        Running,
    }

    impl Reference {
        fn offset(self) -> usize {
            match self {
                Self::Send => GEN_SEND_OFFSET,
                Self::Throw => GEN_THROW_OFFSET,
                Self::Pending => ASYNCGEN_PENDING_OFFSET,
                Self::Running => ASYNCGEN_RUNNING_OFFSET,
            }
        }

        fn owner(self, py: &PyToken<'_>) -> u64 {
            let inner = molt_task_new(0, GEN_CONTROL_SIZE as u64, TASK_KIND_GENERATOR);
            match self {
                Self::Send | Self::Throw => inner,
                Self::Pending | Self::Running => {
                    let owner = molt_asyncgen_new(inner);
                    dec_ref_bits(py, inner);
                    owner
                }
            }
        }

        unsafe fn store(self, py: &PyToken<'_>, owner: *mut u8, value: u64) {
            unsafe {
                match self {
                    Self::Send | Self::Throw => generator_set_slot(py, owner, self.offset(), value),
                    Self::Pending => asyncgen_set_pending_bits(py, owner, value),
                    Self::Running => asyncgen_set_running_bits(py, owner, value),
                }
            }
        }
    }

    #[test]
    fn suspension_slots_preserve_sole_owner_and_owned_self_assignment() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            for field in [
                Reference::Send,
                Reference::Throw,
                Reference::Pending,
                Reference::Running,
            ] {
                let owner = field.owner(py);
                let ptr = ptr_from_bits(owner);
                let value = callback(py, payload as *const ());
                let weak = watched(py, value, none);
                unsafe { field.store(py, ptr, value) };
                dec_ref_bits(py, value);
                unsafe { field.store(py, ptr, value) };
                let retained = crate::molt_weakref_call(weak);
                assert_eq!(
                    retained, value,
                    "borrowed self assignment finalized the value"
                );
                // The acquired weakref result is a distinct incoming owned edge.
                unsafe {
                    crate::object::payload_refs::store_owned(py, ptr, field.offset(), retained)
                };
                unsafe { field.store(py, ptr, none) };
                assert!(
                    obj_from_bits(crate::molt_weakref_call(weak)).is_none(),
                    "owned self assignment leaked an edge"
                );
                dec_ref_bits(py, weak);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn suspension_prefix_is_fully_detached_before_reentrant_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            let owner = crate::molt_alloc(16);
            let ptr = ptr_from_bits(owner);
            let value = callback(py, payload as *const ());
            let hook = callback(py, on_release as *const ());
            let weak = watched(py, value, hook);
            let later = callback(py, payload as *const ());
            let later_weak = watched(py, later, MoltObject::none().bits());
            unsafe {
                crate::object::payload_refs::store_owned(py, ptr, 0, value);
                crate::object::payload_refs::store_owned(py, ptr, 8, later);
            }
            crate::molt_object_publish_initialized(owner);
            CALLBACK_OWNER.store(owner, Ordering::Relaxed);
            CALLBACK_OFFSET.store(8, Ordering::Relaxed);
            CALLBACK_EMPTY_PREFIX.store(2, Ordering::Relaxed);
            unsafe { crate::object::payload_refs::clear_prefix::<2>(py, ptr) };
            for observed in &CALLBACK_PREFIX {
                assert_eq!(
                    observed.load(Ordering::Relaxed),
                    MoltObject::none().bits(),
                    "callback observed a partially retired payload"
                );
            }
            assert!(obj_from_bits(CALLBACK_OBSERVED.load(Ordering::Relaxed)).is_none());
            assert_eq!(
                unsafe { *ptr.cast::<u64>().add(1) },
                MoltObject::from_int(77).bits()
            );
            assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
            assert!(obj_from_bits(crate::molt_weakref_call(later_weak)).is_none());
            dec_ref_bits(py, weak);
            dec_ref_bits(py, later_weak);
            dec_ref_bits(py, hook);
            dec_ref_bits(py, owner);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn suspension_slot_callback_observes_publication_and_keeps_reentrant_write() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            for field in [
                Reference::Send,
                Reference::Throw,
                Reference::Pending,
                Reference::Running,
            ] {
                let owner = field.owner(py);
                let ptr = ptr_from_bits(owner);
                let value = callback(py, payload as *const ());
                let hook = callback(py, on_release as *const ());
                let weak = watched(py, value, hook);
                let incoming = MoltObject::from_ptr(crate::alloc_string(py, b"replacement")).bits();
                unsafe { field.store(py, ptr, value) };
                dec_ref_bits(py, value);
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_OFFSET.store(field.offset(), Ordering::Relaxed);
                CALLBACK_OBSERVED.store(0, Ordering::Relaxed);
                unsafe { field.store(py, ptr, incoming) };
                assert_eq!(CALLBACK_OBSERVED.load(Ordering::Relaxed), incoming);
                assert_eq!(
                    unsafe { *ptr.add(field.offset()).cast::<u64>() },
                    MoltObject::from_int(77).bits()
                );
                assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
                dec_ref_bits(py, incoming);
                dec_ref_bits(py, weak);
                dec_ref_bits(py, hook);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn suspension_callback_records_bad_prefix_and_disarms_on_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::molt_alloc(16);
            let ptr = ptr_from_bits(owner);
            assert!(!ptr.is_null());
            let unexpected = MoltObject::from_int(73).bits();
            unsafe {
                ptr.cast::<u64>().write(unexpected);
                ptr.cast::<u64>().add(1).write(MoltObject::none().bits());
            }
            crate::molt_object_publish_initialized(owner);
            let failure = crate::test_support::catch_expected_unwind(|| {
                let _callbacks = CallbackScope::new();
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_EMPTY_PREFIX.store(2, Ordering::Relaxed);
                let result = on_release(0);
                dec_ref_bits(py, result);
                assert_eq!(CALLBACK_PREFIX[0].load(Ordering::Relaxed), unexpected);
                assert_eq!(CALLBACK_OWNER.load(Ordering::Relaxed), 0);
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                panic!("suspension callback rollback control");
            });
            assert_eq!(
                failure
                    .expect_err("rollback control must unwind")
                    .downcast_ref::<&str>(),
                Some(&"suspension callback rollback control")
            );
            assert_eq!(CALLBACK_OWNER.load(Ordering::Relaxed), 0);
            assert_eq!(CALLBACK_EMPTY_PREFIX.load(Ordering::Relaxed), 0);
            dec_ref_bits(py, owner);
            assert!(!exception_pending(py));
        });
    }
}

#[cfg(test)]
mod frame_state_tests {
    use super::*;
    use crate::object::heap_lifecycle::{
        DetachedEdgeSink, detach_generator_owned_edges, terminal_detach_capacity,
        visit_owned_values,
    };

    /// A body that yields 1 and 2, then returns 3, reporting each stop the way
    /// compiled bodies do: a yield stores its resume label, a return stores
    /// `GENERATOR_RESUME_RETURNED`, and the value itself is the result. A
    /// resumed yield raises a thrown exception first, as compiled bodies do.
    extern "C" fn count_poll(raw: u64) -> i64 {
        let ptr = std::ptr::with_exposed_provenance_mut::<u8>(raw as usize);
        let thrown = unsafe { *generator_slot_ptr(ptr, GEN_THROW_OFFSET) };
        if !obj_from_bits(thrown).is_none() {
            return crate::with_gil_entry_nopanic!(py, {
                inc_ref_bits(py, thrown);
                unsafe { generator_set_slot(py, ptr, GEN_THROW_OFFSET, MoltObject::none().bits()) };
                let raised = molt_raise(thrown);
                dec_ref_bits(py, thrown);
                raised as i64
            });
        }
        let (label, value) = match crate::object::object_state(ptr) {
            0 => (1, 1),
            1 => (2, 2),
            _ => (GENERATOR_RESUME_RETURNED, 3),
        };
        crate::object::object_set_state(ptr, label);
        MoltObject::from_int(value).bits() as i64
    }

    extern "C" fn raising_poll(_raw: u64) -> i64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<i64>(py, BuiltinExc::ValueError, "body raised")
        })
    }

    fn generator(poll: extern "C" fn(u64) -> i64) -> (u64, *mut u8) {
        let address = crate::provenance::abi::expose_function_address(poll as *const ());
        let bits = molt_task_new(address, (GEN_CONTROL_SIZE + 8) as u64, TASK_KIND_GENERATOR);
        (bits, obj_from_bits(bits).as_ptr().expect("generator"))
    }

    fn int(step: GeneratorResult) -> (&'static str, i64) {
        match step {
            Ok(GeneratorStep::Yielded(value)) => ("yielded", to_i64(obj_from_bits(value)).unwrap()),
            Ok(GeneratorStep::Returned(value)) => {
                ("returned", to_i64(obj_from_bits(value)).unwrap_or(i64::MIN))
            }
            Ok(GeneratorStep::Pending) => ("pending", 0),
            Err(()) => ("raised", 0),
        }
    }

    #[test]
    fn a_body_reports_yields_and_completion_without_a_tuple() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = generator(count_poll);
            let none = MoltObject::none().bits();
            unsafe {
                assert_eq!(generator_state(ptr), FrameState::Created);
                assert_eq!(int(generator_send_step(py, ptr, none)), ("yielded", 1));
                assert_eq!(generator_state(ptr), FrameState::Suspended);
                assert_eq!(int(generator_send_step(py, ptr, none)), ("yielded", 2));
                assert_eq!(int(generator_send_step(py, ptr, none)), ("returned", 3));
                assert_eq!(generator_state(ptr), FrameState::Completed);
                // A finished generator never re-enters its body.
                assert_eq!(
                    int(generator_send_step(py, ptr, none)),
                    ("returned", i64::MIN)
                );
            }
            assert!(!exception_pending(py));
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn every_terminal_path_completes_the_generator() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            // An exception escaping the body.
            let (raising, ptr) = generator(raising_poll);
            assert!(unsafe { generator_send_step(py, ptr, none) }.is_err());
            assert_eq!(unsafe { generator_state(ptr) }, FrameState::Completed);
            clear_exception(py);
            dec_ref_bits(py, raising);
            // close() of a created generator.
            let (created, ptr) = generator(count_poll);
            dec_ref_bits(py, molt_generator_close(created));
            assert_eq!(unsafe { generator_state(ptr) }, FrameState::Completed);
            dec_ref_bits(py, created);
            // throw() into a created generator raises at its first line.
            let (thrown, ptr) = generator(count_poll);
            let exception =
                MoltObject::from_ptr(alloc_exception(py, BuiltinExc::KeyError, "k")).bits();
            assert!(unsafe { generator_throw_step(py, ptr, exception) }.is_err());
            assert_eq!(unsafe { generator_state(ptr) }, FrameState::Completed);
            clear_exception(py);
            // throw() into a finished generator raises without entering it.
            assert!(unsafe { generator_throw_step(py, ptr, exception) }.is_err());
            assert_eq!(unsafe { generator_state(ptr) }, FrameState::Completed);
            clear_exception(py);
            dec_ref_bits(py, exception);
            dec_ref_bits(py, thrown);
            // close() of a suspended generator throws GeneratorExit into it.
            let (suspended, ptr) = generator(count_poll);
            assert_eq!(
                int(unsafe { generator_send_step(py, ptr, none) }),
                ("yielded", 1)
            );
            let result = molt_generator_close(suspended);
            assert_eq!(unsafe { generator_state(ptr) }, FrameState::Completed);
            dec_ref_bits(py, result);
            clear_exception(py);
            dec_ref_bits(py, suspended);
        });
    }

    #[test]
    fn entry_is_exclusive_and_terminal_states_absorb() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = generator(count_poll);
            unsafe {
                let entered = GeneratorExecution::enter(ptr).ok().expect("created enters");
                assert_eq!(generator_state(ptr), FrameState::Executing);
                assert!(matches!(
                    GeneratorExecution::enter(ptr),
                    Err(EntryRefusal::Executing)
                ));
                drop(entered);
                assert_eq!(
                    generator_state(ptr),
                    FrameState::Created,
                    "an entry whose body never ran restores its state"
                );
                let mut entered = GeneratorExecution::enter(ptr).ok().expect("enters");
                entered.run();
                drop(entered);
                assert_eq!(generator_state(ptr), FrameState::Suspended);
                generator_complete(py, ptr, true);
                assert!(matches!(
                    GeneratorExecution::enter(ptr),
                    Err(EntryRefusal::Terminal)
                ));
                generator_clear(ptr);
                assert_eq!(generator_state(ptr), FrameState::Cleared);
                generator_complete(py, ptr, true);
                assert_eq!(
                    generator_state(ptr),
                    FrameState::Cleared,
                    "completion never lowers a cleared state"
                );
            }
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn raw_control_words_are_never_visited_or_detached_as_edges() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (bits, ptr) = generator(count_poll);
            // Raw words that a boxed-value reader would misread; none may
            // reach an edge visitor or the detachment sink.
            let reserved = 0x0000_dead_beef_u64;
            unsafe {
                generator_set_state_for_test(ptr, FrameState::Completed);
                for offset in crate::GEN_RESERVED_OFFSETS {
                    *generator_slot_ptr(ptr, offset) = reserved;
                }
            }
            let mut visited = Vec::new();
            unsafe { visit_owned_values(py, ptr, &mut |edge| visited.push(edge)) };
            for raw in [FrameState::Completed as u64, reserved] {
                assert!(
                    !visited.contains(&raw),
                    "raw word {raw:#x} visited as an edge"
                );
            }
            let (edges, resources) = unsafe { terminal_detach_capacity(py, ptr) };
            let mut sink = DetachedEdgeSink::try_with_capacities(edges, resources).unwrap();
            unsafe { detach_generator_owned_edges(ptr, &mut sink) };
            sink.release_all(py);
            unsafe {
                assert_eq!(generator_state(ptr), FrameState::Cleared);
                for offset in crate::GEN_RESERVED_OFFSETS {
                    assert_eq!(
                        *generator_slot_ptr(ptr, offset),
                        reserved,
                        "detachment never takes a raw word"
                    );
                }
            }
            dec_ref_bits(py, bits);
        });
    }
}
