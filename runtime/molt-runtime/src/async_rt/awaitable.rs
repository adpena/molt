//! Python await acquisition and iterator-to-native-poll delegation.
//!
//! The payload owns Python values; the existing task shape owns traversal,
//! cancellation, waiter edges, completion and destruction. A direct Python
//! send/throw call has only a scoped invocation transport, never a task registry.

use super::throw_protocol::{
    call_throw_method, normalize_throw_argument, parse_throw_call, raise_throw_argument,
};
use crate::async_rt::generators::{
    is_iterable_coroutine_bits, is_native_coroutine_bits, is_native_poll_future_bits,
};
use crate::object::iterable::{SpecialIterationKind, SpecialIterationStep, special_iteration_step};
use crate::*;
use std::cell::RefCell;

const ITERATOR: usize = 0;
const YIELDED: usize = 1;
const CHILD: usize = 2;
const THROWN: usize = 3;
const ITERATOR_SLOTS: usize = 4;

enum ResumeRequest {
    Send(u64),
    Throw(u64),
    // A normalized leaf exception targets its parent's continuation, not
    // Python throw/close methods on a newly created delegate.
    Inject(u64),
}

impl ResumeRequest {
    fn bits(&self) -> u64 {
        match self {
            Self::Send(bits) | Self::Throw(bits) | Self::Inject(bits) => *bits,
        }
    }
}

struct PythonResume {
    target: usize,
    request: Option<ResumeRequest>,
    yielded: Option<u64>,
    direct: bool,
}

thread_local! {
    // Dynamic call context only. A nested user .send has its own frame and
    // cannot steal another invocation's values. Nothing persists after return.
    static PYTHON_RESUMES: RefCell<Vec<PythonResume>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct PythonResumeScope<'a, 'py> {
    py: &'a PyToken<'py>,
    depth: usize,
}

impl<'a, 'py> PythonResumeScope<'a, 'py> {
    /// A scheduler root is independent of any enclosing manual send/throw.
    /// Nested Python resumes still push their own request above this boundary.
    pub(crate) fn scheduled(py: &'a PyToken<'py>) -> Option<Self> {
        let depth = PYTHON_RESUMES.with(|stack| {
            let mut stack = stack.borrow_mut();
            // Ordinary scheduler polling has no enclosing transport to mask.
            if stack.is_empty() {
                return None;
            }
            let depth = stack.len();
            stack.push(PythonResume {
                target: 0,
                request: None,
                yielded: None,
                direct: false,
            });
            Some(depth)
        })?;
        Some(Self { py, depth })
    }

    fn enter(py: &'a PyToken<'py>, target: *mut u8, request: ResumeRequest, direct: bool) -> Self {
        inc_ref_bits(py, request.bits());
        inc_ref_bits(py, MoltObject::from_ptr(target).bits());
        let depth = PYTHON_RESUMES.with(|stack| {
            let mut stack = stack.borrow_mut();
            let depth = stack.len();
            stack.push(PythonResume {
                target: target as usize,
                request: Some(request),
                yielded: None,
                direct,
            });
            depth
        });
        Self { py, depth }
    }

    fn take_yielded(&self) -> u64 {
        PYTHON_RESUMES.with(|stack| {
            stack
                .borrow_mut()
                .last_mut()
                .and_then(|frame| frame.yielded.take())
                .unwrap_or(MoltObject::none().bits())
        })
    }
}

impl Drop for PythonResumeScope<'_, '_> {
    fn drop(&mut self) {
        let frame = PYTHON_RESUMES.with(|stack| {
            let mut stack = stack.borrow_mut();
            assert_eq!(
                stack.len(),
                self.depth + 1,
                "unbalanced Python resume context"
            );
            stack.pop().unwrap()
        });
        // Release after popping, so destructors can enter another resumption.
        if let Some(request) = frame.request {
            dec_ref_bits(self.py, request.bits());
        }
        if let Some(yielded) = frame.yielded {
            dec_ref_bits(self.py, yielded);
        }
        if frame.target != 0 {
            dec_ref_bits(
                self.py,
                MoltObject::from_ptr(std::ptr::with_exposed_provenance_mut::<u8>(frame.target))
                    .bits(),
            );
        }
    }
}

fn direct_resume() -> bool {
    PYTHON_RESUMES.with(|stack| stack.borrow().last().is_some_and(|frame| frame.direct))
}

fn take_request(ptr: *mut u8) -> Option<ResumeRequest> {
    PYTHON_RESUMES.with(|stack| {
        let mut stack = stack.borrow_mut();
        let frame = stack.last_mut()?;
        (frame.target == ptr as usize)
            .then(|| frame.request.take())
            .flatten()
    })
}

fn publish_yield(py: &PyToken<'_>, value: u64) {
    inc_ref_bits(py, value);
    let previous = PYTHON_RESUMES.with(|stack| {
        stack
            .borrow_mut()
            .last_mut()
            .expect("direct resume context")
            .yielded
            .replace(value)
    });
    if let Some(previous) = previous {
        dec_ref_bits(py, previous);
    }
}

unsafe fn slot(ptr: *mut u8, index: usize) -> u64 {
    unsafe { *ptr.cast::<u64>().add(index) }
}

unsafe fn replace_owned(py: &PyToken<'_>, ptr: *mut u8, index: usize, value: u64) {
    unsafe {
        crate::object::payload_refs::store_owned(
            py,
            ptr,
            index * std::mem::size_of::<u64>(),
            value,
        );
    }
}

unsafe fn take_slot(ptr: *mut u8, index: usize) -> u64 {
    unsafe { crate::object::payload_refs::take(ptr, index * std::mem::size_of::<u64>()) }
}

unsafe fn clear_iterator(py: &PyToken<'_>, ptr: *mut u8) {
    let values =
        std::array::from_fn::<_, ITERATOR_SLOTS, _>(|index| unsafe { take_slot(ptr, index) });
    let awaited = unsafe { crate::object::aux_header::object_take_frame_awaited_bits(ptr) };
    crate::await_waiter_clear(py, ptr);
    for value in values {
        dec_ref_bits(py, value);
    }
    if awaited != 0 {
        dec_ref_bits(py, awaited);
    }
}

pub(crate) fn is_coroutine_wrapper_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && crate::object::object_poll_fn(ptr)
                == crate::async_rt::poll::coroutine_wrapper_poll_fn_addr()
    })
}

