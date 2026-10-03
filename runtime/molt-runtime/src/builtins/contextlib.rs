use crate::audit::{AuditArgs, audit_capability_decision};
use crate::builtins::exceptions::{
    ExceptionFieldSlot, ExceptionValue, exception_class, exception_is_instance,
    exception_replace_field_bits, exception_traceback, molt_exception_last_pending,
};
use crate::object::payload_refs;
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_TYPE, attr_name_bits_from_bytes,
    call_callable0, call_callable1, call_callable3, class_dict_bits, class_mro_pinned,
    clear_exception, contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_asyncgen_enter_poll_fn_addr,
    contextlib_asyncgen_exit_poll_fn_addr, dec_ref_bits, dict_get_in_place,
    exception_pending, exception_stack_pop,
    exception_stack_push, has_capability, header_from_obj_ptr, inc_ref_bits, is_missing_bits,
    is_registered_ptr, is_truthy, missing_bits, molt_call_bind, molt_callargs_expand_kwstar,
    molt_callargs_expand_star, molt_callargs_new, molt_future_new, molt_future_poll,
    molt_getattr_builtin, molt_inspect_getasyncgenstate, molt_is_callable, molt_issubclass,
    molt_object_setattr, molt_raise, obj_from_bits, object_type_id,
    opaque_handle_bits, path_from_bits, pending_bits_i64, ptr_from_bits, raise_exception,
    release_ptr, string_obj_to_owned,
};

const ASYNCGEN_ENTER_SLOT_AGEN: usize = 0;
const ASYNCGEN_ENTER_SLOT_AWAIT: usize = 1;

const ASYNCGEN_EXIT_SLOT_AGEN: usize = 0;
const ASYNCGEN_EXIT_SLOT_EXC_TYPE: usize = 1;
const ASYNCGEN_EXIT_SLOT_EXC: usize = 2;
const ASYNCGEN_EXIT_SLOT_TB: usize = 3;
const ASYNCGEN_EXIT_SLOT_AWAIT: usize = 4;
const ASYNCGEN_EXIT_SLOT_MODE: usize = 5;
const ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC: usize = 6;
const ASYNCGEN_EXIT_MODE_ANEXT: i64 = 1;
const ASYNCGEN_EXIT_MODE_THROW: i64 = 2;

const ASYNC_EXITSTACK_ENTER_SLOT_HANDLE: usize = 0;
const ASYNC_EXITSTACK_ENTER_SLOT_CM: usize = 1;
const ASYNC_EXITSTACK_ENTER_SLOT_AWAIT: usize = 2;

const ASYNC_EXITSTACK_SLOT_HANDLE: usize = 0;
const ASYNC_EXITSTACK_SLOT_CUR_TYPE: usize = 1;
const ASYNC_EXITSTACK_SLOT_CUR_EXC: usize = 2;
const ASYNC_EXITSTACK_SLOT_CUR_TB: usize = 3;
const ASYNC_EXITSTACK_SLOT_RECEIVED_EXC: usize = 4;
const ASYNC_EXITSTACK_SLOT_SUPPRESSED: usize = 5;
const ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT: usize = 6;
const ASYNC_EXITSTACK_SLOT_ACTIVE_KIND: usize = 7;
const ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED: usize = 8;
const ASYNC_EXITSTACK_ACTIVE_NONE: i64 = 0;
const ASYNC_EXITSTACK_ACTIVE_EXIT: i64 = 1;
const ASYNC_EXITSTACK_ACTIVE_CALLBACK: i64 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitStackCallbackKind {
    Exit,
    SyncCallback,
    AsyncCallback,
}

#[derive(Clone, Copy, Debug)]
struct ExitStackCallback {
    kind: ExitStackCallbackKind,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
}

impl ExitStackCallback {
    fn exit(callback_bits: u64) -> Self {
        Self {
            kind: ExitStackCallbackKind::Exit,
            callback_bits,
            args_bits: MoltObject::none().bits(),
            kwargs_bits: MoltObject::none().bits(),
        }
    }

    fn async_callback(callback_bits: u64, args_bits: u64, kwargs_bits: u64) -> Self {
        Self {
            kind: ExitStackCallbackKind::AsyncCallback,
            callback_bits,
            args_bits,
            kwargs_bits,
        }
    }

    fn sync_callback(callback_bits: u64, args_bits: u64, kwargs_bits: u64) -> Self {
        Self {
            kind: ExitStackCallbackKind::SyncCallback,
            callback_bits,
            args_bits,
            kwargs_bits,
        }
    }

    fn release_refs(&mut self, _py: &PyToken<'_>) {
        if !obj_from_bits(self.callback_bits).is_none() {
            dec_ref_bits(_py, self.callback_bits);
        }
        if self.kind != ExitStackCallbackKind::Exit {
            if !obj_from_bits(self.args_bits).is_none() {
                dec_ref_bits(_py, self.args_bits);
            }
            if !obj_from_bits(self.kwargs_bits).is_none() {
                dec_ref_bits(_py, self.kwargs_bits);
            }
        }
        self.callback_bits = MoltObject::none().bits();
        self.args_bits = MoltObject::none().bits();
        self.kwargs_bits = MoltObject::none().bits();
    }
}

struct ExitStackHandle {
    callbacks: Vec<ExitStackCallback>,
}

struct AsyncGeneratorContextManagerHandle {
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
    agen_bits: u64,
}

impl AsyncGeneratorContextManagerHandle {
    fn new(func_bits: u64, args_bits: u64, kwargs_bits: u64) -> Self {
        Self {
            func_bits,
            args_bits,
            kwargs_bits,
            agen_bits: MoltObject::none().bits(),
        }
    }

    fn release_refs(&mut self, _py: &PyToken<'_>) {
        if !obj_from_bits(self.func_bits).is_none() {
            dec_ref_bits(_py, self.func_bits);
        }
        if !obj_from_bits(self.args_bits).is_none() {
            dec_ref_bits(_py, self.args_bits);
        }
        if !obj_from_bits(self.kwargs_bits).is_none() {
            dec_ref_bits(_py, self.kwargs_bits);
        }
        if !obj_from_bits(self.agen_bits).is_none() {
            dec_ref_bits(_py, self.agen_bits);
        }
        self.func_bits = MoltObject::none().bits();
        self.args_bits = MoltObject::none().bits();
        self.kwargs_bits = MoltObject::none().bits();
        self.agen_bits = MoltObject::none().bits();
    }
}

fn ptr_live(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    is_registered_ptr(ptr)
}

fn alloc_str_bits(_py: &PyToken<'_>, value: &str) -> Result<u64, u64> {
    let ptr = crate::alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn contextlib_check_methods(
    _py: &PyToken<'_>,
    candidate_bits: u64,
    methods: &[&[u8]],
) -> Result<bool, u64> {
    let candidate = obj_from_bits(candidate_bits);
    let Some(candidate_ptr) = candidate.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "object has no attribute '__mro__'",
        ));
    };
    if unsafe { object_type_id(candidate_ptr) } != TYPE_ID_TYPE {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "object has no attribute '__mro__'",
        ));
    }
    let Some(mro) = (unsafe { class_mro_pinned(_py, candidate_ptr) }) else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "object has no attribute '__mro__'",
        ));
    };

    for method in methods {
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return Err(MoltObject::none().bits());
        };
        let mut found = false;
        let mut non_none = false;
        for class_bits in mro.iter() {
            let class_obj = obj_from_bits(*class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                continue;
            };
            let dict_bits = unsafe { class_dict_bits(class_ptr) };
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                continue;
            }
            if let Some(value_bits) = unsafe { dict_get_in_place(_py, dict_ptr, method_name_bits) }
            {
                found = true;
                non_none = !obj_from_bits(value_bits).is_none();
                break;
            }
        }
        dec_ref_bits(_py, method_name_bits);
        if !found || !non_none {
            return Ok(false);
        }
    }

    Ok(true)
}

