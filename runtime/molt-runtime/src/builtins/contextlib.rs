use crate::{
    attr_name_bits_from_bytes, bits_from_ptr, call_callable0, call_callable1, call_callable3,
    class_dict_bits, class_mro_ref, clear_exception, clear_exception_state,
    contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_asyncgen_enter_poll_fn_addr,
    contextlib_asyncgen_exit_poll_fn_addr, dec_ref_bits, dict_get_in_place, exception_kind_bits,
    exception_pending, exception_stack_pop, exception_stack_push, exception_trace_bits,
    has_capability, header_from_obj_ptr, inc_ref_bits, is_missing_bits, is_truthy, missing_bits,
    molt_call_bind, molt_callargs_expand_kwstar, molt_callargs_expand_star, molt_callargs_new,
    molt_exception_clear, molt_exception_last, molt_future_new, molt_future_poll,
    molt_getattr_builtin, molt_inspect_getasyncgenstate, molt_inspect_isawaitable,
    molt_is_callable, molt_issubclass, molt_object_setattr, molt_raise, obj_from_bits,
    object_type_id, path_from_bits, pending_bits_i64, ptr_from_bits, raise_exception, release_ptr,
    resolve_ptr, string_obj_to_owned, type_of_bits, MoltObject, PyToken, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_TYPE,
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
    let addr = ptr.expose_provenance() as u64;
    resolve_ptr(addr).is_some()
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
    let Some(mro) = (unsafe { class_mro_ref(candidate_ptr) }) else {
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

fn exception_kind_name(_py: &PyToken<'_>, exc_bits: u64) -> Option<String> {
    let exc_ptr = obj_from_bits(exc_bits).as_ptr()?;
    unsafe {
        let kind_bits = exception_kind_bits(exc_ptr);
        string_obj_to_owned(obj_from_bits(kind_bits))
    }
}

fn rethrow_with_owned_exception(_py: &PyToken<'_>, exc_bits: u64) -> u64 {
    let raised = molt_raise(exc_bits);
    dec_ref_bits(_py, exc_bits);
    raised
}

fn set_traceback_best_effort(_py: &PyToken<'_>, exc_bits: u64, tb_bits: u64) {
    if obj_from_bits(tb_bits).is_none() {
        return;
    }
    let Some(tb_name_bits) = attr_name_bits_from_bytes(_py, b"__traceback__") else {
        return;
    };
    let _ = molt_object_setattr(exc_bits, tb_name_bits, tb_bits);
    dec_ref_bits(_py, tb_name_bits);
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

fn normalize_exit_exception(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> Result<u64, u64> {
    if !obj_from_bits(exc_bits).is_none() {
        inc_ref_bits(_py, exc_bits);
        set_traceback_best_effort(_py, exc_bits, tb_bits);
        return Ok(exc_bits);
    }
    let out = unsafe { call_callable0(_py, exc_type_bits) };
    if exception_pending(_py) {
        let raised = molt_exception_last();
        clear_exception(_py);
        return Err(raised);
    }
    set_traceback_best_effort(_py, out, tb_bits);
    Ok(out)
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
    // Drain pending exception markers before dispatching cleanup callbacks.
    // This keeps __exit__ dispatch deterministic in exceptional control flow.
    for _ in 0..4 {
        if exception_pending(_py) {
            molt_exception_clear();
        }
        clear_exception_state(_py);
        if !exception_pending(_py) {
            break;
        }
    }
}

unsafe fn payload_slot(payload_ptr: *mut u64, idx: usize) -> u64 {
    *payload_ptr.add(idx)
}

unsafe fn payload_replace_borrowed(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    idx: usize,
    bits: u64,
) {
    let slot = payload_ptr.add(idx);
    let old_bits = *slot;
    if !obj_from_bits(old_bits).is_none() {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
    if !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
}

unsafe fn payload_replace_owned(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize, bits: u64) {
    let slot = payload_ptr.add(idx);
    let old_bits = *slot;
    if !obj_from_bits(old_bits).is_none() {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
}

unsafe fn payload_clear(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    payload_replace_borrowed(_py, payload_ptr, idx, MoltObject::none().bits());
}

unsafe fn payload_set_bool(payload_ptr: *mut u64, idx: usize, value: bool) {
    *payload_ptr.add(idx) = MoltObject::from_bool(value).bits();
}

unsafe fn payload_bool(payload_ptr: *mut u64, idx: usize) -> bool {
    obj_from_bits(*payload_ptr.add(idx))
        .as_bool()
        .unwrap_or(false)
}

unsafe fn payload_set_i64(payload_ptr: *mut u64, idx: usize, value: i64) {
    *payload_ptr.add(idx) = MoltObject::from_int(value).bits();
}

unsafe fn payload_i64(payload_ptr: *mut u64, idx: usize) -> i64 {
    obj_from_bits(*payload_ptr.add(idx)).as_int().unwrap_or(0)
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
    let kind = exception_kind_name(_py, raised_bits);
    if mode == ASYNCGEN_EXIT_MODE_ANEXT {
        if kind.as_deref() == Some("StopAsyncIteration") {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(false).bits() as i64;
        }
        return rethrow_with_owned_exception(_py, raised_bits) as i64;
    }
    if mode == ASYNCGEN_EXIT_MODE_THROW {
        if kind.as_deref() == Some("StopAsyncIteration") {
            let suppress = raised_bits != normalized_exc_bits;
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(suppress).bits() as i64;
        }
        if kind.as_deref() == Some("RuntimeError") && raised_bits == normalized_exc_bits {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::from_bool(false).bits() as i64;
        }
        if kind.as_deref() == Some("RuntimeError") {
            return rethrow_with_owned_exception(_py, raised_bits) as i64;
        }
        dec_ref_bits(_py, raised_bits);
        return MoltObject::from_bool(false).bits() as i64;
    }
    rethrow_with_owned_exception(_py, raised_bits) as i64
}

unsafe fn async_exitstack_set_current_exception_owned(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    new_exc_bits: u64,
) {
    let none_bits = MoltObject::none().bits();
    let new_type_bits = type_of_bits(_py, new_exc_bits);
    let new_tb_bits = obj_from_bits(new_exc_bits)
        .as_ptr()
        .map(|ptr| exception_trace_bits(ptr))
        .unwrap_or(none_bits);
    payload_replace_borrowed(
        _py,
        payload_ptr,
        ASYNC_EXITSTACK_SLOT_CUR_TYPE,
        new_type_bits,
    );
    payload_replace_owned(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC, new_exc_bits);
    payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB, new_tb_bits);
    payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED, false);
    payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED, true);
}

unsafe fn async_exitstack_suppress_current(_py: &PyToken<'_>, payload_ptr: *mut u64) {
    let none_bits = MoltObject::none().bits();
    payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE, none_bits);
    payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC, none_bits);
    payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB, none_bits);
    payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED, true);
    payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED, false);
}