fn is_iterator_adapter(ptr: *mut u8) -> bool {
    (unsafe { object_type_id(ptr) == TYPE_ID_OBJECT })
        && crate::object::object_poll_fn(ptr)
            == crate::async_rt::poll::await_iterator_poll_fn_addr()
}

/// Project the Python object actually acquired by await, retaining one owner.
pub(crate) fn python_awaited_bits(py: &PyToken<'_>, bits: u64) -> u64 {
    let value = maybe_ptr_from_bits(bits)
        .filter(|&ptr| is_iterator_adapter(ptr))
        .map(|ptr| unsafe { slot(ptr, ITERATOR) })
        .unwrap_or(bits);
    inc_ref_bits(py, value);
    value
}

fn iterator_poll_adapter(py: &PyToken<'_>, iterator: u64) -> u64 {
    let future = crate::molt_future_new(
        crate::async_rt::poll::await_iterator_poll_fn_addr(),
        (ITERATOR_SLOTS * std::mem::size_of::<u64>()) as u64,
    );
    if let Some(ptr) = maybe_ptr_from_bits(future) {
        inc_ref_bits(py, iterator);
        unsafe { replace_owned(py, ptr, ITERATOR, iterator) };
    }
    future
}

/// Acquire one owned native poll representation of a validated Python awaitable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_get_awaitable(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if is_native_coroutine_bits(bits) || is_native_poll_future_bits(bits) {
            inc_ref_bits(py, bits);
            return bits;
        }
        if is_iterable_coroutine_bits(bits) {
            return iterator_poll_adapter(py, bits);
        }
        let method =
            unsafe { crate::builtins::attr::lookup_async_special_method(py, bits, b"__await__") };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(method) = method else {
            let message = format!(
                "object {} can't be used in 'await' expression",
                type_name(py, obj_from_bits(bits))
            );
            return raise_exception::<_>(py, "TypeError", &message);
        };
        let iterator = unsafe { call_callable0(py, method) };
        dec_ref_bits(py, method);
        if exception_pending(py) {
            dec_ref_bits(py, iterator);
            return MoltObject::none().bits();
        }
        if is_native_coroutine_bits(iterator) || is_iterable_coroutine_bits(iterator) {
            dec_ref_bits(py, iterator);
            return raise_exception::<_>(py, "TypeError", "__await__() returned a coroutine");
        }
        if is_coroutine_wrapper_bits(iterator) || is_native_poll_future_bits(iterator) {
            return iterator;
        }
        let valid = unsafe { crate::builtins::attr::is_iterator_bits(py, iterator) };
        if exception_pending(py) {
            dec_ref_bits(py, iterator);
            return MoltObject::none().bits();
        }
        if !valid {
            let message = format!(
                "__await__() returned non-iterator of type '{}'",
                type_name(py, obj_from_bits(iterator))
            );
            dec_ref_bits(py, iterator);
            return raise_exception::<_>(py, "TypeError", &message);
        }
        let adapter = iterator_poll_adapter(py, iterator);
        dec_ref_bits(py, iterator);
        adapter
    })
}

/// Native coroutine __await__ creates a real Python iterator identity.
#[unsafe(no_mangle)]
pub extern "C" fn molt_awaitable_await(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !is_native_coroutine_bits(bits) {
            if is_native_poll_future_bits(bits) {
                inc_ref_bits(py, bits);
                return bits;
            }
            return raise_exception::<_>(py, "TypeError", "object is not awaitable");
        }
        let wrapper = crate::molt_future_new(
            crate::async_rt::poll::coroutine_wrapper_poll_fn_addr(),
            std::mem::size_of::<u64>() as u64,
        );
        if let Some(ptr) = maybe_ptr_from_bits(wrapper) {
            inc_ref_bits(py, bits);
            unsafe { replace_owned(py, ptr, 0, bits) };
            if !unsafe {
                crate::object::object_init_class_edge_unpublished(
                    py,
                    ptr,
                    crate::builtin_classes(py).coroutine_wrapper,
                    crate::object::ClassEdgeOwnership::Owned,
                )
            } {
                dec_ref_bits(py, wrapper);
                return raise_exception::<_>(
                    py,
                    "SystemError",
                    "coroutine wrapper class initialization failed",
                );
            }
        }
        wrapper
    })
}

fn coroutine_receiver(py: &PyToken<'_>, bits: u64) -> Option<u64> {
    if is_native_coroutine_bits(bits) {
        return Some(bits);
    }
    if is_coroutine_wrapper_bits(bits) {
        let inner = unsafe { slot(ptr_from_bits(bits), 0) };
        if is_native_coroutine_bits(inner) {
            return Some(inner);
        }
    }
    raise_exception::<Option<u64>>(py, "TypeError", "expected coroutine or coroutine wrapper")
}