#[allow(clippy::mut_from_ref)]
fn exitstack_from_bits_mut<'a>(
    _py: &'a PyToken<'_>,
    handle_bits: u64,
) -> Result<&'a mut ExitStackHandle, u64> {
    let ptr = ptr_from_bits(handle_bits);
    if !ptr_live(ptr) {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid ExitStack handle",
        ));
    }
    Ok(unsafe { &mut *(ptr as *mut ExitStackHandle) })
}

#[allow(clippy::mut_from_ref)]
fn asyncgen_cm_from_bits_mut<'a>(
    _py: &'a PyToken<'_>,
    handle_bits: u64,
) -> Result<&'a mut AsyncGeneratorContextManagerHandle, u64> {
    let ptr = ptr_from_bits(handle_bits);
    if !ptr_live(ptr) {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid async contextmanager handle",
        ));
    }
    Ok(unsafe { &mut *(ptr as *mut AsyncGeneratorContextManagerHandle) })
}

fn take_pending_exception(_py: &PyToken<'_>) -> u64 {
    let exc_bits = molt_exception_last_pending();
    clear_exception(_py);
    exc_bits
}

fn rethrow_with_owned_exception(_py: &PyToken<'_>, exc_bits: u64) -> u64 {
    let raised = molt_raise(exc_bits);
    dec_ref_bits(_py, exc_bits);
    raised
}

fn exit_exception_triple<'a, 'py>(
    py: &'a PyToken<'py>,
    value: ExceptionValue<'a, 'py>,
) -> Option<[ExceptionValue<'a, 'py>; 3]> {
    if !exception_is_instance(py, value.bits()) {
        return raise_exception(py, "TypeError", "value must be an exception instance");
    }
    let class = exception_class(py, value.bits())?;
    let traceback = exception_traceback(py, value.bits())?;
    if exception_pending(py) {
        return None;
    }
    Some([class, value, traceback])
}

fn normalize_exit_exception(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> Result<u64, u64> {
    let value = if !obj_from_bits(exc_bits).is_none() {
        ExceptionValue::pin(_py, exc_bits)
    } else {
        let out = unsafe { call_callable0(_py, exc_type_bits) };
        let value = ExceptionValue::adopt(_py, out);
        if exception_pending(_py) {
            return Err(take_pending_exception(_py));
        }
        value
    };
    if !exception_is_instance(_py, value.bits()) {
        raise_exception::<()>(_py, "TypeError", "calling exception class did not return an exception instance");
        return Err(take_pending_exception(_py));
    }
    if !obj_from_bits(tb_bits).is_none()
        && let Err(message) = exception_replace_field_bits(
            _py, value.bits(), ExceptionFieldSlot::Traceback, tb_bits,
        )
    {
        if !exception_pending(_py) {
            raise_exception::<()>(_py, "TypeError", message);
        }
        return Err(take_pending_exception(_py));
    }
    Ok(value.into_bits())
}

fn call_next_method(_py: &PyToken<'_>, gen_bits: u64) -> u64 {
    let Some(next_name_bits) = attr_name_bits_from_bytes(_py, b"__next__") else {
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let next_bits = molt_getattr_builtin(gen_bits, next_name_bits, missing);
    dec_ref_bits(_py, next_name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let out = unsafe { call_callable0(_py, next_bits) };
    dec_ref_bits(_py, next_bits);
    out
}

fn call_method0(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> u64 {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let method_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let out = unsafe { call_callable0(_py, method_bits) };
    dec_ref_bits(_py, method_bits);
    out
}

fn call_method1(_py: &PyToken<'_>, obj_bits: u64, method: &[u8], arg_bits: u64) -> u64 {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let method_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let out = unsafe { call_callable1(_py, method_bits, arg_bits) };
    dec_ref_bits(_py, method_bits);
    out
}

fn call_method3(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
) -> u64 {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let method_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let out = unsafe { call_callable3(_py, method_bits, arg1_bits, arg2_bits, arg3_bits) };
    dec_ref_bits(_py, method_bits);
    out
}

fn call_with_star_kwargs(
    _py: &PyToken<'_>,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let builder_bits = molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if !obj_from_bits(args_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    if !obj_from_bits(kwargs_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callback_bits, builder_bits)
}

fn contextlib_clear_pending_exception_state(_py: &PyToken<'_>) {
    if exception_pending(_py) {
        clear_exception(_py);
    }
    debug_assert!(!exception_pending(_py));
}

unsafe fn payload_slot(payload_ptr: *mut u64, idx: usize) -> u64 {
    unsafe { *payload_ptr.add(idx) }
}

unsafe fn payload_replace_borrowed(py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize, bits: u64) {
    unsafe {
        payload_refs::store_borrowed(
            py,
            payload_ptr.cast(),
            idx * std::mem::size_of::<u64>(),
            bits,
        );
    }
}

unsafe fn payload_replace_owned(py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize, bits: u64) {
    unsafe {
        payload_refs::store_owned(
            py,
            payload_ptr.cast(),
            idx * std::mem::size_of::<u64>(),
            bits,
        );
    }
}

unsafe fn payload_clear(py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    unsafe {
        payload_replace_owned(py, payload_ptr, idx, MoltObject::none().bits());
    }
}

unsafe fn payload_set_bool(payload_ptr: *mut u64, idx: usize, value: bool) {
    unsafe {
        *payload_ptr.add(idx) = MoltObject::from_bool(value).bits();
    }
}

unsafe fn payload_bool(payload_ptr: *mut u64, idx: usize) -> bool {
    unsafe {
        obj_from_bits(*payload_ptr.add(idx))
            .as_bool()
            .unwrap_or(false)
    }
}

unsafe fn payload_set_i64(payload_ptr: *mut u64, idx: usize, value: i64) {
    unsafe {
        *payload_ptr.add(idx) = MoltObject::from_int(value).bits();
    }
}

unsafe fn payload_i64(payload_ptr: *mut u64, idx: usize) -> i64 {
    unsafe { obj_from_bits(*payload_ptr.add(idx)).as_int().unwrap_or(0) }
}

fn push_exit_callback(_py: &PyToken<'_>, handle: &mut ExitStackHandle, callback_bits: u64) -> u64 {
    let callable_bits = molt_is_callable(callback_bits);
    if !is_truthy(_py, obj_from_bits(callable_bits)) {
        return raise_exception::<u64>(_py, "TypeError", "callback must be callable");
    }
    inc_ref_bits(_py, callback_bits);
    handle
        .callbacks
        .push(ExitStackCallback::exit(callback_bits));
    MoltObject::none().bits()
}

fn push_async_callback(
    _py: &PyToken<'_>,
    handle: &mut ExitStackHandle,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let callable_bits = molt_is_callable(callback_bits);
    if !is_truthy(_py, obj_from_bits(callable_bits)) {
        return raise_exception::<u64>(_py, "TypeError", "callback must be callable");
    }
    inc_ref_bits(_py, callback_bits);
    inc_ref_bits(_py, args_bits);
    inc_ref_bits(_py, kwargs_bits);
    handle.callbacks.push(ExitStackCallback::async_callback(
        callback_bits,
        args_bits,
        kwargs_bits,
    ));
    MoltObject::none().bits()
}

fn push_sync_callback(
    _py: &PyToken<'_>,
    handle: &mut ExitStackHandle,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let callable_bits = molt_is_callable(callback_bits);
    if !is_truthy(_py, obj_from_bits(callable_bits)) {
        return raise_exception::<u64>(_py, "TypeError", "callback must be callable");
    }
    inc_ref_bits(_py, callback_bits);
    inc_ref_bits(_py, args_bits);
    inc_ref_bits(_py, kwargs_bits);
    handle.callbacks.push(ExitStackCallback::sync_callback(
        callback_bits,
        args_bits,
        kwargs_bits,
    ));
    MoltObject::none().bits()
}

fn asyncgen_exit_handle_exception(
    _py: &PyToken<'_>,
    mode: i64,
    raised_bits: u64,
    normalized_exc_bits: u64,
) -> i64 {
    if mode == ASYNCGEN_EXIT_MODE_ANEXT {
        if crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            raised_bits,
            "StopAsyncIteration",
        ) {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(false).bits() as i64;
        }
        return rethrow_with_owned_exception(_py, raised_bits) as i64;
    }
    if mode == ASYNCGEN_EXIT_MODE_THROW {
        if crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            raised_bits,
            "StopAsyncIteration",
        ) {
            let suppress = raised_bits != normalized_exc_bits;
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(suppress).bits() as i64;
        }
        let is_runtime_error = crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            raised_bits,
            "RuntimeError",
        );
        if is_runtime_error && raised_bits == normalized_exc_bits {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(false).bits() as i64;
        }
        if is_runtime_error {
            return rethrow_with_owned_exception(_py, raised_bits) as i64;
        }
        dec_ref_bits(_py, raised_bits);
        return MoltObject::from_bool(false).bits() as i64;
    }
    rethrow_with_owned_exception(_py, raised_bits) as i64
}

// Publish the complete exception state before releasing any displaced owner.
// All three incoming references are owned; callbacks may install a later state.
unsafe fn async_exitstack_publish_current_owned(
    py: &PyToken<'_>,
    payload_ptr: *mut u64,
    current: [u64; 3],
    suppressed: bool,
    exception_owned: bool,
) {
    unsafe {
        let slots = [
            ASYNC_EXITSTACK_SLOT_CUR_TYPE,
            ASYNC_EXITSTACK_SLOT_CUR_EXC,
            ASYNC_EXITSTACK_SLOT_CUR_TB,
        ];
        let previous: [u64; 3] = std::array::from_fn(|index| {
            payload_refs::exchange_owned(
                py,
                payload_ptr.cast(),
                slots[index] * std::mem::size_of::<u64>(),
                current[index],
            )
        });
        payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED, suppressed);
        payload_set_bool(
            payload_ptr,
            ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED,
            exception_owned,
        );
        for bits in previous {
            dec_ref_bits(py, bits);
        }
    }
}