unsafe fn async_exitstack_result(payload_ptr: *mut u64) -> bool {
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

fn async_result_is_awaitable(_py: &PyToken<'_>, result_bits: u64) -> bool {
    let awaitable_bits = molt_inspect_isawaitable(result_bits);
    is_truthy(_py, obj_from_bits(awaitable_bits))
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
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

extern "C" fn contextlib_closing_exit(payload_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_closing(payload_bits: u64) -> u64 {
    let enter_fn = contextlib_closing_enter as *const ();
    let exit_fn = contextlib_closing_exit as *const ();
    crate::molt_context_new(enter_fn, exit_fn, payload_bits)
}

#[no_mangle]
pub extern "C" fn molt_contextlib_aclosing_enter(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_aclosing_exit(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { call_method0(_py, payload_bits, b"aclose") })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_abstract_enter(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_abstract_aenter(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_abstract_subclasshook(candidate_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match contextlib_check_methods(_py, candidate_bits, &[b"__enter__", b"__exit__"]) {
            Ok(true) => MoltObject::from_bool(true).bits(),
            Ok(false) => crate::molt_not_implemented(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_abstract_async_subclasshook(candidate_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match contextlib_check_methods(_py, candidate_bits, &[b"__aenter__", b"__aexit__"]) {
            Ok(true) => MoltObject::from_bool(true).bits(),
            Ok(false) => crate::molt_not_implemented(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_contextdecorator_call(
    cm_bits: u64,
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        contextlib_clear_pending_exception_state(_py);
        let none_bits = MoltObject::none().bits();
        let entered_bits = call_method0(_py, cm_bits, b"__enter__");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(entered_bits).is_none() {
            dec_ref_bits(_py, entered_bits);
        }

        // ContextDecorator must catch wrapped-body exceptions so __exit__ can decide suppression.
        exception_stack_push();
        let out_bits = call_with_star_kwargs(_py, func_bits, args_bits, kwargs_bits);
        let body_pending = exception_pending(_py);
        exception_stack_pop(_py);
        if !body_pending {
            let exit_out = call_method3(_py, cm_bits, b"__exit__", none_bits, none_bits, none_bits);
            if exception_pending(_py) {
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                return MoltObject::none().bits();
            }
            if !obj_from_bits(exit_out).is_none() {
                dec_ref_bits(_py, exit_out);
            }
            return out_bits;
        }

        let raised_bits = molt_exception_last();
        contextlib_clear_pending_exception_state(_py);
        let raised_type_bits = obj_from_bits(raised_bits)
            .as_ptr()
            .and_then(|ptr| unsafe {
                if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                    Some(crate::exception_class_bits(ptr))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| type_of_bits(_py, raised_bits));
        let raised_tb_bits = obj_from_bits(raised_bits)
            .as_ptr()
            .map(|ptr| unsafe { exception_trace_bits(ptr) })
            .unwrap_or(none_bits);
        let exit_out = call_method3(
            _py,
            cm_bits,
            b"__exit__",
            raised_type_bits,
            raised_bits,
            raised_tb_bits,
        );
        if exception_pending(_py) {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::none().bits();
        }
        let suppress = is_truthy(_py, obj_from_bits(exit_out));
        if !obj_from_bits(exit_out).is_none() {
            dec_ref_bits(_py, exit_out);
        }
        if suppress {
            dec_ref_bits(_py, raised_bits);
            return MoltObject::none().bits();
        }
        rethrow_with_owned_exception(_py, raised_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_chdir_enter(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        if !has_capability(_py, "fs.write") {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_chdir_exit(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        if !has_capability(_py, "fs.write") {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_cm_new(
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_cm_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_cm_aenter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_cm_aexit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_generator_enter(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let out = call_next_method(_py, gen_bits);
        if !exception_pending(_py) {
            return out;
        }
        let exc_bits = molt_exception_last();
        clear_exception(_py);
        if exception_kind_name(_py, exc_bits).as_deref() == Some("StopIteration") {
            dec_ref_bits(_py, exc_bits);
            return raise_exception::<u64>(_py, "RuntimeError", "generator didn't yield");
        }
        rethrow_with_owned_exception(_py, exc_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_generator_exit(
    gen_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(exc_type_bits).is_none() {
            let out = call_next_method(_py, gen_bits);
            if !exception_pending(_py) {
                if !obj_from_bits(out).is_none() {
                    dec_ref_bits(_py, out);
                }
                return raise_exception::<u64>(_py, "RuntimeError", "generator didn't stop");
            }
            let raised = molt_exception_last();
            clear_exception(_py);
            if exception_kind_name(_py, raised).as_deref() == Some("StopIteration") {
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
        let raised = molt_exception_last();
        clear_exception(_py);
        let kind = exception_kind_name(_py, raised);
        if kind.as_deref() == Some("StopIteration") {
            let suppress = raised != normalized_exc;
            dec_ref_bits(_py, raised);
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::from_bool(suppress).bits();
        }
        if kind.as_deref() == Some("RuntimeError") && raised == normalized_exc {
            dec_ref_bits(_py, raised);
            dec_ref_bits(_py, normalized_exc);
            return MoltObject::from_bool(false).bits();
        }
        if kind.as_deref() == Some("RuntimeError") {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_enter(agen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { contextlib_asyncgen_enter_impl(_py, agen_bits) })
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

#[no_mangle]
pub extern "C" fn molt_contextlib_asyncgen_exit(
    agen_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        contextlib_asyncgen_exit_impl(_py, agen_bits, exc_type_bits, exc_bits, tb_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_suppress_match(exc_type_bits: u64, exceptions_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(exc_type_bits).is_none() {
            return MoltObject::from_bool(false).bits();
        }
        molt_issubclass(exc_type_bits, exceptions_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_redirect_enter(
    sys_bits: u64,
    stream_name_bits: u64,
    new_target_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_redirect_exit(
    sys_bits: u64,
    stream_name_bits: u64,
    old_target_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = Box::into_raw(Box::new(ExitStackHandle {
            callbacks: Vec::new(),
        })) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_push(handle_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_exit_callback(_py, handle, callback_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_push_callback(
    handle_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_sync_callback(_py, handle, callback_bits, args_bits, kwargs_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_async_exitstack_push_callback(
    handle_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        push_async_callback(_py, handle, callback_bits, args_bits, kwargs_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_async_exitstack_push_exit(
    handle_bits: u64,
    exit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
            let raised_bits = molt_exception_last();
            clear_exception(_py);
            if exception_kind_name(_py, raised_bits).as_deref() == Some("AttributeError") {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_enter_context(handle_bits: u64, cm_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let entered_bits = call_method0(_py, cm_bits, b"__enter__");
        if exception_pending(_py) {
            let raised_bits = molt_exception_last();
            clear_exception(_py);
            if exception_kind_name(_py, raised_bits).as_deref() == Some("AttributeError") {
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
            let raised_bits = molt_exception_last();
            clear_exception(_py);
            if !obj_from_bits(entered_bits).is_none() {
                dec_ref_bits(_py, entered_bits);
            }
            if exception_kind_name(_py, raised_bits).as_deref() == Some("AttributeError") {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_pop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_pop_all(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match exitstack_from_bits_mut(_py, handle_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        let mut new_handle = ExitStackHandle {
            callbacks: Vec::new(),
        };
        std::mem::swap(&mut handle.callbacks, &mut new_handle.callbacks);
        let ptr = Box::into_raw(Box::new(new_handle)) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_exitstack_exit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let received_exc = !obj_from_bits(exc_type_bits).is_none();
        let mut current_type = exc_type_bits;
        let mut current_exc = exc_bits;
        let mut current_tb = tb_bits;
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
                        current_type,
                        current_exc,
                        current_tb,
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
                    if current_exc_owned && !obj_from_bits(current_exc).is_none() {
                        dec_ref_bits(_py, current_exc);
                    }
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "async callback cannot run in ExitStack",
                    );
                }
            };
            callback.release_refs(_py);
            if exception_pending(_py) {
                let new_exc_bits = molt_exception_last();
                clear_exception(_py);
                if current_exc_owned && !obj_from_bits(current_exc).is_none() {
                    dec_ref_bits(_py, current_exc);
                }
                current_exc = new_exc_bits;
                current_exc_owned = true;
                current_type = type_of_bits(_py, new_exc_bits);
                current_tb = obj_from_bits(new_exc_bits)
                    .as_ptr()
                    .map(|ptr| unsafe { exception_trace_bits(ptr) })
                    .unwrap_or(none_bits);
                suppressed = false;
                continue;
            }
            let callback_suppressed =
                callback.kind == ExitStackCallbackKind::Exit && is_truthy(_py, obj_from_bits(out));
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            if callback_suppressed {
                if current_exc_owned && !obj_from_bits(current_exc).is_none() {
                    dec_ref_bits(_py, current_exc);
                }
                current_type = none_bits;
                current_exc = none_bits;
                current_tb = none_bits;
                current_exc_owned = false;
                suppressed = true;
            }
        }

        if current_exc_owned && !obj_from_bits(current_exc).is_none() {
            return rethrow_with_owned_exception(_py, current_exc);
        }

        let result = if received_exc && obj_from_bits(current_type).is_none() {
            true
        } else if obj_from_bits(current_type).is_none() {
            suppressed
        } else {
            false
        };
        MoltObject::from_bool(result).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_contextlib_async_exitstack_enter_context(
    handle_bits: u64,
    cm_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_contextlib_async_exitstack_exit(
    handle_bits: u64,
    exc_type_bits: u64,
    exc_bits: u64,
    tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
            payload_replace_borrowed(
                _py,
                payload_ptr,
                ASYNC_EXITSTACK_SLOT_CUR_TYPE,
                exc_type_bits,
            );
            payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC, exc_bits);
            payload_replace_borrowed(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB, tb_bits);
            payload_set_bool(
                payload_ptr,
                ASYNC_EXITSTACK_SLOT_RECEIVED_EXC,
                !obj_from_bits(exc_type_bits).is_none(),
            );
            payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_SUPPRESSED, false);
            payload_set_i64(
                payload_ptr,
                ASYNC_EXITSTACK_SLOT_ACTIVE_KIND,
                ASYNC_EXITSTACK_ACTIVE_NONE,
            );
            payload_set_bool(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC_OWNED, false);
        }
        future_bits
    })
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib asyncgen-enter future object.
#[no_mangle]
pub unsafe extern "C" fn molt_contextlib_asyncgen_enter_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        if payload_bytes < 2 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async payload");
        }
        let payload_ptr = obj_ptr as *mut u64;

        if (*header).state == 0 {
            let agen_bits = payload_slot(payload_ptr, ASYNCGEN_ENTER_SLOT_AGEN);
            let await_bits = call_method0(_py, agen_bits, b"__anext__");
            if exception_pending(_py) {
                let raised_bits = molt_exception_last();
                clear_exception(_py);
                if exception_kind_name(_py, raised_bits).as_deref() == Some("StopAsyncIteration") {
                    dec_ref_bits(_py, raised_bits);
                    return raise_exception::<i64>(
                        _py,
                        "RuntimeError",
                        "async generator didn't yield",
                    );
                }
                return rethrow_with_owned_exception(_py, raised_bits) as i64;
            }
            payload_replace_owned(_py, payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT, await_bits);
            (*header).state = 1;
        }

        let await_bits = payload_slot(payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT);
        if obj_from_bits(await_bits).is_none() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async state");
        }
        let res = molt_future_poll(await_bits);
        if res == pending_bits_i64() {
            return res;
        }
        payload_clear(_py, payload_ptr, ASYNCGEN_ENTER_SLOT_AWAIT);
        if exception_pending(_py) {
            let raised_bits = molt_exception_last();
            clear_exception(_py);
            if exception_kind_name(_py, raised_bits).as_deref() == Some("StopAsyncIteration") {
                dec_ref_bits(_py, raised_bits);
                return raise_exception::<i64>(_py, "RuntimeError", "async generator didn't yield");
            }
            return rethrow_with_owned_exception(_py, raised_bits) as i64;
        }
        res
    })
}

/// # Safety
/// - `obj_bits` must reference a valid contextlib asyncgen-exit future object.
#[no_mangle]
pub unsafe extern "C" fn molt_contextlib_asyncgen_exit_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        if payload_bytes < 7 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async payload");
        }
        let payload_ptr = obj_ptr as *mut u64;

        if (*header).state == 0 {
            let agen_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_AGEN);
            let exc_type_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_EXC_TYPE);
            let exc_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_EXC);
            let tb_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_TB);

            if obj_from_bits(exc_type_bits).is_none() {
                let await_bits = call_method0(_py, agen_bits, b"__anext__");
                if exception_pending(_py) {
                    let raised_bits = molt_exception_last();
                    clear_exception(_py);
                    return asyncgen_exit_handle_exception(
                        _py,
                        ASYNCGEN_EXIT_MODE_ANEXT,
                        raised_bits,
                        MoltObject::none().bits(),
                    );
                }
                payload_replace_owned(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT, await_bits);
                payload_set_i64(
                    payload_ptr,
                    ASYNCGEN_EXIT_SLOT_MODE,
                    ASYNCGEN_EXIT_MODE_ANEXT,
                );
                (*header).state = 1;
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
                let raised_bits = molt_exception_last();
                clear_exception(_py);
                return asyncgen_exit_handle_exception(
                    _py,
                    ASYNCGEN_EXIT_MODE_THROW,
                    raised_bits,
                    payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC),
                );
            }
            payload_replace_owned(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT, await_bits);
            payload_set_i64(
                payload_ptr,
                ASYNCGEN_EXIT_SLOT_MODE,
                ASYNCGEN_EXIT_MODE_THROW,
            );
            (*header).state = 1;
            return pending_bits_i64();
        }

        let await_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT);
        if obj_from_bits(await_bits).is_none() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async state");
        }
        let res = molt_future_poll(await_bits);
        if res == pending_bits_i64() {
            return res;
        }
        payload_clear(_py, payload_ptr, ASYNCGEN_EXIT_SLOT_AWAIT);

        let mode = payload_i64(payload_ptr, ASYNCGEN_EXIT_SLOT_MODE);
        let normalized_exc_bits = payload_slot(payload_ptr, ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC);
        if exception_pending(_py) {
            let raised_bits = molt_exception_last();
            clear_exception(_py);
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

/// # Safety
/// - `obj_bits` must reference a valid contextlib async enter-context future object.
#[no_mangle]
pub unsafe extern "C" fn molt_contextlib_async_exitstack_enter_context_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async payload");
        }
        let payload_ptr = obj_ptr as *mut u64;

        if (*header).state == 0 {
            let cm_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_CM);
            let await_bits = call_method0(_py, cm_bits, b"__aenter__");
            if exception_pending(_py) {
                let raised_bits = molt_exception_last();
                clear_exception(_py);
                if exception_kind_name(_py, raised_bits).as_deref() == Some("AttributeError") {
                    dec_ref_bits(_py, raised_bits);
                    return raise_exception::<i64>(
                        _py,
                        "TypeError",
                        "object does not support the asynchronous context manager protocol",
                    );
                }
                return rethrow_with_owned_exception(_py, raised_bits) as i64;
            }
            payload_replace_owned(
                _py,
                payload_ptr,
                ASYNC_EXITSTACK_ENTER_SLOT_AWAIT,
                await_bits,
            );
            (*header).state = 1;
            return pending_bits_i64();
        }

        let await_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_ENTER_SLOT_AWAIT);
        if obj_from_bits(await_bits).is_none() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async state");
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
            let raised_bits = molt_exception_last();
            clear_exception(_py);
            if exception_kind_name(_py, raised_bits).as_deref() == Some("AttributeError") {
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

/// # Safety
/// - `obj_bits` must reference a valid contextlib async exitstack-exit future object.
#[no_mangle]
pub unsafe extern "C" fn molt_contextlib_async_exitstack_exit_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        if payload_bytes < 9 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid contextlib async payload");
        }
        let payload_ptr = obj_ptr as *mut u64;

        loop {
            let active_await_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT);
            if !obj_from_bits(active_await_bits).is_none() {
                let active_kind = payload_i64(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND);
                let res = molt_future_poll(active_await_bits);
                if res == pending_bits_i64() {
                    return res;
                }
                payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT);
                payload_set_i64(
                    payload_ptr,
                    ASYNC_EXITSTACK_SLOT_ACTIVE_KIND,
                    ASYNC_EXITSTACK_ACTIVE_NONE,
                );

                if exception_pending(_py) {
                    let new_exc_bits = molt_exception_last();
                    clear_exception(_py);
                    async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits);
                    continue;
                }

                if active_kind == ASYNC_EXITSTACK_ACTIVE_EXIT {
                    let callback_suppressed = is_truthy(_py, obj_from_bits(res as u64));
                    if callback_suppressed {
                        async_exitstack_suppress_current(_py, payload_ptr);
                    }
                }
                if !obj_from_bits(res as u64).is_none() {
                    dec_ref_bits(_py, res as u64);
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
                    payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE);
                    payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC);
                    payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB);
                    return rethrow_with_owned_exception(_py, current_exc_bits) as i64;
                }
                let result = async_exitstack_result(payload_ptr);
                payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE);
                payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC);
                payload_clear(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB);
                return MoltObject::from_bool(result).bits() as i64;
            };

            let callback_kind = callback.kind;
            let current_type_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TYPE);
            let current_exc_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_EXC);
            let current_tb_bits = payload_slot(payload_ptr, ASYNC_EXITSTACK_SLOT_CUR_TB);

            let out = match callback_kind {
                ExitStackCallbackKind::Exit => unsafe {
                    call_callable3(
                        _py,
                        callback.callback_bits,
                        current_type_bits,
                        current_exc_bits,
                        current_tb_bits,
                    )
                },
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
            callback.release_refs(_py);

            if exception_pending(_py) {
                let new_exc_bits = molt_exception_last();
                clear_exception(_py);
                async_exitstack_set_current_exception_owned(_py, payload_ptr, new_exc_bits);
                continue;
            }

            if async_result_is_awaitable(_py, out) {
                payload_replace_owned(_py, payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT, out);
                let active_kind = if callback_kind == ExitStackCallbackKind::Exit {
                    ASYNC_EXITSTACK_ACTIVE_EXIT
                } else {
                    ASYNC_EXITSTACK_ACTIVE_CALLBACK
                };
                payload_set_i64(payload_ptr, ASYNC_EXITSTACK_SLOT_ACTIVE_KIND, active_kind);
                continue;
            }

            if callback_kind == ExitStackCallbackKind::Exit {
                let callback_suppressed = is_truthy(_py, obj_from_bits(out));
                if callback_suppressed {
                    async_exitstack_suppress_current(_py, payload_ptr);
                }
            }
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
        }
    })
}

unsafe fn contextlib_drop_payload_slots(_py: &PyToken<'_>, future_ptr: *mut u8, slots: &[usize]) {
    let payload_ptr = future_ptr as *mut u64;
    for idx in slots {
        let bits = *payload_ptr.add(*idx);
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        *payload_ptr.add(*idx) = MoltObject::none().bits();
    }
}

pub(crate) unsafe fn contextlib_asyncgen_enter_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    contextlib_drop_payload_slots(
        _py,
        future_ptr,
        &[ASYNCGEN_ENTER_SLOT_AGEN, ASYNCGEN_ENTER_SLOT_AWAIT],
    );
}

pub(crate) unsafe fn contextlib_asyncgen_exit_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    contextlib_drop_payload_slots(
        _py,
        future_ptr,
        &[
            ASYNCGEN_EXIT_SLOT_AGEN,
            ASYNCGEN_EXIT_SLOT_EXC_TYPE,
            ASYNCGEN_EXIT_SLOT_EXC,
            ASYNCGEN_EXIT_SLOT_TB,
            ASYNCGEN_EXIT_SLOT_AWAIT,
            ASYNCGEN_EXIT_SLOT_NORMALIZED_EXC,
        ],
    );
}

pub(crate) unsafe fn contextlib_async_exitstack_enter_context_task_drop(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) {
    contextlib_drop_payload_slots(
        _py,
        future_ptr,
        &[
            ASYNC_EXITSTACK_ENTER_SLOT_CM,
            ASYNC_EXITSTACK_ENTER_SLOT_AWAIT,
        ],
    );
}

pub(crate) unsafe fn contextlib_async_exitstack_exit_task_drop(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) {
    contextlib_drop_payload_slots(
        _py,
        future_ptr,
        &[
            ASYNC_EXITSTACK_SLOT_CUR_TYPE,
            ASYNC_EXITSTACK_SLOT_CUR_EXC,
            ASYNC_EXITSTACK_SLOT_CUR_TB,
            ASYNC_EXITSTACK_SLOT_ACTIVE_AWAIT,
        ],
    );
}