pub(crate) fn coroutine_is_done(ptr: *mut u8) -> bool {
    unsafe { ((*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0 }
}

/// The semantic delegation owner outlives scheduler wake subscriptions. A
/// ready Future or cancellation may remove its waiter edge before resumption.
fn resume_target(ptr: *mut u8) -> *mut u8 {
    let next = |ptr: *mut u8| {
        if ptr.is_null() || is_iterator_adapter(ptr) {
            return std::ptr::null_mut();
        }
        maybe_ptr_from_bits(crate::object::aux_header::object_frame_awaited_bits(ptr))
            .unwrap_or(std::ptr::null_mut())
    };
    let mut cursor = ptr;
    let mut fast = ptr;
    loop {
        let target = next(cursor);
        if target.is_null() {
            return cursor;
        }
        cursor = target;
        fast = next(next(fast));
        if cursor == fast {
            return ptr;
        }
    }
}

fn coroutine_resume(py: &PyToken<'_>, receiver: u64, request: ResumeRequest) -> u64 {
    let Some(coro) = coroutine_receiver(py, receiver) else {
        return MoltObject::none().bits();
    };
    let ptr = ptr_from_bits(coro);
    let initially_started = unsafe {
        ((*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_GEN_STARTED) != 0
    };
    let initial_awaited = crate::object::aux_header::object_frame_awaited_bits(ptr);
    let normalized = if let ResumeRequest::Throw(exception) = request
        && (!initially_started || coroutine_is_done(ptr) || initial_awaited == 0)
    {
        let Some(exception) = normalize_throw_argument(py, exception) else {
            return MoltObject::none().bits();
        };
        Some(exception)
    } else {
        None
    };
    // Normalization can call Python, including this same coroutine. Admission
    // and the continuation target must describe its state after that callback.
    if coroutine_is_done(ptr) {
        if let Some(exception) = normalized {
            dec_ref_bits(py, exception);
        }
        return raise_exception::<_>(py, "RuntimeError", "cannot reuse already awaited coroutine");
    }
    if unsafe {
        ((*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_GEN_RUNNING) != 0
    } {
        if let Some(exception) = normalized {
            dec_ref_bits(py, exception);
        }
        return raise_exception::<_>(py, "ValueError", "coroutine already executing");
    }
    let started = unsafe {
        ((*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_GEN_STARTED) != 0
    };
    if !started {
        if let Some(exception) = normalized {
            // Raise at the created activation's first instruction: one real
            // target frame, entered and left without running the body. The
            // execution flag blocks reentrant resumption meanwhile.
            let _running = match CoroutinePollGuard::enter_before_body(py, ptr) {
                Ok(guard) => guard,
                Err(result) => {
                    dec_ref_bits(py, exception);
                    return result as u64;
                }
            };
            let frame = unsafe { crate::builtins::frames::ActivationFrameScope::enter(py, ptr) };
            crate::task_mark_done(py, ptr);
            let Ok(_frame) = frame else {
                dec_ref_bits(py, exception);
                return MoltObject::none().bits();
            };
            crate::molt_exception_trace_prepend(exception);
            let result = crate::molt_raise(exception);
            dec_ref_bits(py, exception);
            return result;
        }
        if let ResumeRequest::Send(value) = request
            && !obj_from_bits(value).is_none()
        {
            return raise_exception::<_>(
                py,
                "TypeError",
                "can't send non-None value to a just-started coroutine",
            );
        }
    }
    let scope = if let Some(exception) = normalized {
        // A constructor-created delegation did not receive the original throw.
        // Inject at the immediate await result so the parent's compiled error
        // continuation runs without polling, closing or throwing into its child.
        let awaited = crate::object::aux_header::object_frame_awaited_bits(ptr);
        let target = maybe_ptr_from_bits(awaited).unwrap_or(ptr);
        let scope = PythonResumeScope::enter(py, target, ResumeRequest::Inject(exception), true);
        dec_ref_bits(py, exception);
        scope
    } else {
        PythonResumeScope::enter(py, resume_target(ptr), request, true)
    };
    // A Python callback can manually resume another coroutine without awaiting
    // it. Only nested polls inside that coroutine establish delegation edges.
    let (result, failure) = {
        let _caller = crate::CurrentTaskScope::enter(py, std::ptr::null_mut());
        let result = crate::molt_future_poll(coro);
        let failure = if exception_pending(py) {
            let exception = crate::molt_exception_last();
            crate::clear_exception(py);
            Some(exception)
        } else {
            None
        };
        (result, failure)
    };
    if let Some(exception) = failure {
        let raised = crate::molt_raise(exception);
        dec_ref_bits(py, exception);
        dec_ref_bits(py, result as u64);
        return raised;
    }
    if exception_pending(py) {
        return result as u64;
    }
    if result == pending_bits_i64() {
        return scope.take_yielded();
    }
    let value = result as u64;
    let raised = unsafe { crate::async_rt::generators::raise_stop_iteration_from_value(py, value) };
    dec_ref_bits(py, value);
    raised
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_coroutine_send_method(receiver: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        coroutine_resume(py, receiver, ResumeRequest::Send(value))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_coroutine_throw_method(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((receiver, arguments)) = parse_throw_call(py, args, kwargs) else {
            return MoltObject::none().bits();
        };
        let result = coroutine_resume(py, receiver, ResumeRequest::Throw(arguments));
        dec_ref_bits(py, arguments);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_coroutine_close_method(receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(coro) = coroutine_receiver(py, receiver) else {
            return MoltObject::none().bits();
        };
        let ptr = ptr_from_bits(coro);
        if coroutine_is_done(ptr) {
            return MoltObject::none().bits();
        }
        let flags = unsafe { (*header_from_obj_ptr(ptr)).load_synchronized_flags() };
        if flags & (HEADER_FLAG_GEN_STARTED | HEADER_FLAG_GEN_RUNNING) == 0 {
            unsafe {
                crate::object::heap_lifecycle::close_unstarted_coroutine(py, ptr);
            }
            return MoltObject::none().bits();
        }
        let exit_ptr = crate::alloc_exception(py, "GeneratorExit", "");
        if exit_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let exit = MoltObject::from_ptr(exit_ptr).bits();
        let result = coroutine_resume(py, receiver, ResumeRequest::Throw(exit));
        dec_ref_bits(py, exit);
        if !exception_pending(py) {
            dec_ref_bits(py, result);
            return raise_exception::<_>(py, "RuntimeError", "coroutine ignored GeneratorExit");
        }
        let exception = crate::molt_exception_last();
        let stopped = crate::builtins::exceptions::exception_matches_builtin_name(
            py,
            exception,
            "StopIteration",
        );
        let expected = stopped
            || crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "GeneratorExit",
            );
        let returned = if stopped && crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
            crate::builtins::exceptions::exception_typed_field_get(
                py,
                ptr_from_bits(exception),
                molt_obj_model::ExceptionTypedField::StopIterationValue,
            )
            .and_then(Result::ok)
            .unwrap_or(MoltObject::none().bits())
        } else {
            MoltObject::none().bits()
        };
        dec_ref_bits(py, exception);
        if expected {
            crate::clear_exception(py);
            dec_ref_bits(py, result);
            return returned;
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_coroutine_wrapper_iter(receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !is_coroutine_wrapper_bits(receiver) {
            return raise_exception::<_>(py, "TypeError", "expected coroutine wrapper");
        }
        inc_ref_bits(py, receiver);
        receiver
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_coroutine_wrapper_next(receiver: u64) -> u64 {
    molt_coroutine_send_method(receiver, MoltObject::none().bits())
}

/// Native scheduler delegation does not introduce a Python frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_coroutine_wrapper_poll(raw: u64) -> i64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = ptr_from_bits(raw);
        let coroutine = unsafe { slot(ptr, 0) };
        let Some(inner) = coroutine_receiver(py, coroutine) else {
            return MoltObject::none().bits() as i64;
        };
        if coroutine_is_done(ptr_from_bits(inner)) {
            return raise_exception::<i64>(
                py,
                "RuntimeError",
                "cannot reuse already awaited coroutine",
            );
        }
        crate::molt_future_poll(inner)
    })
}

/// Called before the native poll. Exceptions aimed at a delegated iterator are
/// handled by that iterator's throw protocol; other leaves raise into their
/// awaiting compiled continuation.
pub(crate) fn direct_exception_before_poll(py: &PyToken<'_>, ptr: *mut u8) -> Option<i64> {
    let intercept = PYTHON_RESUMES.with(|stack| {
        stack.borrow().last().is_some_and(|frame| {
            frame.target == ptr as usize
                && match frame.request {
                    Some(ResumeRequest::Inject(_)) => true,
                    Some(ResumeRequest::Throw(_)) => !is_iterator_adapter(ptr),
                    _ => false,
                }
        })
    });
    if !intercept {
        return None;
    }
    let exception = match take_request(ptr) {
        Some(ResumeRequest::Inject(exception)) => exception,
        Some(ResumeRequest::Throw(arguments)) => {
            let normalized = normalize_throw_argument(py, arguments);
            dec_ref_bits(py, arguments);
            let Some(exception) = normalized else {
                return Some(MoltObject::none().bits() as i64);
            };
            exception
        }
        _ => unreachable!(),
    };
    crate::molt_exception_trace_prepend(exception);
    let result = crate::molt_raise(exception);
    dec_ref_bits(py, exception);
    Some(result as i64)
}

/// Route the existing task cancellation through its suspended delegation leaf,
/// so Python finally/except blocks execute before the task becomes terminal.
pub(crate) fn cancellation_resume_scope<'a, 'py>(
    py: &'a PyToken<'py>,
    task: *mut u8,
) -> Option<PythonResumeScope<'a, 'py>> {
    if !crate::task_cancel_pending(task) {
        return None;
    }
    let target = resume_target(task);
    crate::task_take_cancel_pending(task);
    crate::raise_cancelled_with_message::<u64>(py, task);
    let exception = crate::molt_exception_last();
    crate::clear_exception(py);
    let scope = PythonResumeScope::enter(py, target, ResumeRequest::Throw(exception), false);
    dec_ref_bits(py, exception);
    Some(scope)
}

pub(crate) struct CoroutinePollGuard(*mut MoltHeader);

impl CoroutinePollGuard {
    pub(crate) fn enter(py: &PyToken<'_>, ptr: *mut u8) -> Result<Option<Self>, i64> {
        if !is_native_coroutine_bits(MoltObject::from_ptr(ptr).bits()) {
            return Ok(None);
        }
        let header = unsafe { header_from_obj_ptr(ptr) };
        if !unsafe {
            (*header).try_set_flags_unless(HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_RUNNING)
        } {
            return Err(raise_exception::<i64>(
                py,
                "ValueError",
                "coroutine already executing",
            ));
        }
        unsafe {
            (*header).fetch_or_flags(HEADER_FLAG_GEN_STARTED);
        }
        Ok(Some(Self(header)))
    }

    /// Execution custody for a created coroutine that raises before its body.
    /// The activation is never marked started.
    pub(crate) fn enter_before_body(py: &PyToken<'_>, ptr: *mut u8) -> Result<Self, i64> {
        let header = unsafe { header_from_obj_ptr(ptr) };
        if !unsafe {
            (*header).try_set_flags_unless(HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_RUNNING)
        } {
            return Err(raise_exception::<i64>(
                py,
                "ValueError",
                "coroutine already executing",
            ));
        }
        Ok(Self(header))
    }
}

impl Drop for CoroutinePollGuard {
    fn drop(&mut self) {
        unsafe {
            (*self.0).take_flags(HEADER_FLAG_GEN_RUNNING);
        }
    }
}

fn iterator_resume(
    py: &PyToken<'_>,
    iterator: u64,
    request: ResumeRequest,
) -> Result<SpecialIterationStep, ()> {
    if let ResumeRequest::Send(value) = request
        && obj_from_bits(value).is_none()
    {
        return special_iteration_step(py, iterator, SpecialIterationKind::Next);
    }
    let closing = match &request {
        ResumeRequest::Throw(arguments) => {
            super::throw_protocol::throw_is_generator_exit(py, *arguments)
        }
        ResumeRequest::Send(_) | ResumeRequest::Inject(_) => false,
    };
    if exception_pending(py) {
        return Err(());
    }
    crate::exception_stack_push();
    let result = match request {
        ResumeRequest::Inject(exception) => {
            crate::molt_exception_trace_prepend(exception);
            crate::molt_raise(exception)
        }
        ResumeRequest::Send(value) => unsafe {
            crate::async_rt::generators_async::asyncio_call_method1(py, iterator, b"send", value)
        },
        ResumeRequest::Throw(exception) => {
            if !closing
                && maybe_ptr_from_bits(iterator)
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR })
            {
                let pair = crate::molt_generator_throw(iterator, exception);
                if exception_pending(py) {
                    pair
                } else {
                    unsafe { crate::async_rt::generators::generator_method_result(py, pair) }
                }
            } else {
                let method_name = if closing {
                    b"close".as_slice()
                } else {
                    b"throw".as_slice()
                };
                let name = crate::attr_name_bits_from_bytes(py, method_name);
                let method = name.and_then(|name| {
                    let method = unsafe {
                        crate::builtins::attr::attr_lookup_ptr_allow_missing(
                            py,
                            ptr_from_bits(iterator),
                            name,
                        )
                    };
                    dec_ref_bits(py, name);
                    method
                });
                if exception_pending(py) {
                    if let Some(method) = method {
                        dec_ref_bits(py, method);
                    }
                    MoltObject::none().bits()
                } else if let Some(method) = method {
                    let result = unsafe {
                        if closing {
                            call_callable0(py, method)
                        } else {
                            call_throw_method(py, method, exception)
                        }
                    };
                    dec_ref_bits(py, method);
                    if closing && !exception_pending(py) {
                        dec_ref_bits(py, result);
                        raise_throw_argument(py, exception)
                    } else {
                        result
                    }
                } else {
                    raise_throw_argument(py, exception)
                }
            }
        }
    };
    if !exception_pending(py) {
        crate::exception_stack_pop(py);
        return Ok(SpecialIterationStep::Item(result));
    }
    dec_ref_bits(py, result);
    let exception = crate::molt_exception_last();
    // SEND consumes delegate exhaustion, including a failing close method.
    let exhausted =
        crate::builtins::exceptions::exception_matches_builtin_name(py, exception, "StopIteration");
    if exhausted {
        let value = maybe_ptr_from_bits(exception)
            .and_then(|ptr| {
                crate::builtins::exceptions::exception_typed_field_get(
                    py,
                    ptr,
                    molt_obj_model::ExceptionTypedField::StopIterationValue,
                )
                .and_then(Result::ok)
            })
            .unwrap_or(MoltObject::none().bits());
        crate::clear_exception(py);
        crate::exception_stack_pop(py);
        dec_ref_bits(py, exception);
        return Ok(SpecialIterationStep::Exhausted(value));
    }
    crate::exception_stack_pop_restore_last(py, exception);
    dec_ref_bits(py, exception);
    Err(())
}

fn scheduled_yield(py: &PyToken<'_>, value: u64) -> u64 {
    if is_native_poll_future_bits(value) {
        inc_ref_bits(py, value);
        return value;
    }
    let marker_present = unsafe {
        crate::builtins::attr::has_special_method(py, value, b"_asyncio_future_blocking")
    };
    if !marker_present {
        return raise_exception::<_>(py, "RuntimeError", "Task got bad yield");
    }
    let Some(name) = crate::attr_name_bits_from_bytes(py, b"_asyncio_future_blocking") else {
        return MoltObject::none().bits();
    };
    let marker = crate::molt_getattr_builtin(value, name, MoltObject::none().bits());
    if exception_pending(py) {
        dec_ref_bits(py, name);
        dec_ref_bits(py, marker);
        return MoltObject::none().bits();
    }
    let blocking = is_truthy(py, obj_from_bits(marker));
    dec_ref_bits(py, marker);
    if exception_pending(py) {
        dec_ref_bits(py, name);
        return MoltObject::none().bits();
    }
    if !blocking {
        dec_ref_bits(py, name);
        return raise_exception::<_>(py, "RuntimeError", "yield was used instead of yield from");
    }
    crate::molt_set_attr_name(value, name, MoltObject::from_bool(false).bits());
    dec_ref_bits(py, name);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    // The compiled guest owns Future callback ordering, loop identity and
    // cancellation. Reuse the same promise bridge as Future.__await__ without
    // recursively re-entering a foreign future's self-yielding __await__.
    let Some(module_id) = crate::builtins::module_table::module_id_of("asyncio.futures") else {
        return raise_exception::<_>(
            py,
            "RuntimeError",
            "Future scheduling requires compiled asyncio.futures",
        );
    };
    let module = crate::builtins::module_table::module_ensure(py, module_id);
    if exception_pending(py) {
        dec_ref_bits(py, module);
        return MoltObject::none().bits();
    }
    let Some(name) = crate::attr_name_bits_from_bytes(py, b"_wait_for_future") else {
        dec_ref_bits(py, module);
        return MoltObject::none().bits();
    };
    let method = crate::molt_getattr_builtin(module, name, MoltObject::none().bits());
    dec_ref_bits(py, name);
    dec_ref_bits(py, module);
    if exception_pending(py) {
        dec_ref_bits(py, method);
        return MoltObject::none().bits();
    }
    let coroutine = unsafe { call_callable1(py, method, value) };
    dec_ref_bits(py, method);
    if exception_pending(py) {
        dec_ref_bits(py, coroutine);
        return MoltObject::none().bits();
    }
    let acquired = molt_get_awaitable(coroutine);
    dec_ref_bits(py, coroutine);
    acquired
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_await_iterator_poll(raw: u64) -> i64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = ptr_from_bits(raw);
        let iterator = unsafe { slot(ptr, ITERATOR) };
        if obj_from_bits(iterator).is_none() {
            return MoltObject::none().bits() as i64;
        }
        let mut request = take_request(ptr);
        if request.is_none() {
            let queued = unsafe { take_slot(ptr, THROWN) };
            if !obj_from_bits(queued).is_none() {
                request = Some(ResumeRequest::Throw(queued));
            }
        }
        let child = unsafe { slot(ptr, CHILD) };
        if request.is_some() && !obj_from_bits(child).is_none() {
            // Retire the scheduling bridge before resuming its Python iterator.
            // Keep an owner through close so its finally block can unsubscribe.
            let child = unsafe { take_slot(ptr, CHILD) };
            unsafe {
                crate::object::aux_header::object_replace_frame_awaited_owned(py, ptr, 0);
            }
            crate::await_waiter_clear(py, ptr);
            if is_native_coroutine_bits(child) {
                let closed = molt_coroutine_close_method(child);
                dec_ref_bits(py, closed);
                if exception_pending(py) {
                    let exception = crate::molt_exception_last();
                    crate::clear_exception(py);
                    if let Some(previous) = request.replace(ResumeRequest::Throw(exception)) {
                        dec_ref_bits(py, previous.bits());
                    }
                }
            }
            dec_ref_bits(py, child);
        }
        if request.is_none() && !obj_from_bits(child).is_none() {
            let result = crate::molt_future_poll(child);
            if result == pending_bits_i64() && !exception_pending(py) {
                return result;
            }
            unsafe {
                replace_owned(py, ptr, CHILD, MoltObject::none().bits());
            }
            if exception_pending(py) {
                let exception = crate::molt_exception_last();
                crate::clear_exception(py);
                request = Some(ResumeRequest::Throw(exception));
            } else {
                dec_ref_bits(py, result as u64);
            }
        }
        unsafe {
            replace_owned(py, ptr, YIELDED, MoltObject::none().bits());
        }
        let request = request.unwrap_or(ResumeRequest::Send(MoltObject::none().bits()));
        let request_bits = request.bits();
        let step = iterator_resume(py, iterator, request);
        dec_ref_bits(py, request_bits);
        match step {
            Ok(SpecialIterationStep::Exhausted(value)) => {
                unsafe {
                    clear_iterator(py, ptr);
                }
                value as i64
            }
            Err(()) | Ok(SpecialIterationStep::Missing) => {
                unsafe {
                    clear_iterator(py, ptr);
                }
                if !exception_pending(py) {
                    raise_exception::<()>(py, "TypeError", "await iterator lost __next__");
                }
                MoltObject::none().bits() as i64
            }
            Ok(SpecialIterationStep::Item(value)) => {
                unsafe {
                    replace_owned(py, ptr, YIELDED, value);
                }
                if direct_resume() {
                    publish_yield(py, value);
                    return pending_bits_i64();
                }
                if obj_from_bits(value).is_none() {
                    if !crate::async_rt::scheduler::task_waiting_on_event(py, ptr) {
                        crate::wake_task_ptr(py, ptr);
                    }
                    return pending_bits_i64();
                }
                let child = scheduled_yield(py, value);
                if exception_pending(py) {
                    let exception = crate::molt_exception_last();
                    crate::clear_exception(py);
                    dec_ref_bits(py, child);
                    unsafe {
                        replace_owned(py, ptr, THROWN, exception);
                    }
                    crate::wake_task_ptr(py, ptr);
                    return pending_bits_i64();
                }
                unsafe {
                    replace_owned(py, ptr, CHILD, child);
                }
                // Re-enter only through the existing scheduler. Registering the
                // child poll now establishes the normal waiter/wake dependency.
                let result = crate::molt_future_poll(child);
                if exception_pending(py) {
                    let exception = crate::molt_exception_last();
                    crate::clear_exception(py);
                    unsafe {
                        replace_owned(py, ptr, THROWN, exception);
                        replace_owned(py, ptr, CHILD, MoltObject::none().bits());
                    }
                    crate::wake_task_ptr(py, ptr);
                } else if result != pending_bits_i64() {
                    dec_ref_bits(py, result as u64);
                    unsafe {
                        replace_owned(py, ptr, CHILD, MoltObject::none().bits());
                    }
                    crate::wake_task_ptr(py, ptr);
                }
                pending_bits_i64()
            }
        }
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn scheduled_foreign_future_reset_invokes_native_setter_once() {
        use molt_cpython_abi::abi_types::{
            Py_TPFLAGS_READY, PyBaseObject_Type, PyObject, PyType_Type, PyTypeObject,
        };
        use molt_cpython_abi::api::{errors, mapping, object, refcount, sequences, strings};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        #[repr(C)]
        struct Future {
            object: PyObject,
            resets: usize,
        }
        unsafe extern "C" fn reset(
            receiver: *mut PyObject,
            name: *mut PyObject,
            value: *mut PyObject,
        ) -> std::os::raw::c_int {
            unsafe {
                let future = &mut *receiver.cast::<Future>();
                future.resets += 1;
                assert_eq!(
                    std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(name)).to_bytes(),
                    b"_asyncio_future_blocking"
                );
                assert_eq!(object::PyObject_IsTrue(value), 0);
                errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                    c"future reset rejected".as_ptr(),
                );
            }
            -1
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut class: PyTypeObject = std::mem::zeroed();
                class.ob_base.ob_base.ob_refcnt = 1;
                class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
                class.tp_name = c"ForeignFutureMutation".as_ptr();
                class.tp_base = &raw mut PyBaseObject_Type;
                class.tp_flags = Py_TPFLAGS_READY;
                class.tp_getattro = Some(object::PyObject_GenericGetAttr);
                class.tp_setattro = Some(reset);
                class.tp_dict = mapping::PyDict_New();
                class.tp_mro = sequences::PyTuple_New(2);
                for (index, base) in [(&raw mut class).cast(), (&raw mut PyBaseObject_Type).cast()]
                    .into_iter()
                    .enumerate()
                {
                    refcount::Py_INCREF(base);
                    assert_eq!(
                        sequences::PyTuple_SetItem(class.tp_mro, index as isize, base),
                        0
                    );
                }
                let truth =
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_bool(true).bits());
                assert_eq!(
                    mapping::PyDict_SetItemString(
                        class.tp_dict,
                        c"_asyncio_future_blocking".as_ptr(),
                        truth
                    ),
                    0
                );
                refcount::Py_DECREF(truth);
                let mut future = Future {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut class,
                    },
                    resets: 0,
                };
                let bits = GLOBAL_BRIDGE
                    .molt_value_for_pyobj(&raw mut future.object)
                    .unwrap();
                let result = scheduled_yield(py, bits);
                dec_ref_bits(py, result);
                assert_eq!(future.resets, 1);
                let raised = errors::PyErr_GetRaisedException();
                assert!(!raised.is_null());
                assert_ne!(
                    errors::PyErr_GivenExceptionMatches(
                        raised,
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                    ),
                    0
                );
                let message = molt_cpython_abi::api::typeobj::PyObject_Str(raised);
                assert_eq!(
                    std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(message)).to_bytes(),
                    b"future reset rejected"
                );
                refcount::Py_DECREF(message);
                refcount::Py_DECREF(raised);
                dec_ref_bits(py, bits);
                let mro = std::mem::replace(&mut class.tp_mro, std::ptr::null_mut());
                refcount::Py_DECREF(mro);
                refcount::Py_DECREF(class.tp_dict);
                assert_eq!(future.object.ob_refcnt, 1);
                assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
                assert!(!exception_pending(py));
            }
        });
    }

    fn refcount(bits: u64) -> u64 {
        unsafe { (*header_from_obj_ptr(ptr_from_bits(bits))).ref_count_snapshot() as u64 }
    }

    #[test]
    fn native_awaitability_separates_poll_owners_from_python_iterators() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let poll = crate::provenance::abi::expose_function_address(immediate_poll as *const ());
            let coroutine = crate::molt_task_new(poll, 0, crate::TASK_KIND_COROUTINE);
            let future = crate::molt_future_new(poll, 0);
            let generator = crate::molt_task_new(
                poll,
                crate::GEN_CONTROL_SIZE as u64,
                crate::TASK_KIND_GENERATOR,
            );
            let async_generator = crate::molt_asyncgen_new(generator);
            let wrapper = molt_awaitable_await(coroutine);
            let list = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            assert!(!exception_pending(py));
            // CPython's coroutine wrapper is an iterator, not an awaitable;
            // an ordinary generator's poll address does not admit await either.
            for (receiver, admitted) in [
                (coroutine, true),
                (future, true),
                (generator, false),
                (async_generator, false),
                (wrapper, false),
                (list, false),
                (MoltObject::from_int(3).bits(), false),
            ] {
                assert_eq!(
                    crate::molt_is_native_awaitable(receiver),
                    MoltObject::from_bool(admitted).bits()
                );
                let acquired = molt_get_awaitable(receiver);
                if admitted {
                    assert_eq!(acquired, receiver);
                    assert!(!exception_pending(py));
                } else {
                    let error = crate::molt_exception_last();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "TypeError"
                    ));
                    crate::clear_exception(py);
                    dec_ref_bits(py, error);
                }
                dec_ref_bits(py, acquired);
            }
            let adapted = molt_awaitable_await(future);
            assert_eq!(adapted, future);
            assert!(!exception_pending(py));
            dec_ref_bits(py, adapted);
            let closed = molt_coroutine_close_method(coroutine);
            dec_ref_bits(py, closed);
            assert!(!exception_pending(py));
            for value in [list, wrapper, async_generator, generator, future, coroutine] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn nested_resume_scopes_isolate_requests_and_release_owned_values() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let outer_value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let inner_value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let outer_count = refcount(outer_value);
            let inner_count = refcount(inner_value);
            {
                let outer = PythonResumeScope::enter(
                    py,
                    ptr_from_bits(outer_value),
                    ResumeRequest::Send(outer_value),
                    true,
                );
                assert_eq!(refcount(outer_value), outer_count + 2);
                {
                    let _scheduled = PythonResumeScope::scheduled(py);
                    assert!(!direct_resume());
                    assert!(take_request(ptr_from_bits(outer_value)).is_none());
                    let inner = PythonResumeScope::enter(
                        py,
                        ptr_from_bits(inner_value),
                        ResumeRequest::Throw(inner_value),
                        true,
                    );
                    assert!(direct_resume());
                    assert!(take_request(ptr_from_bits(outer_value)).is_none());
                    publish_yield(py, inner_value);
                    let yielded = inner.take_yielded();
                    assert_eq!(yielded, inner_value);
                    dec_ref_bits(py, yielded);
                    assert_eq!(refcount(inner_value), inner_count + 2);
                }
                assert!(direct_resume());
                assert_eq!(refcount(inner_value), inner_count);
                let request = take_request(ptr_from_bits(outer_value)).unwrap();
                assert!(matches!(request, ResumeRequest::Send(value) if value == outer_value));
                dec_ref_bits(py, request.bits());
                publish_yield(py, outer_value);
                assert_eq!(refcount(outer_value), outer_count + 2);
                drop(outer);
            }
            assert_eq!(refcount(outer_value), outer_count);
            assert!(!direct_resume());
            dec_ref_bits(py, outer_value);
            dec_ref_bits(py, inner_value);
        });
    }

    #[test]
    fn adapter_edges_are_owned_until_completion_or_terminal_drop() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let list = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let iterator = crate::molt_iter(list);
            dec_ref_bits(py, list);
            let before = refcount(iterator);
            for clear in [false, true] {
                let adapter = iterator_poll_adapter(py, iterator);
                assert!(!exception_pending(py));
                assert_eq!(refcount(iterator), before + 1);
                let pointer = ptr_from_bits(adapter);
                let mut edges = Vec::new();
                unsafe {
                    crate::object::heap_lifecycle::visit_owned_values(py, pointer, &mut |value| {
                        edges.push(value)
                    });
                }
                assert_eq!(edges.iter().filter(|&&value| value == iterator).count(), 1);
                if clear {
                    unsafe {
                        clear_iterator(py, pointer);
                    }
                    assert_eq!(refcount(iterator), before);
                }
                dec_ref_bits(py, adapter);
                assert_eq!(refcount(iterator), before);
            }
            dec_ref_bits(py, iterator);
        });
    }

    extern "C" fn immediate_poll(_raw: u64) -> i64 {
        MoltObject::from_int(42).bits() as i64
    }

    extern "C" fn raising_poll(raw: u64) -> i64 {
        let ptr = std::ptr::with_exposed_provenance_mut::<u8>(raw as usize);
        crate::molt_raise(unsafe { slot(ptr, 0) }) as i64
    }

    extern "C" fn raising_pending_poll(raw: u64) -> i64 {
        raising_poll(raw);
        pending_bits_i64()
    }

    #[test]
    fn manual_resume_preserves_exception_identity_and_never_caches_success() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for callback in [
                raising_poll as extern "C" fn(u64) -> i64,
                raising_pending_poll,
            ] {
                for nested in [false, true] {
                    let caller = if nested {
                        crate::molt_task_new(
                            crate::provenance::abi::expose_function_address(
                                immediate_poll as *const (),
                            ),
                            0,
                            crate::TASK_KIND_COROUTINE,
                        )
                    } else {
                        MoltObject::none().bits()
                    };
                    let caller_scope = crate::CurrentTaskScope::enter(
                        py,
                        maybe_ptr_from_bits(caller).unwrap_or(std::ptr::null_mut()),
                    );
                    let exception = MoltObject::from_ptr(crate::alloc_exception(
                        py,
                        "LookupError",
                        "manual resume failure",
                    ))
                    .bits();
                    let coroutine = crate::molt_task_new(
                        crate::provenance::abi::expose_function_address(callback as *const ()),
                        std::mem::size_of::<u64>() as u64,
                        crate::TASK_KIND_COROUTINE,
                    );
                    inc_ref_bits(py, exception);
                    unsafe { replace_owned(py, ptr_from_bits(coroutine), 0, exception) };
                    let result = molt_coroutine_send_method(coroutine, MoltObject::none().bits());
                    dec_ref_bits(py, result);
                    assert!(exception_pending(py));
                    let actual = crate::molt_exception_last();
                    assert_eq!(actual, exception);
                    assert!(coroutine_is_done(ptr_from_bits(coroutine)));
                    assert!(crate::task_result_get(py, ptr_from_bits(coroutine)).is_none());
                    if let Some(caller) = maybe_ptr_from_bits(caller) {
                        assert_eq!(
                            crate::object::aux_header::object_frame_awaited_bits(caller),
                            0
                        );
                    }
                    crate::clear_exception(py);
                    dec_ref_bits(py, actual);
                    dec_ref_bits(py, coroutine);
                    dec_ref_bits(py, exception);
                    drop(caller_scope);
                    dec_ref_bits(py, caller);
                    assert!(!exception_pending(py));
                }
            }
        });
    }

    #[test]
    fn injected_await_exception_does_not_poll_or_complete_the_retained_child() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let exception =
                MoltObject::from_ptr(crate::alloc_exception(py, "ValueError", "await injection"))
                    .bits();
            for request in [
                ResumeRequest::Throw(exception),
                ResumeRequest::Inject(exception),
            ] {
                let child = crate::molt_future_new(
                    crate::provenance::abi::expose_function_address(immediate_poll as *const ()),
                    0,
                );
                let parent = crate::molt_task_new(
                    crate::provenance::abi::expose_function_address(immediate_poll as *const ()),
                    0,
                    crate::TASK_KIND_COROUTINE,
                );
                let parent_ptr = ptr_from_bits(parent);
                let child_ptr = ptr_from_bits(child);
                inc_ref_bits(py, child);
                unsafe {
                    crate::object::aux_header::object_replace_frame_awaited_owned(
                        py, parent_ptr, child,
                    );
                }
                let caller = crate::CurrentTaskScope::enter(py, parent_ptr);
                let request = PythonResumeScope::enter(py, child_ptr, request, true);
                let result = crate::molt_future_poll(child);
                dec_ref_bits(py, result as u64);
                assert!(exception_pending(py));
                let actual = crate::molt_exception_last();
                assert_eq!(actual, exception);
                assert!(!coroutine_is_done(child_ptr));
                assert!(crate::task_result_get(py, child_ptr).is_none());
                assert!(!crate::task_last_exception_contains_valid(py, child_ptr));
                assert_eq!(
                    crate::object::aux_header::object_frame_awaited_bits(parent_ptr),
                    0
                );
                crate::clear_exception(py);
                dec_ref_bits(py, actual);
                drop(request);
                drop(caller);
                dec_ref_bits(py, parent);
                dec_ref_bits(py, child);
            }
            dec_ref_bits(py, exception);
        });
    }

    #[test]
    fn coroutine_wrapper_and_send_preserve_completion_value_and_single_use() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let coroutine = crate::molt_task_new(
                crate::provenance::abi::expose_function_address(immediate_poll as *const ()),
                0,
                crate::TASK_KIND_COROUTINE,
            );
            let before = refcount(coroutine);
            let wrapper = molt_awaitable_await(coroutine);
            assert!(is_coroutine_wrapper_bits(wrapper));
            assert!(!is_native_coroutine_bits(wrapper));
            assert_eq!(refcount(coroutine), before + 1);
            let ignored = molt_coroutine_wrapper_next(wrapper);
            dec_ref_bits(py, ignored);
            assert!(exception_pending(py));
            let stop = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                stop,
                "StopIteration"
            ));
            let value = crate::builtins::exceptions::exception_typed_field_get(
                py,
                ptr_from_bits(stop),
                molt_obj_model::ExceptionTypedField::StopIterationValue,
            )
            .unwrap()
            .unwrap();
            assert_eq!(crate::to_i64(obj_from_bits(value)), Some(42));
            dec_ref_bits(py, value);
            crate::clear_exception(py);
            dec_ref_bits(py, stop);
            let ignored = molt_coroutine_send_method(coroutine, MoltObject::none().bits());
            dec_ref_bits(py, ignored);
            let reused = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                reused,
                "RuntimeError"
            ));
            crate::clear_exception(py);
            dec_ref_bits(py, reused);
            dec_ref_bits(py, wrapper);
            assert_eq!(refcount(coroutine), before);
            dec_ref_bits(py, coroutine);
        });
    }
}

/// Shared PEP380 throw delegation. The compiler's yield-from carrier is the
/// same owned (value, done) pair as ordinary generator iteration.
#[unsafe(no_mangle)]
pub extern "C" fn molt_iterator_throw(iterator: u64, arguments: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // Raw argument tuples request delegation. A normalized exception was
        // acquired while this generator was a leaf, possibly before its
        // constructor callback advanced it into this yield-from continuation.
        let request = if crate::builtins::exceptions::exception_is_instance(py, arguments) {
            ResumeRequest::Inject(arguments)
        } else {
            ResumeRequest::Throw(arguments)
        };
        let (value, done) = match iterator_resume(py, iterator, request) {
            Ok(SpecialIterationStep::Item(value)) => (value, false),
            Ok(SpecialIterationStep::Exhausted(value)) => (value, true),
            Ok(SpecialIterationStep::Missing) | Err(()) => return MoltObject::none().bits(),
        };
        let pair = crate::alloc_tuple(py, &[value, MoltObject::from_bool(done).bits()]);
        dec_ref_bits(py, value);
        if pair.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(pair).bits()
        }
    })
}