unsafe fn async_exitstack_set_current_exception_owned(
    py: &PyToken<'_>,
    payload_ptr: *mut u64,
    new_exc_bits: u64,
) -> bool {
    let value = ExceptionValue::adopt(py, new_exc_bits);
    let Some(current) = exit_exception_triple(py, value) else {
        return false;
    };
    unsafe {
        async_exitstack_publish_current_owned(
            py,
            payload_ptr,
            current.map(ExceptionValue::into_bits),
            false,
            true,
        );
    }
    true
}

unsafe fn async_exitstack_suppress_current(py: &PyToken<'_>, payload_ptr: *mut u64) {
    unsafe {
        async_exitstack_publish_current_owned(
            py,
            payload_ptr,
            [MoltObject::none().bits(); 3],
            true,
            false,
        );
    }
}

unsafe fn async_exitstack_clear_current(py: &PyToken<'_>, payload_ptr: *mut u64) {
    unsafe {
        let suppressed = payload_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED);
        async_exitstack_publish_current_owned(
            py,
            payload_ptr,
            [MoltObject::none().bits(); 3],
            suppressed,
            false,
        );
    }
}

unsafe fn async_exitstack_replace_active_owned(
    py: &PyToken<'_>,
    payload_ptr: *mut u64,
    awaitable: u64,
    kind: i64,
) {
    unsafe {
        let previous = payload_refs::exchange_owned(
            py,
            payload_ptr.cast(),
            ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT * std::mem::size_of::<u64>(),
            awaitable,
        );
        payload_set_i64(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND, kind);
        dec_ref_bits(py, previous);
    }
}

unsafe fn async_exitstack_result(payload_ptr: *mut u64) -> bool {
    unsafe {
        let received_exc = payload_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_RECEIVED_EXC);
        let suppressed = payload_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED);
        let current_type = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE);
        if received_exc && obj_from_bits(current_type).is_none() {
            return true;
        }
        if obj_from_bits(current_type).is_none() {
            return suppressed;
        }
        false
    }
}

fn async_result_poll_owned(py: &PyToken<'_>, result: u64) -> u64 {
    let result = ExceptionValue::adopt(py, result);
    crate::molt_get_awaitable(result.bits())
}

fn asyncgen_state_closed(_py: &PyToken<'_>, agen_bits: u64) -> Result<bool, u64> {
    let state_bits = molt_inspect_getasyncgenstate(agen_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let state = string_obj_to_owned(obj_from_bits(state_bits)).unwrap_or_default();
    if !obj_from_bits(state_bits).is_none() {
        dec_ref_bits(_py, state_bits);
    }
    Ok(state == "AGEN_CLOSED")
}

extern "C" fn contextlib_closing_enter(payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

extern "C" fn contextlib_closing_exit(payload_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(close_name_bits) = attr_name_bits_from_bytes(_py, b"close") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let close_bits = molt_getattr_builtin(payload_bits, close_name_bits, missing);
        dec_ref_bits(_py, close_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out = unsafe { call_callable0(_py, close_bits) };
        if !obj_from_bits(out).is_none() {
            dec_ref_bits(_py, out);
        }
        dec_ref_bits(_py, close_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_closing(payload_bits: u64) -> u64 {
    let enter_fn = contextlib_closing_enter as *const ();
    let exit_fn = contextlib_closing_exit as *const ();
    crate::molt_context_new(enter_fn, exit_fn, payload_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_aclosing_enter(payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_aclosing_exit(payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { call_method0(_py, payload_bits, b"aclose") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_abstract_enter(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_abstract_aenter(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_abstract_subclasshook(candidate_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match contextlib_check_methods(_py, candidate_bits, &[b"__enter__", b"__exit__"]) {
            Ok(true) => MoltObject::from_bool(true).bits(),
            Ok(false) => crate::molt_not_implemented(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_abstract_async_subclasshook(candidate_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match contextlib_check_methods(_py, candidate_bits, &[b"__aenter__", b"__aexit__"]) {
            Ok(true) => MoltObject::from_bool(true).bits(),
            Ok(false) => crate::molt_not_implemented(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_contextdecorator_call(
    cm_bits: u64,
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        contextlib_clear_pending_exception_state(_py);
        let none_bits = MoltObject::none().bits();
        let entered = ExceptionValue::adopt(_py, call_method0(_py, cm_bits, b"__enter__"));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        drop(entered);

        // ContextDecorator must catch wrapped-body exceptions so __exit__ can decide suppression.
        exception_stack_push();
        let out = ExceptionValue::adopt(_py, call_with_star_kwargs(_py, func_bits, args_bits, kwargs_bits));
        let body_pending = exception_pending(_py);
        exception_stack_pop(_py);
        if !body_pending {
            let exit_out = ExceptionValue::adopt(_py, call_method3(_py, cm_bits, b"__exit__", none_bits, none_bits, none_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            drop(exit_out);
            return out.into_bits();
        }

        let raised = ExceptionValue::adopt(_py, take_pending_exception(_py));
        contextlib_clear_pending_exception_state(_py);
        let Some([raised_type, raised, raised_tb]) = exit_exception_triple(_py, raised) else {
            return MoltObject::none().bits();
        };
        let exit_out = ExceptionValue::adopt(_py, call_method3(
            _py,
            cm_bits,
            b"__exit__",
            raised_type.bits(),
            raised.bits(),
            raised_tb.bits(),
        ));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let suppress = is_truthy(_py, obj_from_bits(exit_out.bits()));
        if exception_pending(_py) { return MoltObject::none().bits(); }
        if suppress {
            return MoltObject::none().bits();
        }
        rethrow_with_owned_exception(_py, raised.into_bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_chdir_enter(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed_read = has_capability(_py, "fs.read");
        audit_capability_decision(
            "contextlib.chdir_enter",
            "fs.read",
            AuditArgs::None,
            allowed_read,
        );
        if !allowed_read {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let allowed_write = has_capability(_py, "fs.write");
        audit_capability_decision(
            "contextlib.chdir_enter",
            "fs.write",
            AuditArgs::None,
            allowed_write,
        );
        if !allowed_write {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
        };
        let old_cwd = match std::env::current_dir() {
            Ok(value) => value,
            Err(err) => return raise_exception::<u64>(_py, "OSError", &err.to_string()),
        };
        if let Err(err) = std::env::set_current_dir(&path) {
            return raise_exception::<u64>(_py, "OSError", &err.to_string());
        }
        let old_text = old_cwd.to_string_lossy().into_owned();
        match alloc_str_bits(_py, &old_text) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_chdir_exit(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed_read = has_capability(_py, "fs.read");
        audit_capability_decision(
            "contextlib.chdir_exit",
            "fs.read",
            AuditArgs::None,
            allowed_read,
        );
        if !allowed_read {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let allowed_write = has_capability(_py, "fs.write");
        audit_capability_decision(
            "contextlib.chdir_exit",
            "fs.write",
            AuditArgs::None,
            allowed_write,
        );
        if !allowed_write {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
        };
        if let Err(err) = std::env::set_current_dir(&path) {
            return raise_exception::<u64>(_py, "OSError", &err.to_string());
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_cm_new(
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !obj_from_bits(func_bits).is_none() {
            inc_ref_bits(_py, func_bits);
        }
        if !obj_from_bits(args_bits).is_none() {
            inc_ref_bits(_py, args_bits);
        }
        if !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        let ptr = Box::into_raw(Box::new(AsyncGeneratorContextManagerHandle::new(
            func_bits,
            args_bits,
            kwargs_bits,
        ))) as *mut u8;
        opaque_handle_bits(ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_cm_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if !ptr_live(ptr) {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let mut handle = unsafe { Box::from_raw(ptr as *mut AsyncGeneratorContextManagerHandle) };
        handle.release_refs(_py);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_cm_aenter(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match asyncgen_cm_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        if obj_from_bits(handle.agen_bits).is_none() {
            let agen_bits =
                call_with_star_kwargs(_py, handle.func_bits, handle.args_bits, handle.kwargs_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !obj_from_bits(handle.agen_bits).is_none() {
                dec_ref_bits(_py, handle.agen_bits);
            }
            handle.agen_bits = agen_bits;
        }
        contextlib_asyncgen_enter_impl(_py, handle.agen_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_cm_aexit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match asyncgen_cm_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        if obj_from_bits(handle.agen_bits).is_none() {
            return MoltObject::from_bool(false).bits();
        }
        contextlib_asyncgen_exit_impl(_py, handle.agen_bits, exc_type_bits, exc_bits, tb_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_generator_enter(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let out = call_next_method(_py, gen_bits);
        if !exception_pending(_py) {
            return out;
        }
        let exc_bits = take_pending_exception(_py);
        if crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            exc_bits,
            "StopIteration",
        ) {
            dec_ref_bits(_py, exc_bits);
            return raise_exception::<u64>(_py, "RuntimeError", "generator didn't yield");
        }
        rethrow_with_owned_exception(_py, exc_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_generator_exit(
    gen_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(exc_type_bits).is_none() {
            let out = call_next_method(_py, gen_bits);
            if !exception_pending(_py) {
                if !obj_from_bits(out).is_none() {
                    dec_ref_bits(_py, out);
                }
                return raise_exception::<u64>(_py, "RuntimeError", "generator didn't stop");
            }
            let raised = take_pending_exception(_py);
            if crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                raised,
                "StopIteration",
            ) {
                dec_ref_bits(_py, raised);
                return MoltObject::from_bool(false).bits();
            }
            return rethrow_with_owned_exception(_py, raised);
        }

        let normalized_exc = match normalize_exit_exception(_py, exc_type_bits, exc_bits, tb_bits) {
            Ok(bits) => bits,
            Err(bits) => return rethrow_with_owned_exception(_py, bits),
        };
        let Some(throw_name_bits) = attr_name_bits_from_bytes(_py, b"throw") else {
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let throw_bits = molt_getattr_builtin(gen_bits, throw_name_bits, missing);
        dec_ref_bits(_py, throw_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::none().bits();
        }
        let out = unsafe { call_callable1(_py, throw_bits, normalized_exc) };
        dec_ref_bits(_py, throw_bits);
        if !exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            dec_ref_bits(_py, normalized_exc);
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "generator didn't stop after throw",
            );
        }
        let raised = take_pending_exception(_py);
        if crate::builtins::exceptions::exception_matches_builtin_name(_py, raised, "StopIteration")
        {
            let suppress = raised != normalized_exc;
            dec_ref_bits(_py, raised);
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::from_bool(suppress).bits();
        }
        if crate::builtins::exceptions::exception_matches_builtin_name(_py, raised, "RuntimeError")
            && raised == normalized_exc
        {
            dec_ref_bits(_py, raised);
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::from_bool(false).bits();
        }
        if crate::builtins::exceptions::exception_matches_builtin_name(_py, raised, "RuntimeError")
        {
            dec_ref_bits(_py, normalized_exc);
            return rethrow_with_owned_exception(_py, raised);
        }
        dec_ref_bits(_py, raised);
        dec_ref_bits(_py, normalized_exc);
        MoltObject::from_bool(false).bits()
    })
}

fn contextlib_asyncgen_enter_impl(_py: &PyToken<'_>, agen_bits: u64) -> u64 {
    let payload = (2 * std::mem::size_of::<u64>()) as u64;
    let future_bits = molt_future_new(contextlib_asyncgen_enter_poll_fn_addr(), payload);
    if obj_from_bits(future_bits).is_none() {
        return future_bits;
    }
    let future_ptr = ptr_from_bits(future_bits);
    if future_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let payload_ptr = future_ptr as *mut u64;
        *payload_ptr.add(ASYNCGEN_ENTER_SLOT_AGEN) = MoltObject::none().bits();
        *payload_ptr.add(ASYNCGEN_ENTER_SLOT_AWAIT) = MoltObject::none().bits();
        payload_replace_borrowed(_py, payload_ptr, ASYNCGEN_ENTER_SLOT_AGEN, agen_bits);
    }
    future_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_enter(agen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { contextlib_asyncgen_enter_impl(_py, agen_bits) })
}

fn contextlib_asyncgen_exit_impl(
    _py: &PyToken<'_>,
    agen_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    let payload = (7 * std::mem::size_of::<u64>()) as u64;
    let future_bits = molt_future_new(contextlib_asyncgen_exit_poll_fn_addr(), payload);
    if obj_from_bits(future_bits).is_none() {
        return future_bits;
    }
    let future_ptr = ptr_from_bits(future_bits);
    if future_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let payload_ptr = future_ptr as *mut u64;
        for idx in 0..7 {
            *payload_ptr.add(idx) = MoltObject::none().bits();
        }
        payload_replace_borrowed(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AGEN, agen_bits);
        payload_replace_borrowed(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_EXC_TYPE, exc_type_bits);
        payload_replace_borrowed(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_EXC, exc_bits);
        payload_replace_borrowed(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_TB, tb_bits);
        payload_set_i64(payload_ptr, ASYNCGEN_EXIT_SLOT_MODE, 0);
    }
    future_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_asyncgen_exit(
    agen_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        contextlib_asyncgen_exit_impl(_py, agen_bits, exc_type_bits, exc_bits, tb_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_suppress_match(exc_type_bits: u64, exceptions_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(exc_type_bits).is_none() {
            return MoltObject::from_bool(false).bits();
        }
        molt_issubclass(exc_type_bits, exceptions_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_redirect_enter(
    sys_bits: u64,
    stream_name_bits: u64,
    new_target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(stream_name) = string_obj_to_owned(obj_from_bits(stream_name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "stream name must be str");
        };
        let Some(name_bits) = attr_name_bits_from_bytes(_py, stream_name.as_bytes()) else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let old_bits = molt_getattr_builtin(sys_bits, name_bits, missing);
        if exception_pending(_py) {
            dec_ref_bits(_py, name_bits);
            return MoltObject::none().bits();
        }
        let _ = molt_object_setattr(sys_bits, name_bits, new_target_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            if !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
            return MoltObject::none().bits();
        }
        old_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_redirect_exit(
    sys_bits: u64,
    stream_name_bits: u64,
    old_target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(stream_name) = string_obj_to_owned(obj_from_bits(stream_name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "stream name must be str");
        };
        let Some(name_bits) = attr_name_bits_from_bytes(_py, stream_name.as_bytes()) else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(sys_bits, name_bits, old_target_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = Box::into_raw(Box::new(ExitStackHandle {
            callbacks: Vec::new(),
        })) as *mut u8;
        opaque_handle_bits(ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if !ptr_live(ptr) {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let mut handle = unsafe { Box::from_raw(ptr as *mut ExitStackHandle) };
        for mut callback in handle.callbacks.drain(..) {
            callback.release_refs(_py);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_push(handle_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_exit_callback(_py, handle, callback_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_push_callback(
    handle_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_sync_callback(_py, handle, callback_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_async_exitstack_push_callback(
    handle_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_async_callback(_py, handle, callback_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_async_exitstack_push_exit(
    handle_bits: u64,
    exit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        let Some(aexit_name_bits) = attr_name_bits_from_bytes(_py, b"__aexit__") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let aexit_bits = molt_getattr_builtin(exit_bits, aexit_name_bits, missing);
        dec_ref_bits(_py, aexit_name_bits);
        let mut attr_missing = false;
        if exception_pending(_py) {
            let raised_bits = take_pending_exception(_py);
            if crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                raised_bits,
                "AttributeError",
            ) {
                dec_ref_bits(_py, raised_bits);
                attr_missing = true;
            } else {
                return rethrow_with_owned_exception(_py, raised_bits);
            }
        }

        let callback_bits = if attr_missing || is_missing_bits(_py, aexit_bits) {
            if !obj_from_bits(aexit_bits).is_none() {
                dec_ref_bits(_py, aexit_bits);
            }
            exit_bits
        } else {
            aexit_bits
        };

        let push_res = push_exit_callback(_py, handle, callback_bits);
        if callback_bits != exit_bits && !obj_from_bits(callback_bits).is_none() {
            dec_ref_bits(_py, callback_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(push_res).is_none() {
            dec_ref_bits(_py, push_res);
        }
        inc_ref_bits(_py, exit_bits);
        exit_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_enter_context(handle_bits: u64, cm_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let entered_bits = call_method0(_py, cm_bits, b"__enter__");
        if exception_pending(_py) {
            let raised_bits = take_pending_exception(_py);
            if crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                raised_bits,
                "AttributeError",
            ) {
                dec_ref_bits(_py, raised_bits);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "object does not support the context manager protocol",
                );
            }
            return rethrow_with_owned_exception(_py, raised_bits);
        }

        let Some(exit_name_bits) = attr_name_bits_from_bytes(_py, b"__exit__") else {
            if !obj_from_bits(entered_bits).is_none() {
                dec_ref_bits(_py, entered_bits);
            }
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let exit_bits = molt_getattr_builtin(cm_bits, exit_name_bits, missing);
        dec_ref_bits(_py, exit_name_bits);
        if exception_pending(_py) {
            let raised_bits = take_pending_exception(_py);
            if !obj_from_bits(entered_bits).is_none() {
                dec_ref_bits(_py, entered_bits);
            }
            if crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                raised_bits,
                "AttributeError",
            ) {
                dec_ref_bits(_py, raised_bits);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "object does not support the context manager protocol",
                );
            }
            return rethrow_with_owned_exception(_py, raised_bits);
        }

        let push_res = {
            let handle = match exitstack_from_bits_mut(_py, handle_bits) {
                Ok(handle) => handle,
                Err(bits) => {
                    dec_ref_bits(_py, exit_bits);
                    if !obj_from_bits(entered_bits).is_none() {
                        dec_ref_bits(_py, entered_bits);
                    }
                    return bits;
                }
            };
            push_exit_callback(_py, handle, exit_bits)
        };
        dec_ref_bits(_py, exit_bits);
        if exception_pending(_py) {
            if !obj_from_bits(entered_bits).is_none() {
                dec_ref_bits(_py, entered_bits);
            }
            return MoltObject::none().bits();
        }
        if !obj_from_bits(push_res).is_none() {
            dec_ref_bits(_py, push_res);
        }
        entered_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_pop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        let Some(mut callback) = handle.callbacks.pop() else {
            return MoltObject::none().bits();
        };
        if callback.kind != ExitStackCallbackKind::Exit {
            callback.release_refs(_py);
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "callback is not a synchronous __exit__",
            );
        }
        callback.callback_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_pop_all(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        let mut new_handle = ExitStackHandle {
            callbacks: Vec::new(),
        };
        std::mem::swap(&mut handle.callbacks, &mut new_handle.callbacks);
        let ptr = Box::into_raw(Box::new(new_handle)) as *mut u8;
        opaque_handle_bits(ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_exitstack_exit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let received_exc = !obj_from_bits(exc_type_bits).is_none();
        let mut current = [exc_type_bits, exc_bits, tb_bits]
            .map(|bits| ExceptionValue::pin(_py, bits));
        let mut current_exc_owned = false;
        let mut suppressed = false;

        loop {
            let callback = {
                let handle = match exitstack_from_bits_mut(_py, handle_bits) {
                    Ok(handle) => handle,
                    Err(bits) => return bits,
                };
                handle.callbacks.pop()
            };
            let Some(mut callback) = callback else {
                break;
            };

            let out = match callback.kind {
                ExitStackCallbackKind::Exit => unsafe {
                    call_callable3(
                        _py,
                        callback.callback_bits,
                        current[0].bits(),
                        current[1].bits(),
                        current[2].bits(),
                    )
                },
                ExitStackCallbackKind::SyncCallback => call_with_star_kwargs(
                    _py,
                    callback.callback_bits,
                    callback.args_bits,
                    callback.kwargs_bits,
                ),
                ExitStackCallbackKind::AsyncCallback => {
                    callback.release_refs(_py);
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "async callback cannot run in ExitStack",
                    );
                }
            };
            callback.release_refs(_py);
            let out = ExceptionValue::adopt(_py, out);
            let callback_suppressed = !exception_pending(_py)
                && callback.kind == ExitStackCallbackKind::Exit
                && is_truthy(_py, obj_from_bits(out.bits()));
            if exception_pending(_py) {
                let new_exc = ExceptionValue::adopt(_py, take_pending_exception(_py));
                let Some(new_current) = exit_exception_triple(_py, new_exc) else {
                    return MoltObject::none().bits();
                };
                current = new_current;
                current_exc_owned = true;
                suppressed = false;
                continue;
            }
            if callback_suppressed {
                current = std::array::from_fn(|_| ExceptionValue::pin(_py, none_bits));
                current_exc_owned = false;
                suppressed = true;
            }
        }

        if current_exc_owned && !obj_from_bits(current[1].bits()).is_none() {
            let [_, value, _] = current;
            return rethrow_with_owned_exception(_py, value.into_bits());
        }

        let result = if received_exc && obj_from_bits(current[0].bits()).is_none() {
            true
        } else if obj_from_bits(current[0].bits()).is_none() {
            suppressed
        } else {
            false
        };
        MoltObject::from_bool(result).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_async_exitstack_enter_context(
    handle_bits: u64,
    cm_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let payload = (3 * std::mem::size_of::<u64>()) as u64;
        let future_bits = molt_future_new(
            contextlib_async_exitstack_enter_context_poll_fn_addr(),
            payload,
        );
        if obj_from_bits(future_bits).is_none() {
            return future_bits;
        }
        let future_ptr = ptr_from_bits(future_bits);
        if future_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let payload_ptr = future_ptr as *mut u64;
            *payload_ptr.add(ASYNC_EXITSTACK_ENTER_SLOT_HANDLE) = handle_bits;
            *payload_ptr.add(ASYNC_EXITSTACK_ENTER_SLOT_CM) = MoltObject::none().bits();
            *payload_ptr.add(ASYNC_EXITSTACK_ENTER_SLOT_AWAIT) = MoltObject::none().bits();
            payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_CM, cm_bits);
        }
        future_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contextlib_async_exitstack_exit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let payload = (9 * std::mem::size_of::<u64>()) as u64;
        let future_bits = molt_future_new(contextlib_async_exitstack_exit_poll_fn_addr(), payload);
        if obj_from_bits(future_bits).is_none() {
            return future_bits;
        }
        let future_ptr = ptr_from_bits(future_bits);
        if future_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let payload_ptr = future_ptr as *mut u64;
            for idx in 0..9 {
                *payload_ptr.add(idx) = MoltObject::none().bits();
            }
            *payload_ptr.add(ASYNC_EXITSTACK_SLOT_HANDLE) = handle_bits;
            inc_ref_bits(_py, exc_type_bits);
            inc_ref_bits(_py, exc_bits);
            inc_ref_bits(_py, tb_bits);
            async_exitstack_publish_current_owned(
                _py,
                payload_ptr,
                [exc_type_bits, exc_bits, tb_bits],
                false,
                false,
            );
            payload_set_bool(
                payload_ptr,
                ASYNC_EXITSTACK_SLOT_RECEIVED_EXC,
                !obj_from_bits(exc_type_bits).is_none(),
            );
            payload_set_i64(
                payload_ptr,
                ASYNC_EXITSTACK_SLOT_ACTIVE_KIND,
                ASYNC_EXITSTACK_ACTIVE_NONE,
            );
        }
        future_bits
    })
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib asyncgen-enter future object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_contextlib_asyncgen_enter_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 2 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;

            if crate::object::object_state(obj_ptr) == 0 {
                let agen_bits = payload_slot(payload_ptr, ASYNCGEN_ENTER_SLOT_AGEN);
                let await_bits = call_method0(_py, agen_bits, b"__anext__");
                if exception_pending(_py) {
                    let raised_bits = take_pending_exception(_py);
                    if crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        raised_bits,
                        "StopAsyncIteration",
                    ) {
                        dec_ref_bits(_py, raised_bits);
                        return raise_exception::<i64>(
                            _py,
                            "RuntimeError",
                            "async generator didn't yield",
                        );
                    }
                    return rethrow_with_owned_exception(_py, raised_bits) as i64;
                }
                let await_bits = async_result_poll_owned(_py, await_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
                payload_replace_owned(_py, payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT, await_bits);
                crate::object::object_set_state(obj_ptr, 1);
            }

            let await_bits = payload_slot(payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT);
            if obj_from_bits(await_bits).is_none() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async state",
                );
            }
            let res = molt_future_poll(await_bits);
            if res == pending_bits_i64() {
                return res;
            }
            payload_clear(_py, payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT);
            if exception_pending(_py) {
                let raised_bits = take_pending_exception(_py);
                if crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    raised_bits,
                    "StopAsyncIteration",
                ) {
                    dec_ref_bits(_py, raised_bits);
                    return raise_exception::<i64>(
                        _py,
                        "RuntimeError",
                        "async generator didn't yield",
                    );
                }
                return rethrow_with_owned_exception(_py, raised_bits) as i64;
            }
            res
        })
    }
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib asyncgen-exit future object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_contextlib_asyncgen_exit_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 7 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;

            if crate::object::object_state(obj_ptr) == 0 {
                let agen_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_AGEN);
                let exc_type_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_EXC_TYPE);
                let exc_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_EXC);
                let tb_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_TB);

                if obj_from_bits(exc_type_bits).is_none() {
                    let await_bits = call_method0(_py, agen_bits, b"__anext__");
                    if exception_pending(_py) {
                        let raised_bits = take_pending_exception(_py);
                        return asyncgen_exit_handle_exception(
                            _py,
                            ASYNCGEN_EXIT_MODE_ANEXT,
                            raised_bits,
                            MoltObject::none().bits(),
                        );
                    }
                    let await_bits = async_result_poll_owned(_py, await_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits() as i64;
                    }
                    payload_replace_owned(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT, await_bits);
                    payload_set_i64(
                        payload_ptr,
                        ASYNCGEN_EXIT_SLOT_MODE,
                        ASYNCGEN_EXIT_MODE_ANEXT,
                    );
                    crate::object::object_set_state(obj_ptr, 1);
                    return pending_bits_i64();
                }

                let normalized_exc =
                    match normalize_exit_exception(_py, exc_type_bits, exc_bits, tb_bits) {
                        Ok(bits) => bits,
                        Err(bits) => return rethrow_with_owned_exception(_py, bits) as i64,
                    };
                payload_replace_owned(
                    _py,
                    payload_ptr,
                    ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC,
                    normalized_exc,
                );
                let await_bits = call_method1(_py, agen_bits, b"athrow", normalized_exc);
                if exception_pending(_py) {
                    let raised_bits = take_pending_exception(_py);
                    return asyncgen_exit_handle_exception(
                        _py,
                        ASYNCGEN_EXIT_MODE_THROW,
                        raised_bits,
                        payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC),
                    );
                }
                let await_bits = async_result_poll_owned(_py, await_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
                payload_replace_owned(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT, await_bits);
                payload_set_i64(
                    payload_ptr,
                    ASYNCGEN_EXIT_SLOT_MODE,
                    ASYNCGEN_EXIT_MODE_THROW,
                );
                crate::object::object_set_state(obj_ptr, 1);
                return pending_bits_i64();
            }

            let await_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT);
            if obj_from_bits(await_bits).is_none() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async state",
                );
            }
            let res = molt_future_poll(await_bits);
            if res == pending_bits_i64() {
                return res;
            }
            payload_clear(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT);

            let mode = payload_i64(payload_ptr, ASYNCGEN_EXIT_SLOT_MODE);
            let normalized_exc_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC);
            if exception_pending(_py) {
                let raised_bits = take_pending_exception(_py);
                return asyncgen_exit_handle_exception(_py, mode, raised_bits, normalized_exc_bits);
            }

            if !obj_from_bits(res as u64).is_none() {
                dec_ref_bits(_py, res as u64);
            }
            if mode == ASYNCGEN_EXIT_MODE_ANEXT {
                return raise_exception::<i64>(_py, "RuntimeError", "async generator didn't stop");
            }
            if mode == ASYNCGEN_EXIT_MODE_THROW {
                let agen_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_AGEN);
                match asyncgen_state_closed(_py, agen_bits) {
                    Ok(true) => return MoltObject::from_bool(true).bits() as i64,
                    Ok(false) => {}
                    Err(bits) => return bits as i64,
                }
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "async generator didn't stop after athrow",
                );
            }
            raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async state")
        })
    }
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib async enter-context future object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_contextlib_async_exitstack_enter_context_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 3 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;

            if crate::object::object_state(obj_ptr) == 0 {
                let cm_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_CM);
                let await_bits = call_method0(_py, cm_bits, b"__aenter__");
                if exception_pending(_py) {
                    let raised_bits = take_pending_exception(_py);
                    if crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        raised_bits,
                        "AttributeError",
                    ) {
                        dec_ref_bits(_py, raised_bits);
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "object does not support the asynchronous context manager protocol",
                        );
                    }
                    return rethrow_with_owned_exception(_py, raised_bits) as i64;
                }
                let await_bits = async_result_poll_owned(_py, await_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
                payload_replace_owned(
                    _py,
                    payload_ptr,
                    ASYNC_EXITSTACK_ENTER_SLOT_AWAIT,
                    await_bits,
                );
                crate::object::object_set_state(obj_ptr, 1);
                return pending_bits_i64();
            }

            let await_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_AWAIT);
            if obj_from_bits(await_bits).is_none() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async state",
                );
            }
            let res = molt_future_poll(await_bits);
            if res == pending_bits_i64() {
                return res;
            }
            payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_AWAIT);
            if exception_pending(_py) {
                return res;
            }

            let cm_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_CM);
            let Some(exit_name_bits) = attr_name_bits_from_bytes(_py, b"__aexit__") else {
                return MoltObject::none().bits() as i64;
            };
            let missing = missing_bits(_py);
            let exit_bits = molt_getattr_builtin(cm_bits, exit_name_bits, missing);
            dec_ref_bits(_py, exit_name_bits);
            if exception_pending(_py) {
                let raised_bits = take_pending_exception(_py);
                if crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    raised_bits,
                    "AttributeError",
                ) {
                    dec_ref_bits(_py, raised_bits);
                    return raise_exception::<i64>(
                        _py,
                        "TypeError",
                        "object does not support the asynchronous context manager protocol",
                    );
                }
                return rethrow_with_owned_exception(_py, raised_bits) as i64;
            }

            let handle_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_HANDLE);
            let push_res = {
                let handle = match exitstack_from_bits_mut(_py, handle_bits) {
                    Ok(handle) => handle,
                    Err(bits) => {
                        dec_ref_bits(_py, exit_bits);
                        return bits as i64;
                    }
                };
                push_exit_callback(_py, handle, exit_bits)
            };
            dec_ref_bits(_py, exit_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
            if !obj_from_bits(push_res).is_none() {
                dec_ref_bits(_py, push_res);
            }
            res
        })
    }
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib async exitstack-exit future object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_contextlib_async_exitstack_exit_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 9 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid contextlib async payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;

            loop {
                let active_await_bits =
                    payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT);
                if !obj_from_bits(active_await_bits).is_none() {
                    let active_kind = payload_i64(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND);
                    let res = molt_future_poll(active_await_bits);
                    if res == pending_bits_i64() {
                        return res;
                    }
                    let res = ExceptionValue::adopt(_py, res as u64);
                    async_exitstack_replace_active_owned(
                        _py,
                        payload_ptr,
                        MoltObject::none().bits(),
                        ASYNC_EXITSTACK_ACTIVE_NONE,
                    );

                    if exception_pending(_py) {
                        let new_exc_bits = take_pending_exception(_py);
                        if !async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits) {
                            return MoltObject::none().bits() as i64;
                        }
                        continue;
                    }

                    if active_kind == ASYNC_EXITSTACK_ACTIVE_EXIT {
                        let callback_suppressed = is_truthy(_py, obj_from_bits(res.bits()));
                        if exception_pending(_py) {
                            let new_exc_bits = take_pending_exception(_py);
                            if !async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits) {
                                return MoltObject::none().bits() as i64;
                            }
                            continue;
                        }
                        if callback_suppressed {
                            async_exitstack_suppress_current(_py, payload_ptr);
                        }
                    }
                    continue;
                }

                let handle_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_HANDLE);
                let callback = {
                    let handle = match exitstack_from_bits_mut(_py, handle_bits) {
                        Ok(handle) => handle,
                        Err(bits) => return bits as i64,
                    };
                    handle.callbacks.pop()
                };
                let Some(mut callback) = callback else {
                    let current_exc_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC);
                    let current_exc_owned =
                        payload_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED);
                    if current_exc_owned && !obj_from_bits(current_exc_bits).is_none() {
                        inc_ref_bits(_py, current_exc_bits);
                        async_exitstack_clear_current(_py, payload_ptr);
                        return rethrow_with_owned_exception(_py, current_exc_bits) as i64;
                    }
                    let result = async_exitstack_result(payload_ptr);
                    async_exitstack_clear_current(_py, payload_ptr);
                    return MoltObject::from_bool(result).bits() as i64;
                };

                let callback_kind = callback.kind;
                let current_type_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE);
                let current_exc_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC);
                let current_tb_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB);

                let out = match callback_kind {
                    ExitStackCallbackKind::Exit => call_callable3(
                        _py,
                        callback.callback_bits,
                        current_type_bits,
                        current_exc_bits,
                        current_tb_bits,
                    ),
                    ExitStackCallbackKind::SyncCallback => {
                        callback.release_refs(_py);
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "synchronous callback cannot run in AsyncExitStack",
                        );
                    }
                    ExitStackCallbackKind::AsyncCallback => call_with_star_kwargs(
                        _py,
                        callback.callback_bits,
                        callback.args_bits,
                        callback.kwargs_bits,
                    ),
                };
                let out = ExceptionValue::adopt(_py, out);
                callback.release_refs(_py);

                if exception_pending(_py) {
                    let new_exc_bits = take_pending_exception(_py);
                    if !async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits) {
                        return MoltObject::none().bits() as i64;
                    }
                    continue;
                }

                let awaitable = async_result_poll_owned(_py, out.into_bits());
                if exception_pending(_py) {
                    let new_exc_bits = take_pending_exception(_py);
                    if !async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits) {
                        return MoltObject::none().bits() as i64;
                    }
                    continue;
                }
                let active_kind = if callback_kind == ExitStackCallbackKind::Exit {
                    ASYNC_EXITSTACK_ACTIVE_EXIT
                } else {
                    ASYNC_EXITSTACK_ACTIVE_CALLBACK
                };
                async_exitstack_replace_active_owned(_py, payload_ptr, awaitable, active_kind);
            }
        })
    }
}

#[cfg(test)]
mod payload_publication_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

    static CALLBACK_OWNER: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXPECT_SUPPRESSED: AtomicBool = AtomicBool::new(false);
    static EXPECT_AWAIT: AtomicU64 = AtomicU64::new(0);
    static EXPECT_KIND: AtomicI64 = AtomicI64::new(0);

    extern "C" fn value(_argument: u64) -> u64 {
        MoltObject::none().bits()
    }

    extern "C" fn exception_released(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let payload = ptr_from_bits(CALLBACK_OWNER.load(Ordering::Relaxed)).cast::<u64>();
            let suppressed = EXPECT_SUPPRESSED.load(Ordering::Relaxed);
            unsafe {
                for index in 0..3 {
                    let expected = if suppressed {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_int(11 + index as i64).bits()
                    };
                    assert_eq!(
                        payload_slot(payload, ASYNC_EXITSTACK_SLOT_CUR_TYPE + index),
                        expected,
                        "callback observed a partially published exception triple"
                    );
                }
                assert_eq!(
                    payload_bool(payload, ASYNC_EXITSTACK_SLOT_SUPPRESSED),
                    suppressed
                );
                assert_eq!(
                    payload_bool(payload, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED),
                    !suppressed
                );
                async_exitstack_publish_current_owned(
                    py,
                    payload,
                    [MoltObject::from_int(77).bits(); 3],
                    false,
                    true,
                );
            }
            CALLBACK_CALLS.fetch_add(1, Ordering::Relaxed);
            MoltObject::none().bits()
        })
    }

    extern "C" fn await_released(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let payload = ptr_from_bits(CALLBACK_OWNER.load(Ordering::Relaxed)).cast::<u64>();
            unsafe {
                assert_eq!(
                    payload_slot(payload, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT),
                    EXPECT_AWAIT.load(Ordering::Relaxed)
                );
                assert_eq!(
                    payload_i64(payload, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND),
                    EXPECT_KIND.load(Ordering::Relaxed)
                );
                async_exitstack_replace_active_owned(
                    py,
                    payload,
                    MoltObject::from_int(77).bits(),
                    ASYNC_EXITSTACK_ACTIVE_EXIT,
                );
            }
            CALLBACK_CALLS.fetch_add(1, Ordering::Relaxed);
            MoltObject::none().bits()
        })
    }

    fn function(py: &PyToken<'_>, address: *const ()) -> u64 {
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

    fn owner() -> u64 {
        let owner = crate::molt_alloc((9 * std::mem::size_of::<u64>()) as u64);
        let ptr = ptr_from_bits(owner).cast::<u64>();
        assert!(!ptr.is_null());
        for index in 0..9 {
            unsafe { ptr.add(index).write(MoltObject::none().bits()) };
        }
        crate::molt_object_publish_initialized(owner);
        owner
    }

    #[test]
    fn payload_reference_exception_publication_retires_the_complete_triple_and_keeps_callback_state()
     {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            // Function objects are weakref-capable ownership probes for the three
            // reference slots; no exception dispatch is performed in this test.
            for suppressed in [false, true] {
                let owner = owner();
                let payload = ptr_from_bits(owner).cast::<u64>();
                let hook = function(py, exception_released as *const ());
                let values: [u64; 3] = std::array::from_fn(|_| function(py, value as *const ()));
                let weak: [u64; 3] = std::array::from_fn(|index| {
                    watched(py, values[index], if index == 0 { hook } else { none })
                });
                unsafe {
                    async_exitstack_publish_current_owned(py, payload, values, false, true);
                    payload_replace_borrowed(py, payload, ASYNC_EXITSTACK_SLOT_CUR_TYPE, values[0]);
                    let retained = crate::molt_weakref_call(weak[0]);
                    assert_eq!(
                        retained, values[0],
                        "borrowed self-assignment lost the sole owner"
                    );
                    payload_replace_owned(py, payload, ASYNC_EXITSTACK_SLOT_CUR_TYPE, retained);
                }
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_CALLS.store(0, Ordering::Relaxed);
                EXPECT_SUPPRESSED.store(suppressed, Ordering::Relaxed);
                unsafe {
                    if suppressed {
                        async_exitstack_suppress_current(py, payload);
                    } else {
                        async_exitstack_publish_current_owned(
                            py,
                            payload,
                            std::array::from_fn(|index| {
                                MoltObject::from_int(11 + index as i64).bits()
                            }),
                            false,
                            true,
                        );
                    }
                    assert_eq!(CALLBACK_CALLS.load(Ordering::Relaxed), 1);
                    for index in 0..3 {
                        assert_eq!(
                            payload_slot(payload, ASYNC_EXITSTACK_SLOT_CUR_TYPE + index),
                            MoltObject::from_int(77).bits()
                        );
                        assert!(
                            obj_from_bits(crate::molt_weakref_call(weak[index])).is_none(),
                            "displaced owner leaked"
                        );
                    }
                    assert!(!payload_bool(payload, ASYNC_EXITSTACK_SLOT_SUPPRESSED));
                    assert!(payload_bool(payload, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED));
                    async_exitstack_clear_current(py, payload);
                }
                CALLBACK_OWNER.store(0, Ordering::Relaxed);
                for bits in weak {
                    dec_ref_bits(py, bits);
                }
                dec_ref_bits(py, hook);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn payload_reference_active_await_publication_keeps_kind_coherent_through_callback_reentry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for kind in [ASYNC_EXITSTACK_ACTIVE_NONE, ASYNC_EXITSTACK_ACTIVE_CALLBACK] {
                let owner = owner();
                let payload = ptr_from_bits(owner).cast::<u64>();
                let old = function(py, value as *const ());
                let hook = function(py, await_released as *const ());
                let weak = watched(py, old, hook);
                unsafe {
                    async_exitstack_replace_active_owned(
                        py,
                        payload,
                        old,
                        ASYNC_EXITSTACK_ACTIVE_EXIT,
                    )
                };
                let next = if kind == ASYNC_EXITSTACK_ACTIVE_NONE {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_int(11).bits()
                };
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_CALLS.store(0, Ordering::Relaxed);
                EXPECT_AWAIT.store(next, Ordering::Relaxed);
                EXPECT_KIND.store(kind, Ordering::Relaxed);
                unsafe {
                    async_exitstack_replace_active_owned(py, payload, next, kind);
                    assert_eq!(CALLBACK_CALLS.load(Ordering::Relaxed), 1);
                    assert_eq!(
                        payload_slot(payload, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT),
                        MoltObject::from_int(77).bits()
                    );
                    assert_eq!(
                        payload_i64(payload, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND),
                        ASYNC_EXITSTACK_ACTIVE_EXIT
                    );
                }
                assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
                CALLBACK_OWNER.store(0, Ordering::Relaxed);
                dec_ref_bits(py, weak);
                dec_ref_bits(py, hook);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }
}
