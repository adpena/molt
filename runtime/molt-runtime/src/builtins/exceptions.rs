macro_rules! fn_addr {
    ($func:path) => {
        $func as *const () as usize as u64
    };
}

fn debug_oom() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| matches!(std::env::var("MOLT_DEBUG_OOM").ok().as_deref(), Some("1")))
}

use crate::PyToken;
use crate::builtins::frames::{
    frame_stack_top_info, frame_stack_trace_payload_bits, traceback_payload_is_lazy,
};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{
    FRAME_STACK, HEADER_FLAG_TRACEBACK_SUPPRESSED, MoltHeader, PtrSlot, RuntimeState,
    TRACEBACK_SUPPRESS_COUNT, TYPE_ID_CODE, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_class_obj,
    alloc_dict_with_pairs, alloc_list, alloc_object, alloc_string, alloc_tuple,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, builtin_classes, builtin_func_bits,
    bytes_like_slice, call_callable1, call_class_init_with_args, class_break_cycles,
    class_dict_bits, class_name_bits, class_name_for_error, code_filename_bits, code_name_bits,
    context_stack_unwind, current_task_key, current_task_ptr, current_token_id, dec_ref_bits,
    dict_find_entry_fast, dict_get_in_place, dict_hashes, dict_order, dict_set_in_place,
    dict_table, format_obj, format_obj_str, header_from_obj_ptr, inc_ref_bits,
    index_bigint_from_obj, init_atomic_bits, instance_dict_bits, int_bits_from_i64,
    intern_static_name, is_truthy, isinstance_bits, issubclass_bits, maybe_ptr_from_bits,
    module_dict_bits, molt_class_set_base, molt_dec_ref, molt_index, molt_is_callable,
    molt_iter_checked, molt_iter_next, molt_repr_from_obj, molt_str_from_obj, obj_from_bits,
    object_class_bits, object_type_id, profile_enabled, runtime_state, seq_vec, seq_vec_ref,
    string_bytes, string_len, string_obj_to_owned, task_exception_depths,
    task_exception_handler_stacks, task_exception_stacks, task_last_exceptions, to_i64,
    token_is_cancelled, traceback_suppressed, type_name, type_of_bits,
};
use molt_obj_model::MoltObject;
use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};

mod exception_group;
use exception_group::{
    alloc_exception_group_from_class_bits, exception_group_exceptions_bits,
    exception_group_message_bits,
};

mod exception_payload;
pub(crate) use exception_group::{
    molt_exceptiongroup_derive, molt_exceptiongroup_init, molt_exceptiongroup_split,
    molt_exceptiongroup_subgroup,
};
pub(crate) use exception_payload::{
    alloc_exception_from_class_bits, format_exception, format_exception_message,
    format_exception_with_traceback, raise_os_error, raise_os_error_errno,
};
use exception_payload::{oserror_args, unicode_error_fields_from_args, unicode_error_kind};

mod exception_state_abi;
#[cfg(test)]
use exception_state_abi::{exception_last_pending_bits, exception_last_public_bits};
pub(crate) use exception_state_abi::{
    molt_exception_active, molt_exception_clear, molt_exception_last, molt_exception_last_pending,
    molt_exception_pending, molt_raise,
};

pub(crate) trait ExceptionSentinel {
    fn exception_sentinel() -> Self;
}

impl ExceptionSentinel for u64 {
    fn exception_sentinel() -> Self {
        MoltObject::none().bits()
    }
}

impl ExceptionSentinel for i64 {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for i32 {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for usize {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for bool {
    fn exception_sentinel() -> Self {
        false
    }
}

impl ExceptionSentinel for *mut u8 {
    fn exception_sentinel() -> Self {
        std::ptr::null_mut()
    }
}

impl ExceptionSentinel for () {
    fn exception_sentinel() -> Self {}
}

impl<T> ExceptionSentinel for Option<T> {
    fn exception_sentinel() -> Self {
        None
    }
}

thread_local! {
    pub(crate) static EXCEPTION_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    pub(crate) static EXCEPTION_STACK_BASELINE: Cell<usize> = const { Cell::new(0) };
    pub(crate) static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static GENERATOR_EXCEPTION_STACKS: RefCell<HashMap<usize, Vec<u64>>> =
        RefCell::new(HashMap::new());
    pub(crate) static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
    pub(crate) static TASK_RAISE_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

const EXCEPTIONS_OBJECT_SLOT_COUNT: usize = 26;

pub(crate) struct ExceptionsRuntimeState {
    errno_attr_name: AtomicU64,
    strerror_attr_name: AtomicU64,
    filename_attr_name: AtomicU64,
    characters_written_attr_name: AtomicU64,
    exc_group_message_name: AtomicU64,
    exc_group_exceptions_name: AtomicU64,
    unicode_encoding_attr_name: AtomicU64,
    unicode_object_attr_name: AtomicU64,
    unicode_start_attr_name: AtomicU64,
    unicode_end_attr_name: AtomicU64,
    unicode_reason_attr_name: AtomicU64,
    exception_with_traceback: AtomicU64,
    base_exception_class_cache: AtomicU64,
    exception_class_cache: AtomicU64,
    key_error_class_cache: AtomicU64,
    index_error_class_cache: AtomicU64,
    value_error_class_cache: AtomicU64,
    type_error_class_cache: AtomicU64,
    runtime_error_class_cache: AtomicU64,
    stop_iteration_class_cache: AtomicU64,
    stop_async_iteration_class_cache: AtomicU64,
    assertion_error_class_cache: AtomicU64,
    import_error_class_cache: AtomicU64,
    name_error_class_cache: AtomicU64,
    unbound_local_error_class_cache: AtomicU64,
    not_implemented_error_class_cache: AtomicU64,
}

impl ExceptionsRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            errno_attr_name: AtomicU64::new(0),
            strerror_attr_name: AtomicU64::new(0),
            filename_attr_name: AtomicU64::new(0),
            characters_written_attr_name: AtomicU64::new(0),
            exc_group_message_name: AtomicU64::new(0),
            exc_group_exceptions_name: AtomicU64::new(0),
            unicode_encoding_attr_name: AtomicU64::new(0),
            unicode_object_attr_name: AtomicU64::new(0),
            unicode_start_attr_name: AtomicU64::new(0),
            unicode_end_attr_name: AtomicU64::new(0),
            unicode_reason_attr_name: AtomicU64::new(0),
            exception_with_traceback: AtomicU64::new(0),
            base_exception_class_cache: AtomicU64::new(0),
            exception_class_cache: AtomicU64::new(0),
            key_error_class_cache: AtomicU64::new(0),
            index_error_class_cache: AtomicU64::new(0),
            value_error_class_cache: AtomicU64::new(0),
            type_error_class_cache: AtomicU64::new(0),
            runtime_error_class_cache: AtomicU64::new(0),
            stop_iteration_class_cache: AtomicU64::new(0),
            stop_async_iteration_class_cache: AtomicU64::new(0),
            assertion_error_class_cache: AtomicU64::new(0),
            import_error_class_cache: AtomicU64::new(0),
            name_error_class_cache: AtomicU64::new(0),
            unbound_local_error_class_cache: AtomicU64::new(0),
            not_implemented_error_class_cache: AtomicU64::new(0),
        }
    }

    fn object_slots(&self) -> [&AtomicU64; EXCEPTIONS_OBJECT_SLOT_COUNT] {
        [
            &self.errno_attr_name,
            &self.strerror_attr_name,
            &self.filename_attr_name,
            &self.characters_written_attr_name,
            &self.exc_group_message_name,
            &self.exc_group_exceptions_name,
            &self.unicode_encoding_attr_name,
            &self.unicode_object_attr_name,
            &self.unicode_start_attr_name,
            &self.unicode_end_attr_name,
            &self.unicode_reason_attr_name,
            &self.exception_with_traceback,
            &self.base_exception_class_cache,
            &self.exception_class_cache,
            &self.key_error_class_cache,
            &self.index_error_class_cache,
            &self.value_error_class_cache,
            &self.type_error_class_cache,
            &self.runtime_error_class_cache,
            &self.stop_iteration_class_cache,
            &self.stop_async_iteration_class_cache,
            &self.assertion_error_class_cache,
            &self.import_error_class_cache,
            &self.name_error_class_cache,
            &self.unbound_local_error_class_cache,
            &self.not_implemented_error_class_cache,
        ]
    }
}

static STOPASYNC_BT_PRINTED: AtomicBool = AtomicBool::new(false);

fn exceptions_state(_py: &PyToken<'_>) -> &'static ExceptionsRuntimeState {
    &runtime_state(_py).exceptions
}

#[inline]
fn debug_exception_flow() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_FLOW").as_deref() == Ok("1"))
}

#[inline]
fn debug_exception_clear() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_CLEAR").as_deref() == Ok("1"))
}

#[inline]
fn debug_exception_raise() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_RAISE").as_deref() == Ok("1"))
}

#[inline]
fn debug_exception_pending() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_PENDING").as_deref() == Ok("1"))
}

#[inline]
fn debug_exception_rc() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_RC").as_deref() == Ok("1"))
}

#[inline]
fn trace_exception_stack() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_EXCEPTION_STACK").as_deref() == Ok("1"))
}

/// Cached `MOLT_DEBUG_EXCEPTIONS` flag. `record_exception_with_caller_frame`
/// runs on every exception raise, so reading the env var directly there takes
/// the libc environ lock and heap-allocates per raise — a measurable tax in
/// exception-heavy loops. Cache it like the sibling flags above.
#[inline]
fn debug_exceptions() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTIONS").as_deref() == Ok("1"))
}

thread_local! {
    static EXCEPTION_CLEAR_REASON: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

thread_local! {
    /// Stashed col_offset/end_col_offset from the frame stack at the point
    /// an exception was recorded.  The traceback formatter reads this to
    /// produce correct caret annotations.  (-1, -1) = unknown.
    static LAST_EXCEPTION_COL: RefCell<(i64, i64)> = const { RefCell::new((-1, -1)) };
}

pub(crate) fn exception_clear_reason_set(reason: &'static str) {
    EXCEPTION_CLEAR_REASON.with(|cell| {
        *cell.borrow_mut() = Some(reason);
    });
}

fn exception_clear_reason_take() -> Option<&'static str> {
    EXCEPTION_CLEAR_REASON.with(|cell| cell.borrow_mut().take())
}

pub(crate) mod internals {
    use super::{HashMap, Mutex};
    use crate::{PyToken, runtime_state};

    pub(crate) fn module_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).module_cache
    }

    pub(crate) fn exception_type_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).exception_type_cache
    }
}

use internals::{exception_type_cache, module_cache};

pub(crate) fn exception_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__init__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_init,
            fn_addr!(molt_exception_init),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_new,
            fn_addr!(molt_exception_new_bound),
            2,
        )),
        "add_note" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_add_note,
            fn_addr!(molt_exception_add_note),
            2,
        )),
        "with_traceback" => Some(builtin_func_bits(
            _py,
            &exceptions_state(_py).exception_with_traceback,
            fn_addr!(molt_exception_with_traceback),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn exception_group_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__init__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_group_init,
            fn_addr!(molt_exceptiongroup_init),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_group_new,
            fn_addr!(molt_exception_new_bound),
            2,
        )),
        "subgroup" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_group_subgroup,
            fn_addr!(molt_exceptiongroup_subgroup),
            2,
        )),
        "split" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_group_split,
            fn_addr!(molt_exceptiongroup_split),
            2,
        )),
        "derive" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.exception_group_derive,
            fn_addr!(molt_exceptiongroup_derive),
            2,
        )),
        _ => None,
    }
}

#[track_caller]
pub(crate) fn raise_exception<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    kind: &str,
    message: &str,
) -> T {
    if debug_exception_flow() && kind == "TypeError" {
        let loc = std::panic::Location::caller();
        eprintln!(
            "molt exc RAISE_EXCEPTION TypeError at {}:{}:{} ({})",
            loc.file(),
            loc.line(),
            loc.column(),
            message
        );
    }
    if debug_oom() && kind == "MemoryError" {
        let loc = std::panic::Location::caller();
        eprintln!(
            "molt MemoryError at {}:{}:{} ({})",
            loc.file(),
            loc.line(),
            loc.column(),
            message
        );
    }
    let ptr = alloc_exception(_py, kind, message);
    if !ptr.is_null() {
        record_exception_owned(_py, ptr);
    }
    T::exception_sentinel()
}

pub(crate) fn raise_unicode_decode_error<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    encoding: &str,
    object_bits: u64,
    start: usize,
    end: usize,
    reason: &str,
) -> T {
    let encoding_ptr = alloc_string(_py, encoding.as_bytes());
    if encoding_ptr.is_null() {
        return T::exception_sentinel();
    }
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        unsafe { molt_dec_ref(encoding_ptr) };
        return T::exception_sentinel();
    }
    let encoding_bits = MoltObject::from_ptr(encoding_ptr).bits();
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let start_bits = int_bits_from_i64(_py, start as i64);
    let end_bits = int_bits_from_i64(_py, end as i64);
    let args_ptr = alloc_tuple(
        _py,
        &[
            encoding_bits,
            object_bits,
            start_bits,
            end_bits,
            reason_bits,
        ],
    );
    if args_ptr.is_null() {
        dec_ref_bits(_py, encoding_bits);
        dec_ref_bits(_py, reason_bits);
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let class_bits = exception_type_bits_from_name(_py, "UnicodeDecodeError");
    let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
    if !ptr.is_null() {
        record_exception_owned(_py, ptr);
    }
    dec_ref_bits(_py, encoding_bits);
    dec_ref_bits(_py, reason_bits);
    T::exception_sentinel()
}

pub(crate) fn raise_unicode_encode_error<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    encoding: &str,
    object_bits: u64,
    start: usize,
    end: usize,
    reason: &str,
) -> T {
    let encoding_ptr = alloc_string(_py, encoding.as_bytes());
    if encoding_ptr.is_null() {
        return T::exception_sentinel();
    }
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        unsafe { molt_dec_ref(encoding_ptr) };
        return T::exception_sentinel();
    }
    let encoding_bits = MoltObject::from_ptr(encoding_ptr).bits();
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let start_bits = int_bits_from_i64(_py, start as i64);
    let end_bits = int_bits_from_i64(_py, end as i64);
    let args_ptr = alloc_tuple(
        _py,
        &[
            encoding_bits,
            object_bits,
            start_bits,
            end_bits,
            reason_bits,
        ],
    );
    if args_ptr.is_null() {
        dec_ref_bits(_py, encoding_bits);
        dec_ref_bits(_py, reason_bits);
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let class_bits = exception_type_bits_from_name(_py, "UnicodeEncodeError");
    let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
    if !ptr.is_null() {
        record_exception_owned(_py, ptr);
    }
    dec_ref_bits(_py, encoding_bits);
    dec_ref_bits(_py, reason_bits);
    T::exception_sentinel()
}

pub(crate) fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken<'_>, bits: u64) -> T {
    let msg = if obj_from_bits(bits).is_none() {
        "'NoneType' object is not iterable".to_string()
    } else {
        format!(
            "'{}' object is not iterable",
            type_name(_py, obj_from_bits(bits))
        )
    };
    raise_exception::<T>(_py, "TypeError", &msg)
}

pub(crate) fn raise_key_error_with_key<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    key_bits: u64,
) -> T {
    let kind_ptr = alloc_string(_py, b"KeyError");
    if kind_ptr.is_null() {
        return T::exception_sentinel();
    }
    let kind_bits = MoltObject::from_ptr(kind_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[key_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, kind_bits);
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let msg_bits = molt_repr_from_obj(key_bits);
    if obj_from_bits(msg_bits).is_none() {
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, args_bits);
        return T::exception_sentinel();
    }
    let class_bits = exception_type_bits(_py, kind_bits);
    let none_bits = MoltObject::none().bits();
    let ptr = alloc_exception_obj(_py, kind_bits, msg_bits, class_bits, args_bits, none_bits);
    if ptr.is_null() {
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, msg_bits);
        dec_ref_bits(_py, args_bits);
        return T::exception_sentinel();
    }
    record_exception_owned(_py, ptr);
    dec_ref_bits(_py, kind_bits);
    dec_ref_bits(_py, msg_bits);
    dec_ref_bits(_py, args_bits);
    T::exception_sentinel()
}

pub(crate) fn raise_unsupported_inplace<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    op: &str,
    lhs_bits: u64,
    rhs_bits: u64,
) -> T {
    let lhs = type_name(_py, obj_from_bits(lhs_bits));
    let rhs = type_name(_py, obj_from_bits(rhs_bits));
    let msg = format!(
        "unsupported operand type(s) for {}: '{}' and '{}'",
        op, lhs, rhs
    );
    raise_exception::<T>(_py, "TypeError", &msg)
}

pub(crate) fn handle_system_exit(_py: &PyToken<'_>, ptr: *mut u8) -> ! {
    let args_bits = unsafe { exception_args_bits(ptr) };
    let args_obj = obj_from_bits(args_bits);
    let code_bits = if let Some(args_ptr) = args_obj.as_ptr() {
        unsafe {
            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                let args = seq_vec_ref(args_ptr);
                if args.is_empty() {
                    MoltObject::none().bits()
                } else if args.len() == 1 {
                    args[0]
                } else {
                    args_bits
                }
            } else {
                MoltObject::none().bits()
            }
        }
    } else {
        MoltObject::none().bits()
    };
    let code_obj = obj_from_bits(code_bits);
    if code_obj.is_none() {
        std::process::exit(0);
    }
    if let Some(code) = to_i64(code_obj) {
        std::process::exit(code as i32);
    }
    let message = format_obj(_py, code_obj);
    if !message.is_empty() {
        eprintln!("{message}");
    }
    std::process::exit(1);
}

pub(crate) fn alloc_exception(_py: &PyToken<'_>, kind: &str, message: &str) -> *mut u8 {
    let kind_ptr = alloc_string(_py, kind.as_bytes());
    if kind_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let msg_ptr = alloc_string(_py, message.as_bytes());
    if msg_ptr.is_null() {
        unsafe { molt_dec_ref(kind_ptr) };
        return std::ptr::null_mut();
    }
    let kind_bits = MoltObject::from_ptr(kind_ptr).bits();
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let args_ptr = if message.is_empty() {
        alloc_tuple(_py, &[])
    } else {
        alloc_tuple(_py, &[msg_bits])
    };
    if args_ptr.is_null() {
        unsafe {
            molt_dec_ref(kind_ptr);
            molt_dec_ref(msg_ptr);
        }
        return std::ptr::null_mut();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let class_bits = exception_type_bits(_py, kind_bits);
    let none_bits = MoltObject::none().bits();
    let ptr = alloc_exception_obj(_py, kind_bits, msg_bits, class_bits, args_bits, none_bits);
    if !ptr.is_null() {
        unsafe {
            exception_set_stop_iteration_value(_py, ptr, args_bits);
            exception_set_system_exit_code(_py, ptr, args_bits);
        }
    }
    dec_ref_bits(_py, kind_bits);
    dec_ref_bits(_py, msg_bits);
    dec_ref_bits(_py, args_bits);
    ptr
}

pub(crate) fn alloc_exception_obj(
    _py: &PyToken<'_>,
    kind_bits: u64,
    msg_bits: u64,
    class_bits: u64,
    args_bits: u64,
    dict_bits: u64,
) -> *mut u8 {
    alloc_exception_obj_with_args_payload(
        _py,
        kind_bits,
        msg_bits,
        class_bits,
        args_bits,
        dict_bits,
        MoltObject::none().bits(),
    )
}

fn alloc_exception_obj_with_args_payload(
    _py: &PyToken<'_>,
    kind_bits: u64,
    msg_bits: u64,
    class_bits: u64,
    args_bits: u64,
    dict_bits: u64,
    args_payload_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 11 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_EXCEPTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = kind_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = msg_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) =
            MoltObject::from_bool(false).bits();
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = class_bits;
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64) = args_bits;
        *(ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        *(ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64) = args_payload_bits;
        inc_ref_bits(_py, kind_bits);
        inc_ref_bits(_py, msg_bits);
        inc_ref_bits(_py, MoltObject::none().bits());
        inc_ref_bits(_py, MoltObject::none().bits());
        inc_ref_bits(_py, MoltObject::from_bool(false).bits());
        inc_ref_bits(_py, MoltObject::none().bits());
        inc_ref_bits(_py, MoltObject::none().bits());
        inc_ref_bits(_py, class_bits);
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, dict_bits);
        inc_ref_bits(_py, args_payload_bits);
    }
    ptr
}

pub(crate) unsafe fn exception_kind_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn exception_msg_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

#[inline]
fn exception_lazy_message_bits() -> u64 {
    MoltObject::pending().bits()
}

#[inline]
pub(crate) fn exception_message_is_lazy(bits: u64) -> bool {
    bits == exception_lazy_message_bits()
}

#[inline]
fn exception_lazy_single_args_bits() -> u64 {
    MoltObject::pending().bits()
}

#[inline]
pub(crate) fn exception_args_is_lazy_single(bits: u64) -> bool {
    bits == exception_lazy_single_args_bits()
}

fn exception_should_defer_message(_py: &PyToken<'_>, kind_bits: u64, class_bits: u64) -> bool {
    if let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits))
        && matches!(
            kind.as_str(),
            "SyntaxError" | "IndentationError" | "TabError"
        )
    {
        return false;
    }
    let base_group_bits = builtin_classes(_py).base_exception_group;
    if base_group_bits != 0 && issubclass_bits(class_bits, base_group_bits) {
        return false;
    }
    unsafe {
        crate::object::ops_format::exception_class_bits_uses_cached_message_str(_py, class_bits)
    }
}

pub(crate) fn exception_message_for_storage(
    _py: &PyToken<'_>,
    kind_bits: u64,
    class_bits: u64,
    args_bits: u64,
) -> u64 {
    if exception_should_defer_message(_py, kind_bits, class_bits) {
        exception_lazy_message_bits()
    } else {
        exception_message_from_args(_py, args_bits)
    }
}

pub(crate) fn exception_materialized_message_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let raw_bits = unsafe { exception_msg_bits(ptr) };
    if !exception_message_is_lazy(raw_bits) {
        return raw_bits;
    }
    let msg_bits = exception_message_from_exception_args(_py, ptr);
    if obj_from_bits(msg_bits).is_none() {
        return msg_bits;
    }
    unsafe {
        let msg_slot = ptr.add(std::mem::size_of::<u64>()) as *mut u64;
        let old_msg = *msg_slot;
        if old_msg != msg_bits {
            dec_ref_bits(_py, old_msg);
            *msg_slot = msg_bits;
        }
    }
    msg_bits
}

pub(crate) unsafe fn exception_cause_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_context_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_suppress_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_trace_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_value_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_class_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) fn exception_match_class_bits(_py: &PyToken<'_>, exc_bits: u64) -> u64 {
    let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return type_of_bits(_py, exc_bits);
        }
        let class_bits = exception_class_bits(exc_ptr);
        if class_bits != 0 && obj_from_bits(class_bits).as_ptr().is_some() {
            return class_bits;
        }
        exception_type_bits(_py, exception_kind_bits(exc_ptr))
    }
}

pub(crate) fn exception_matches_type(_py: &PyToken<'_>, exc_bits: u64, exc_type_bits: u64) -> bool {
    let Some(exc_type_ptr) = obj_from_bits(exc_type_bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(exc_type_ptr) != TYPE_ID_TYPE {
            return false;
        }
    }
    let class_bits = exception_match_class_bits(_py, exc_bits);
    class_bits != 0
        && obj_from_bits(class_bits).as_ptr().is_some()
        && issubclass_bits(class_bits, exc_type_bits)
}

pub(crate) fn exception_matches_builtin_name(_py: &PyToken<'_>, exc_bits: u64, name: &str) -> bool {
    let target_bits = exception_type_bits_from_name(_py, name);
    if target_bits != 0 && exception_matches_type(_py, exc_bits, target_bits) {
        return true;
    }
    let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return false;
        }
        let kind_bits = exception_kind_bits(exc_ptr);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        kind.as_deref() == Some(name)
    }
}

pub(crate) unsafe fn exception_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(8 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) fn exception_materialized_args_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let raw_bits = unsafe { exception_args_bits(ptr) };
    if exception_args_is_lazy_single(raw_bits) {
        let payload_bits = unsafe { exception_args_payload_bits(ptr) };
        let tuple_ptr = alloc_tuple(_py, &[payload_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let new_bits = MoltObject::from_ptr(tuple_ptr).bits();
        unsafe {
            let args_slot = ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
            *args_slot = new_bits;
            exception_set_args_payload_slot(_py, ptr, MoltObject::none().bits());
        }
        return new_bits;
    }
    if obj_from_bits(raw_bits).is_none() || raw_bits == 0 {
        let tuple_ptr = alloc_tuple(_py, &[]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let new_bits = MoltObject::from_ptr(tuple_ptr).bits();
        unsafe {
            let args_slot = ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
            *args_slot = new_bits;
        }
        return new_bits;
    }
    raw_bits
}

pub(crate) unsafe fn exception_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(9 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_args_payload_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(10 * std::mem::size_of::<u64>()) as *const u64) }
}

#[inline]
fn exception_slot_is_valid(ptr: PtrSlot) -> bool {
    let bits = MoltObject::from_ptr(ptr.0).bits();
    let Some(live_ptr) = maybe_ptr_from_bits(bits) else {
        if std::env::var("MOLT_TRACE_EXC_VALID").as_deref() == Ok("1") {
            eprintln!(
                "[EXC_VALID] ptr=0x{:x} bits=0x{:x} -> not a pointer",
                ptr.0 as usize, bits
            );
        }
        return false;
    };
    let tid = unsafe { object_type_id(live_ptr) };
    let valid = tid == TYPE_ID_EXCEPTION;
    if !valid && std::env::var("MOLT_TRACE_EXC_VALID").as_deref() == Ok("1") {
        eprintln!(
            "[EXC_VALID] ptr=0x{:x} type_id={} expected={} -> INVALID",
            ptr.0 as usize, tid, TYPE_ID_EXCEPTION
        );
    }
    valid
}

#[inline]
fn global_last_exception_raw_slot(_py: &PyToken<'_>) -> Option<PtrSlot> {
    let ptr = runtime_state(_py)
        .last_exception
        .load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(PtrSlot(ptr))
    }
}

#[inline]
fn global_last_exception_valid_slot(_py: &PyToken<'_>) -> Option<PtrSlot> {
    let state = runtime_state(_py);
    let ptr = state.last_exception.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        return None;
    }
    let slot = PtrSlot(ptr);
    if exception_slot_is_valid(slot) {
        return Some(slot);
    }
    if state
        .last_exception
        .compare_exchange(
            ptr,
            std::ptr::null_mut(),
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        )
        .is_ok()
    {
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Release);
    }
    None
}

#[inline]
fn global_last_exception_pending_slot(_py: &PyToken<'_>) -> Option<PtrSlot> {
    let state = runtime_state(_py);
    if !state.last_exception_pending.load(AtomicOrdering::Acquire) {
        return None;
    }
    let slot = global_last_exception_valid_slot(_py);
    if slot.is_none() {
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Release);
    }
    slot
}

#[inline]
fn global_last_exception_take(_py: &PyToken<'_>) -> Option<PtrSlot> {
    let state = runtime_state(_py);
    let ptr = state
        .last_exception
        .swap(std::ptr::null_mut(), AtomicOrdering::AcqRel);
    state
        .last_exception_pending
        .store(false, AtomicOrdering::Release);
    if ptr.is_null() {
        None
    } else {
        Some(PtrSlot(ptr))
    }
}

#[inline]
fn global_last_exception_store_recorded(_py: &PyToken<'_>, ptr: *mut u8, reuse_existing_ref: bool) {
    let state = runtime_state(_py);
    if !reuse_existing_ref {
        let bits = MoltObject::from_ptr(ptr).bits();
        inc_ref_bits(_py, bits);
    }
    state.last_exception.store(ptr, AtomicOrdering::Release);
    state
        .last_exception_pending
        .store(true, AtomicOrdering::Release);
}

#[inline]
fn global_last_exception_replace_borrowed(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    let state = runtime_state(_py);
    let old = state.last_exception.load(AtomicOrdering::Acquire);
    if old == ptr {
        state
            .last_exception_pending
            .store(true, AtomicOrdering::Release);
        return;
    }
    inc_ref_bits(_py, bits);
    let old = state.last_exception.swap(ptr, AtomicOrdering::AcqRel);
    state
        .last_exception_pending
        .store(true, AtomicOrdering::Release);
    if !old.is_null() && old != ptr {
        let old_bits = MoltObject::from_ptr(old).bits();
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn global_last_exception_bits_noinc(_py: &PyToken<'_>) -> Option<u64> {
    global_last_exception_raw_slot(_py).map(|ptr| MoltObject::from_ptr(ptr.0).bits())
}

pub(crate) fn exception_pending(_py: &PyToken<'_>) -> bool {
    let state = runtime_state(_py);
    let debug_pending = debug_exception_pending();
    if let Some(task_key) = current_task_key() {
        let pending_ptr = if state
            .task_last_exception_pending
            .load(AtomicOrdering::Relaxed)
        {
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            match guard.get(&task_key).copied() {
                Some(ptr) if exception_slot_is_valid(ptr) => Some(ptr),
                Some(_) => {
                    guard.remove(&task_key);
                    if guard.is_empty() {
                        state
                            .task_last_exception_pending
                            .store(false, AtomicOrdering::Relaxed);
                    }
                    None
                }
                None => {
                    if guard.is_empty() {
                        state
                            .task_last_exception_pending
                            .store(false, AtomicOrdering::Relaxed);
                    }
                    None
                }
            }
        } else {
            None
        };
        let global_pending = global_last_exception_pending_slot(_py).is_some();
        let pending = pending_ptr.is_some() || global_pending;
        if debug_pending
            && pending
            && let Some(ptr) = pending_ptr
        {
            let kind_bits = unsafe { exception_kind_bits(ptr.0) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            eprintln!(
                "molt exc pending task=0x{:x} kind={}",
                task_key.0 as usize, kind
            );
        }
        return pending;
    }
    let pending = global_last_exception_pending_slot(_py).is_some();
    if debug_pending
        && pending
        && let Some(ptr) = global_last_exception_raw_slot(_py)
    {
        let kind_bits = unsafe { exception_kind_bits(ptr.0) };
        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!("molt exc pending task=0x0 kind={}", kind);
    }
    pending
}

pub(crate) fn exception_is_rooted(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    crate::gil_assert();
    if ptr.is_null() {
        return false;
    }
    let state = runtime_state(_py);
    if state.last_exception.load(AtomicOrdering::Acquire) == ptr {
        return true;
    }
    if task_last_exceptions(_py)
        .lock()
        .unwrap()
        .values()
        .any(|slot| slot.0 == ptr)
    {
        return true;
    }
    // Use try_with to avoid panicking when TLS is being destroyed
    // (e.g., during ThreadLocalGuard::drop after an exception).
    // If TLS is destroyed, conservatively treat the exception as rooted.
    if ACTIVE_EXCEPTION_STACK
        .try_with(|stack| {
            let Ok(stack) = stack.try_borrow() else {
                // If the stack is mutably borrowed, we are in exception-stack mutation and must
                // conservatively keep exception objects alive.
                return true;
            };
            stack
                .iter()
                .copied()
                .filter_map(|bits| obj_from_bits(bits).as_ptr())
                .any(|p| p == ptr)
        })
        .unwrap_or(true)
    {
        return true;
    }
    ACTIVE_EXCEPTION_FALLBACK
        .try_with(|stack| {
            let Ok(stack) = stack.try_borrow() else {
                return true;
            };
            stack
                .iter()
                .copied()
                .filter_map(|bits| obj_from_bits(bits).as_ptr())
                .any(|p| p == ptr)
        })
        .unwrap_or(true)
}

pub(crate) fn exception_last_bits_noinc(_py: &PyToken<'_>) -> Option<u64> {
    if let Some(task_key) = current_task_key()
        && let Some(ptr) = task_last_exceptions(_py)
            .lock()
            .unwrap()
            .get(&task_key)
            .copied()
    {
        return Some(MoltObject::from_ptr(ptr.0).bits());
    }
    global_last_exception_bits_noinc(_py)
}

pub(crate) fn clear_exception_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    let ptr = global_last_exception_take(_py);
    if let Some(ptr) = ptr {
        let bits = MoltObject::from_ptr(ptr.0).bits();
        dec_ref_bits(_py, bits);
    }
}

pub(crate) fn clear_exception_type_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let types = {
        let mut guard = state.exception_type_cache.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in types {
        class_break_cycles(_py, bits);
        dec_ref_bits(_py, bits);
    }
}

pub(crate) fn exceptions_clear_runtime_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.exceptions.object_slots();
    crate::state::cache::clear_atomic_slots(_py, &slots);
}

pub(crate) fn exception_handler_active() -> bool {
    // Use try_with to avoid panicking during TLS destruction.
    EXCEPTION_STACK
        .try_with(|stack| !stack.borrow().is_empty())
        .unwrap_or(false)
}

pub(crate) fn exception_stack_baseline_get() -> usize {
    EXCEPTION_STACK_BASELINE
        .try_with(|cell| cell.get())
        .unwrap_or(0)
}

pub(crate) fn exception_stack_baseline_set(baseline: usize) {
    EXCEPTION_STACK_BASELINE.with(|cell| cell.set(baseline));
}

pub(crate) fn exception_context_active_bits() -> Option<u64> {
    let active = ACTIVE_EXCEPTION_STACK.with(|stack| {
        let stack = stack.borrow();
        stack.iter().rev().find_map(|bits| {
            if obj_from_bits(*bits).is_none() {
                None
            } else {
                Some(*bits)
            }
        })
    });
    if active.is_some() {
        return active;
    }
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        let stack = stack.borrow();
        stack.iter().rev().find_map(|bits| {
            if obj_from_bits(*bits).is_none() {
                None
            } else {
                Some(*bits)
            }
        })
    })
}

pub(crate) fn exception_context_set(_py: &PyToken<'_>, bits: u64) {
    crate::gil_assert();
    if debug_exception_flow() {
        let kind = obj_from_bits(bits)
            .as_ptr()
            .map(|ptr| unsafe { exception_kind_bits(ptr) })
            .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
            .unwrap_or_else(|| type_name(_py, obj_from_bits(bits)).into_owned());
        eprintln!("molt exc context_set kind={} bits=0x{:x}", kind, bits);
    }
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(slot) = stack.last_mut() else {
            return;
        };
        if obj_from_bits(bits).is_none() {
            if !obj_from_bits(*slot).is_none() {
                dec_ref_bits(_py, *slot);
            }
            *slot = MoltObject::none().bits();
            return;
        }
        if *slot == bits {
            return;
        }
        if !obj_from_bits(*slot).is_none() {
            dec_ref_bits(_py, *slot);
        }
        inc_ref_bits(_py, bits);
        *slot = bits;
    });
}

pub(crate) fn exception_context_align_depth(_py: &PyToken<'_>, target: usize) {
    crate::gil_assert();
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        while stack.len() > target {
            if let Some(bits) = stack.pop()
                && !obj_from_bits(bits).is_none()
            {
                dec_ref_bits(_py, bits);
            }
        }
        while stack.len() < target {
            stack.push(MoltObject::none().bits());
        }
    });
}

pub(crate) fn exception_context_fallback_push(bits: u64) {
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        stack.borrow_mut().push(bits);
    });
}

pub(crate) fn exception_context_fallback_pop() {
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        let _ = stack.borrow_mut().pop();
    });
}

pub(crate) fn exception_stack_push() {
    let handler_frame_index = FRAME_STACK.with(|stack| stack.borrow().len().saturating_sub(1));
    EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(handler_frame_index);
    });
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(MoltObject::none().bits());
    });
    if trace_exception_stack() {
        let depth = exception_stack_depth();
        let baseline = exception_stack_baseline_get();
        let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
        let (code_bits, line) = FRAME_STACK
            .with(|stack| {
                stack
                    .borrow()
                    .last()
                    .map(|frame| (frame.code_bits, frame.line))
            })
            .unwrap_or((0, 0));
        eprintln!(
            "molt exc stack push task=0x{:x} depth={} baseline={} frame=0x{:x} line={}",
            task, depth, baseline, code_bits as usize, line
        );
    }
}

pub(crate) fn exception_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
    let trace = trace_exception_stack();
    let before_depth = if trace { exception_stack_depth() } else { 0 };
    // Respect the per-function baseline: a function may only pop handlers
    // that it pushed above the baseline captured at function entry.  The
    // frontend's codegen for try/except can emit redundant or stale
    // EXCEPTION_POP ops on join/cleanup paths after a handled exception
    // has already unwound the handler stack — e.g. the handler-entry pop
    // plus a fallthrough cleanup pop after `except: pass` exits.  Treat
    // any pop at or below the baseline as a no-op rather than raising
    // "exception handler stack underflow", which would corrupt the
    // pending-exception state during bootstrap/import and surface as a
    // spurious RuntimeError in a downstream simple `try/except: pass`.
    let (current_depth, baseline) = (exception_stack_depth(), exception_stack_baseline_get());
    if current_depth == 0 || current_depth < baseline {
        if current_depth == 0 && token_is_cancelled(_py, current_token_id()) {
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                for bits in stack.drain(..) {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
            });
            exception_context_align_depth(_py, 0);
        }
        if trace {
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            let (code_bits, line) = FRAME_STACK
                .with(|stack| {
                    stack
                        .borrow()
                        .last()
                        .map(|frame| (frame.code_bits, frame.line))
                })
                .unwrap_or((0, 0));
            eprintln!(
                "molt exc stack pop noop task=0x{:x} depth={} baseline={} frame=0x{:x} line={}",
                task, before_depth, baseline, code_bits as usize, line
            );
        }
        return;
    }
    EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(bits) = stack.pop()
            && !obj_from_bits(bits).is_none()
        {
            dec_ref_bits(_py, bits);
        }
    });
    if trace {
        let after_depth = exception_stack_depth();
        let baseline = exception_stack_baseline_get();
        let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
        let (code_bits, line) = FRAME_STACK
            .with(|stack| {
                stack
                    .borrow()
                    .last()
                    .map(|frame| (frame.code_bits, frame.line))
            })
            .unwrap_or((0, 0));
        eprintln!(
            "molt exc stack pop task=0x{:x} depth={}=>{} baseline={} frame=0x{:x} line={}",
            task, before_depth, after_depth, baseline, code_bits as usize, line
        );
    }
}

/// Pop one synthetic exception handler and restore a captured exception as
/// pending. Use when runtime code must inspect a terminal exception
/// (StopIteration/IndexError) but propagate every other exception unchanged.
///
/// `exc_bits` is borrowed by this helper; callers that obtained it from
/// `molt_exception_last()` still own that reference and must release it.
pub(crate) fn exception_stack_pop_restore_last(_py: &PyToken<'_>, exc_bits: u64) {
    exception_stack_pop(_py);
    if !obj_from_bits(exc_bits).is_none() && exc_bits != 0 {
        let _ = molt_exception_set_last(exc_bits);
    }
}

pub(crate) fn generator_raise_active() -> bool {
    GENERATOR_RAISE.with(|flag| flag.get())
}

pub(crate) fn set_generator_raise(active: bool) {
    GENERATOR_RAISE.with(|flag| flag.set(active));
}

pub(crate) fn task_raise_active() -> bool {
    TASK_RAISE_ACTIVE.with(|flag| flag.get())
}

pub(crate) fn set_task_raise_active(active: bool) {
    TASK_RAISE_ACTIVE.with(|flag| flag.set(active));
}

pub(crate) fn exception_stack_depth() -> usize {
    EXCEPTION_STACK.with(|stack| stack.borrow().len())
}

pub(crate) fn exception_stack_set_depth(_py: &PyToken<'_>, target: usize) {
    crate::gil_assert();
    EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        while stack.len() > target {
            stack.pop();
        }
        while stack.len() < target {
            stack.push(0);
        }
    });
    exception_context_align_depth(_py, target);
}

pub(crate) fn generator_exception_stack_take(ptr: *mut u8) -> Vec<u64> {
    GENERATOR_EXCEPTION_STACKS
        .with(|map| map.borrow_mut().remove(&(ptr as usize)).unwrap_or_default())
}

pub(crate) fn generator_exception_stack_store(ptr: *mut u8, stack: Vec<u64>) {
    GENERATOR_EXCEPTION_STACKS.with(|map| {
        map.borrow_mut().insert(ptr as usize, stack);
    });
}

pub(crate) fn generator_exception_stack_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    GENERATOR_EXCEPTION_STACKS.with(|map| {
        if let Some(stack) = map.borrow_mut().remove(&(ptr as usize)) {
            for bits in stack {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
        }
    });
}

pub(crate) fn task_exception_stack_take(_py: &PyToken<'_>, ptr: *mut u8) -> Vec<u64> {
    task_exception_stacks(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr))
        .unwrap_or_default()
}

pub(crate) fn task_exception_stack_store(_py: &PyToken<'_>, ptr: *mut u8, stack: Vec<u64>) {
    task_exception_stacks(_py)
        .lock()
        .unwrap()
        .insert(PtrSlot(ptr), stack);
}

pub(crate) fn task_exception_stack_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    let stack = task_exception_stacks(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr));
    if let Some(stack) = stack {
        for bits in stack {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
    }
}

pub(crate) fn task_exception_handler_stack_take(_py: &PyToken<'_>, ptr: *mut u8) -> Vec<usize> {
    task_exception_handler_stacks(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr))
        .unwrap_or_default()
}

pub(crate) fn task_exception_handler_stack_store(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    stack: Vec<usize>,
) {
    task_exception_handler_stacks(_py)
        .lock()
        .unwrap()
        .insert(PtrSlot(ptr), stack);
}

pub(crate) fn task_exception_handler_stack_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    task_exception_handler_stacks(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr));
}

pub(crate) fn task_exception_depth_take(_py: &PyToken<'_>, ptr: *mut u8) -> usize {
    task_exception_depths(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr))
        .unwrap_or(0)
}

pub(crate) fn task_exception_depth_store(_py: &PyToken<'_>, ptr: *mut u8, depth: usize) {
    task_exception_depths(_py)
        .lock()
        .unwrap()
        .insert(PtrSlot(ptr), depth);
}

pub(crate) fn task_exception_depth_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    task_exception_depths(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr));
}

pub(crate) fn task_exception_baseline_take(_py: &PyToken<'_>, ptr: *mut u8) -> usize {
    runtime_state(_py)
        .task_exception_baselines
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr))
        .unwrap_or_else(exception_stack_baseline_get)
}

pub(crate) fn task_exception_baseline_store(_py: &PyToken<'_>, ptr: *mut u8, baseline: usize) {
    runtime_state(_py)
        .task_exception_baselines
        .lock()
        .unwrap()
        .insert(PtrSlot(ptr), baseline);
}

pub(crate) fn task_exception_baseline_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    runtime_state(_py)
        .task_exception_baselines
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr));
}

pub(crate) fn task_last_exception_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    let state = runtime_state(_py);
    let old_ptr = {
        let mut guard = task_last_exceptions(_py).lock().unwrap();
        let old = guard.remove(&PtrSlot(ptr));
        if guard.is_empty() {
            state
                .task_last_exception_pending
                .store(false, AtomicOrdering::Relaxed);
        }
        old
    };
    if let Some(old_ptr) = old_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn task_last_exception_contains_valid(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    crate::gil_assert();
    if ptr.is_null() {
        return false;
    }
    task_last_exceptions(_py)
        .lock()
        .unwrap()
        .get(&PtrSlot(ptr))
        .copied()
        .is_some_and(exception_slot_is_valid)
}

pub(crate) fn record_exception(_py: &PyToken<'_>, ptr: *mut u8) {
    record_exception_with_caller_frame(_py, ptr, false);
}

fn record_exception_with_caller_frame(_py: &PyToken<'_>, ptr: *mut u8, include_caller_frame: bool) {
    crate::gil_assert();
    // Stash the frame's col_offset at exception-raise time for caret annotations.
    FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        if let Some(entry) = stack.last() {
            // Only stash if we have real col data — don't overwrite a
            // good stash from a prior recording of the same exception.
            if entry.col_offset >= 0 && entry.end_col_offset >= 0 {
                LAST_EXCEPTION_COL.with(|cell| {
                    *cell.borrow_mut() = (entry.col_offset, entry.end_col_offset);
                });
            }
        }
    });
    if debug_exception_flow() {
        let kind_bits = unsafe { exception_kind_bits(ptr) };
        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!("molt exc SET kind={} ptr=0x{:x}", kind, ptr as usize);
    }
    let state = runtime_state(_py);
    let task_key = current_task_key();
    let mut prior_ptr = None;
    let mut context_bits: Option<u64> = None;
    let mut context_bits_owned = false;
    let mut context_from_active = false;
    let mut same_ptr = false;
    let debug_rc = debug_exception_rc();
    if debug_rc {
        let rc = unsafe {
            let header = header_from_obj_ptr(ptr);
            (*header).ref_count.load(AtomicOrdering::Acquire)
        };
        eprintln!("molt exc rc start ptr=0x{:x} rc={}", ptr as usize, rc);
    }
    let mut suppress_trace = unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).flags & HEADER_FLAG_TRACEBACK_SUPPRESSED != 0
    };
    if !suppress_trace && traceback_suppressed() {
        let kind_bits = unsafe { exception_kind_bits(ptr) };
        if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("AttributeError") {
            suppress_trace = true;
            unsafe {
                let header = header_from_obj_ptr(ptr);
                (*header).flags |= HEADER_FLAG_TRACEBACK_SUPPRESSED;
            }
        }
    }
    if suppress_trace && profile_enabled(_py) {
        TRACEBACK_SUPPRESS_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
    }
    if let Some(task_key) = task_key {
        let mut guard = task_last_exceptions(_py).lock().unwrap();
        if let Some(old_ptr) = guard.remove(&task_key) {
            prior_ptr = Some(old_ptr.0);
        }
        if guard.is_empty() {
            state
                .task_last_exception_pending
                .store(false, AtomicOrdering::Relaxed);
        }
    } else if let Some(old_ptr) = global_last_exception_take(_py) {
        prior_ptr = Some(old_ptr.0);
    }
    if let Some(old_ptr) = prior_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr).bits();
        if debug_rc {
            let old_rc = unsafe {
                let header = header_from_obj_ptr(old_ptr);
                (*header).ref_count.load(AtomicOrdering::Acquire)
            };
            eprintln!(
                "molt exc rc prior ptr=0x{:x} rc={}",
                old_ptr as usize, old_rc
            );
        }
        if old_ptr == ptr {
            same_ptr = true;
        } else {
            context_bits = Some(old_bits);
            // Own the previous exception reference removed from last_exception/task slot.
            // If we attach it as __context__, ownership transfers there; otherwise we drop it.
            context_bits_owned = true;
        }
    }
    if context_bits.is_none() {
        context_bits = exception_context_active_bits();
        context_from_active = context_bits.is_some();
    }
    if debug_rc {
        if let Some(ctx_bits) = context_bits {
            let ctx_obj = obj_from_bits(ctx_bits);
            let ctx_ptr = ctx_obj.as_ptr().map(|p| p as usize).unwrap_or(0);
            let ctx_ty = if let Some(ptr) = ctx_obj.as_ptr() {
                unsafe { object_type_id(ptr) }
            } else {
                0
            };
            eprintln!(
                "molt exc rc context bits=0x{:x} ptr=0x{:x} type_id={} owned={} from_active={}",
                ctx_bits, ctx_ptr, ctx_ty, context_bits_owned, context_from_active
            );
        } else {
            eprintln!("molt exc rc context none");
        }
    }
    if let Some(ctx_bits) = context_bits {
        let new_bits = MoltObject::from_ptr(ptr).bits();
        if ctx_bits != new_bits {
            let existing = unsafe { exception_context_bits(ptr) };
            if obj_from_bits(existing).is_none() {
                unsafe {
                    // Active-exception stack values are borrowed; prior last_exception values
                    // already carry owned storage ref that we transfer into __context__.
                    if !context_bits_owned {
                        inc_ref_bits(_py, ctx_bits);
                    }
                    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = ctx_bits;
                }
            } else if context_bits_owned {
                dec_ref_bits(_py, ctx_bits);
            }
        } else if context_bits_owned {
            dec_ref_bits(_py, ctx_bits);
        }
    }
    let trace_bits = unsafe { exception_trace_bits(ptr) };
    if suppress_trace {
        if !obj_from_bits(trace_bits).is_none() {
            dec_ref_bits(_py, trace_bits);
            unsafe {
                *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
            }
        }
    } else if !obj_from_bits(trace_bits).is_none() {
        // Preserve an existing traceback instead of rebuilding on re-raise.
    } else {
        let handler_frame_index = EXCEPTION_STACK.with(|stack| stack.borrow().last().copied());
        // CPython keeps the active traceback chain rooted at the raising frame even for
        // explicit `raise ... from ...`; the cause carries its own traceback separately.
        if let Some(new_bits) =
            frame_stack_trace_payload_bits(_py, handler_frame_index, include_caller_frame)
        {
            if new_bits != trace_bits {
                if !obj_from_bits(trace_bits).is_none() {
                    dec_ref_bits(_py, trace_bits);
                }
                unsafe {
                    *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = new_bits;
                }
            } else {
                dec_ref_bits(_py, new_bits);
            }
        } else if !obj_from_bits(trace_bits).is_none() {
            dec_ref_bits(_py, trace_bits);
            unsafe {
                *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
            }
        }
    }
    if let Some(task_key) = task_key {
        // Inc-ref for task exception slot (same rationale as global slot).
        let bits = MoltObject::from_ptr(ptr).bits();
        inc_ref_bits(_py, bits);
        task_last_exceptions(_py)
            .lock()
            .unwrap()
            .insert(task_key, PtrSlot(ptr));
        state
            .task_last_exception_pending
            .store(true, AtomicOrdering::Relaxed);
    } else {
        // The global slot owns one strong reference. Re-recording the same
        // pointer reuses the reference removed from the slot above; new
        // pointers acquire a fresh slot reference.
        global_last_exception_store_recorded(_py, ptr, same_ptr);
    }
    if debug_exceptions() {
        let debug_pending = debug_exception_pending();
        let kind_bits = unsafe { exception_kind_bits(ptr) };
        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
            .unwrap_or_else(|| "<unknown>".to_string());
        let msg = {
            let args_bits = unsafe { exception_args_bits(ptr) };
            let args_obj = obj_from_bits(args_bits);
            let mut out = String::new();
            if let Some(args_ptr) = args_obj.as_ptr() {
                unsafe {
                    if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                        let elems = seq_vec_ref(args_ptr);
                        if let Some(&first) = elems.first() {
                            out = format_obj_str(_py, obj_from_bits(first));
                        }
                    }
                }
            }
            out
        };
        let task = task_key.map(|slot| slot.0 as usize).unwrap_or(0);
        if debug_pending
            && task == 0
            && kind == "StopAsyncIteration"
            && !STOPASYNC_BT_PRINTED.swap(true, AtomicOrdering::Relaxed)
        {
            eprintln!("molt exc backtrace (StopAsyncIteration, no task):");
            eprintln!("{:?}", Backtrace::force_capture());
        }
        if msg.is_empty() {
            eprintln!("molt exc record task=0x{:x} kind={}", task, kind);
        } else {
            eprintln!(
                "molt exc record task=0x{:x} kind={} msg={}",
                task, kind, msg
            );
        }
    }
    if debug_rc {
        let rc = unsafe {
            let header = header_from_obj_ptr(ptr);
            (*header).ref_count.load(AtomicOrdering::Acquire)
        };
        eprintln!(
            "molt exc rc end ptr=0x{:x} rc={} same_ptr={} ctx_owned={}",
            ptr as usize, rc, same_ptr, context_bits_owned
        );
    }
}

pub(crate) fn record_exception_owned(_py: &PyToken<'_>, ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    record_exception(_py, ptr);
    dec_ref_bits(_py, bits);
}

pub(crate) fn clear_exception(_py: &PyToken<'_>) {
    crate::gil_assert();
    let state = runtime_state(_py);
    if let Some(task_key) = current_task_key() {
        let old_ptr = {
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            let old = guard.remove(&task_key);
            if guard.is_empty() {
                state
                    .task_last_exception_pending
                    .store(false, AtomicOrdering::Relaxed);
            }
            old
        };
        if let Some(old_ptr) = old_ptr {
            let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
            dec_ref_bits(_py, old_bits);
        }
        return;
    }
    let old_ptr = global_last_exception_take(_py);
    if let Some(old_ptr) = old_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn exception_set_last_bits_raw(_py: &PyToken<'_>, exc_bits: u64) {
    crate::gil_assert();
    let Some(ptr) = obj_from_bits(exc_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            return;
        }
    }
    let state = runtime_state(_py);
    if let Some(task_key) = current_task_key() {
        let old_ptr = {
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            guard.insert(task_key, PtrSlot(ptr))
        };
        if let Some(old_ptr) = old_ptr {
            if old_ptr.0 != ptr {
                let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, exc_bits);
            }
        } else {
            inc_ref_bits(_py, exc_bits);
        }
        state
            .task_last_exception_pending
            .store(true, AtomicOrdering::Relaxed);
    } else {
        global_last_exception_replace_borrowed(_py, ptr, exc_bits);
    }
}

enum ExceptionBaseSpec {
    One(&'static str),
    Two(&'static str, &'static str),
}

fn exception_alias_name(name: &str) -> Option<&'static str> {
    match name {
        "EnvironmentError" | "IOError" | "WindowsError" => Some("OSError"),
        _ => None,
    }
}

fn exception_base_spec(name: &str) -> Option<ExceptionBaseSpec> {
    match name {
        "BaseExceptionGroup" => Some(ExceptionBaseSpec::One("BaseException")),
        "ExceptionGroup" => Some(ExceptionBaseSpec::Two("BaseExceptionGroup", "Exception")),
        "GeneratorExit" | "KeyboardInterrupt" | "SystemExit" | "CancelledError" => {
            Some(ExceptionBaseSpec::One("BaseException"))
        }
        "ArithmeticError" | "AssertionError" | "AttributeError" | "BufferError" | "EOFError"
        | "ImportError" | "LookupError" | "MemoryError" | "NameError" | "OSError"
        | "ReferenceError" | "RuntimeError" | "StopIteration" | "StopAsyncIteration"
        | "SyntaxError" | "SystemError" | "TypeError" | "ValueError" | "Warning" => {
            Some(ExceptionBaseSpec::One("Exception"))
        }
        "FloatingPointError" | "OverflowError" | "ZeroDivisionError" => {
            Some(ExceptionBaseSpec::One("ArithmeticError"))
        }
        "ModuleNotFoundError" => Some(ExceptionBaseSpec::One("ImportError")),
        "IndexError" | "KeyError" => Some(ExceptionBaseSpec::One("LookupError")),
        "UnboundLocalError" => Some(ExceptionBaseSpec::One("NameError")),
        "ConnectionError" => Some(ExceptionBaseSpec::One("OSError")),
        "BrokenPipeError"
        | "ConnectionAbortedError"
        | "ConnectionRefusedError"
        | "ConnectionResetError" => Some(ExceptionBaseSpec::One("ConnectionError")),
        "BlockingIOError" | "ChildProcessError" | "FileExistsError" | "FileNotFoundError"
        | "InterruptedError" | "IsADirectoryError" | "NotADirectoryError" | "PermissionError"
        | "ProcessLookupError" | "TimeoutError" => Some(ExceptionBaseSpec::One("OSError")),
        "UnsupportedOperation" => Some(ExceptionBaseSpec::Two("OSError", "ValueError")),
        "NotImplementedError" | "PythonFinalizationError" | "RecursionError" => {
            Some(ExceptionBaseSpec::One("RuntimeError"))
        }
        "IndentationError" => Some(ExceptionBaseSpec::One("SyntaxError")),
        "TabError" => Some(ExceptionBaseSpec::One("IndentationError")),
        "UnicodeError" => Some(ExceptionBaseSpec::One("ValueError")),
        "UnicodeDecodeError" | "UnicodeEncodeError" | "UnicodeTranslateError" => {
            Some(ExceptionBaseSpec::One("UnicodeError"))
        }
        "DeprecationWarning"
        | "PendingDeprecationWarning"
        | "RuntimeWarning"
        | "SyntaxWarning"
        | "UserWarning"
        | "FutureWarning"
        | "ImportWarning"
        | "UnicodeWarning"
        | "BytesWarning"
        | "ResourceWarning"
        | "EncodingWarning" => Some(ExceptionBaseSpec::One("Warning")),
        _ => None,
    }
}

fn exception_type_bits_from_builtins(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    let module_bits = {
        let cache = module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.get("builtins").copied()
    }?;
    let module_ptr = obj_from_bits(module_bits).as_ptr()?;
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return None;
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return None;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_bits = dict_get_in_place_fast_str(_py, dict_ptr, name_bits);
        dec_ref_bits(_py, name_bits);
        let value_bits = value_bits?;
        let value_ptr = obj_from_bits(value_bits).as_ptr()?;
        if object_type_id(value_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let builtins = builtin_classes(_py);
        if !issubclass_bits(value_bits, builtins.base_exception) {
            return None;
        }
        Some(value_bits)
    }
}

unsafe fn dict_get_in_place_fast_str(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    unsafe {
        let order = dict_order(dict_ptr);
        let hashes = dict_hashes(dict_ptr);
        let table = dict_table(dict_ptr);
        let found = dict_find_entry_fast(_py, order, hashes, table, key_bits);
        found.map(|idx| order[idx * 2 + 1])
    }
}

pub(crate) fn exception_type_bits_from_name(_py: &PyToken<'_>, name: &str) -> u64 {
    let builtins = builtin_classes(_py);
    match name {
        "Exception" => {
            let bits = builtins.exception;
            ensure_exception_in_builtins(_py, name, bits);
            return bits;
        }
        "BaseException" => {
            let bits = builtins.base_exception;
            ensure_exception_in_builtins(_py, name, bits);
            return bits;
        }
        "BaseExceptionGroup" => {
            let bits = builtins.base_exception_group;
            ensure_exception_in_builtins(_py, name, bits);
            return bits;
        }
        "ExceptionGroup" => {
            let bits = builtins.exception_group;
            ensure_exception_in_builtins(_py, name, bits);
            return bits;
        }
        _ => {}
    }
    if let Some(bits) = exception_type_cache(_py).lock().unwrap().get(name).copied() {
        return bits;
    }
    if let Some(bits) = exception_type_bits_from_builtins(_py, name) {
        let mut cache = exception_type_cache(_py).lock().unwrap();
        if let Some(existing) = cache.get(name).copied() {
            return existing;
        }
        inc_ref_bits(_py, bits);
        cache.insert(name.to_string(), bits);
        return bits;
    }
    if let Some(alias) = exception_alias_name(name) {
        let bits = exception_type_bits_from_name(_py, alias);
        if bits != 0 {
            exception_type_cache(_py)
                .lock()
                .unwrap()
                .insert(name.to_string(), bits);
            ensure_exception_in_builtins(_py, name, bits);
        }
        return bits;
    }
    let fallback = builtins.exception;
    let base_spec = exception_base_spec(name);
    let base_bits = match base_spec {
        Some(ExceptionBaseSpec::One(base)) => exception_type_bits_from_name(_py, base),
        Some(ExceptionBaseSpec::Two(left, right)) => {
            let left_bits = exception_type_bits_from_name(_py, left);
            let right_bits = exception_type_bits_from_name(_py, right);
            let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
            if tuple_ptr.is_null() {
                fallback
            } else {
                let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                let class_ptr = alloc_class_obj_from_name(_py, name);
                if class_ptr.is_null() {
                    dec_ref_bits(_py, tuple_bits);
                    return fallback;
                }
                let class_bits = MoltObject::from_ptr(class_ptr).bits();
                let _ = molt_class_set_base(class_bits, tuple_bits);
                set_exception_text_signature_none(_py, class_bits);
                dec_ref_bits(_py, tuple_bits);
                return cache_exception_type(_py, name, class_bits);
            }
        }
        None => fallback,
    };
    let class_ptr = alloc_class_obj_from_name(_py, name);
    if class_ptr.is_null() {
        return fallback;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let _ = molt_class_set_base(class_bits, base_bits);
    set_exception_text_signature_none(_py, class_bits);
    cache_exception_type(_py, name, class_bits)
}

fn alloc_class_obj_from_name(_py: &PyToken<'_>, name: &str) -> *mut u8 {
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    if !class_ptr.is_null() {
        // Ensure the class object is an instance of `type` (CPython parity).
        // Without this, `type(cls)` falls back to `builtins.type_obj` in
        // `type_of_bits`, but `issubclass` checks that compare metaclass
        // identity may fail because the stored class-bits are 0 instead of
        // the canonical `type` object.
        unsafe {
            let builtins = builtin_classes(_py);
            let old = object_class_bits(class_ptr);
            if old != builtins.type_obj {
                if old != 0 {
                    dec_ref_bits(_py, old);
                }
                crate::object_set_class_bits(_py, class_ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
    }
    class_ptr
}

fn set_exception_text_signature_none(_py: &PyToken<'_>, class_bits: u64) {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return;
        }
    }
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__text_signature__") else {
        return;
    };
    unsafe {
        dict_set_in_place(_py, dict_ptr, name_bits, MoltObject::none().bits());
    }
    dec_ref_bits(_py, name_bits);
}

fn cache_exception_type(_py: &PyToken<'_>, name: &str, class_bits: u64) -> u64 {
    let mut cache = exception_type_cache(_py).lock().unwrap();
    if let Some(bits) = cache.get(name).copied() {
        dec_ref_bits(_py, class_bits);
        return bits;
    }
    inc_ref_bits(_py, class_bits);
    cache.insert(name.to_string(), class_bits);
    ensure_exception_in_builtins(_py, name, class_bits);
    class_bits
}

fn ensure_exception_in_builtins(_py: &PyToken<'_>, name: &str, class_bits: u64) {
    let module_bits = {
        let cache = module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.get("builtins").copied()
    };
    let Some(module_bits) = module_bits else {
        return;
    };
    let module_ptr = match obj_from_bits(module_bits).as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_MODULE => ptr,
        _ => return,
    };
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_DICT => ptr,
        _ => return,
    };
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let existing = unsafe { dict_get_in_place_fast_str(_py, dict_ptr, name_bits) };
    let needs_set = existing != Some(class_bits);
    if needs_set {
        unsafe {
            dict_set_in_place(_py, dict_ptr, name_bits, class_bits);
        }
    }
    dec_ref_bits(_py, name_bits);
}

pub(crate) fn exception_type_bits(_py: &PyToken<'_>, kind_bits: u64) -> u64 {
    let name =
        string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "Exception".to_string());
    exception_type_bits_from_name(_py, &name)
}

fn builtin_exception_name_for_tag(tag: u64) -> Option<&'static str> {
    match tag {
        1 => Some("BaseException"),
        2 => Some("Exception"),
        3 => Some("KeyError"),
        4 => Some("IndexError"),
        5 => Some("ValueError"),
        6 => Some("TypeError"),
        7 => Some("RuntimeError"),
        8 => Some("StopIteration"),
        9 => Some("StopAsyncIteration"),
        10 => Some("AssertionError"),
        11 => Some("ImportError"),
        12 => Some("NameError"),
        13 => Some("UnboundLocalError"),
        14 => Some("NotImplementedError"),
        _ => None,
    }
}

fn builtin_exception_class_cache_for_tag(
    _py: &PyToken<'_>,
    tag: u64,
) -> Option<(&'static AtomicU64, &'static str)> {
    let state = exceptions_state(_py);
    match tag {
        1 => Some((&state.base_exception_class_cache, "BaseException")),
        2 => Some((&state.exception_class_cache, "Exception")),
        3 => Some((&state.key_error_class_cache, "KeyError")),
        4 => Some((&state.index_error_class_cache, "IndexError")),
        5 => Some((&state.value_error_class_cache, "ValueError")),
        6 => Some((&state.type_error_class_cache, "TypeError")),
        7 => Some((&state.runtime_error_class_cache, "RuntimeError")),
        8 => Some((&state.stop_iteration_class_cache, "StopIteration")),
        9 => Some((
            &state.stop_async_iteration_class_cache,
            "StopAsyncIteration",
        )),
        10 => Some((&state.assertion_error_class_cache, "AssertionError")),
        11 => Some((&state.import_error_class_cache, "ImportError")),
        12 => Some((&state.name_error_class_cache, "NameError")),
        13 => Some((&state.unbound_local_error_class_cache, "UnboundLocalError")),
        14 => Some((
            &state.not_implemented_error_class_cache,
            "NotImplementedError",
        )),
        _ => None,
    }
}

fn builtin_exception_class_bits_for_tag(_py: &PyToken<'_>, tag: u64) -> Option<u64> {
    let (cache, name) = builtin_exception_class_cache_for_tag(_py, tag)?;
    let cached = cache.load(AtomicOrdering::Acquire);
    if cached != 0 {
        return Some(cached);
    }
    let class_bits = exception_type_bits_from_name(_py, name);
    if class_bits != 0 {
        Some(init_atomic_bits(_py, cache, || {
            inc_ref_bits(_py, class_bits);
            class_bits
        }))
    } else {
        None
    }
}

fn exception_message_for_builtin_tag_storage(
    _py: &PyToken<'_>,
    tag: u64,
    class_bits: u64,
    args_bits: u64,
) -> u64 {
    if tag != 3
        && unsafe {
            crate::object::ops_format::exception_class_bits_uses_cached_message_str(_py, class_bits)
        }
    {
        exception_lazy_message_bits()
    } else {
        exception_message_from_args(_py, args_bits)
    }
}

fn exception_message_for_builtin_tag_single_storage(
    _py: &PyToken<'_>,
    tag: u64,
    class_bits: u64,
    arg_bits: u64,
) -> u64 {
    if tag != 3
        && unsafe {
            crate::object::ops_format::exception_class_bits_uses_cached_message_str(_py, class_bits)
        }
    {
        exception_lazy_message_bits()
    } else {
        molt_str_from_obj(arg_bits)
    }
}

fn alloc_builtin_exception_from_tag(_py: &PyToken<'_>, tag: u64, args_bits: u64) -> *mut u8 {
    let Some(class_bits) = builtin_exception_class_bits_for_tag(_py, tag) else {
        return std::ptr::null_mut();
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    let kind_bits = unsafe { class_name_bits(class_ptr) };
    let msg_bits = exception_message_for_builtin_tag_storage(_py, tag, class_bits, args_bits);
    if obj_from_bits(msg_bits).is_none() {
        return std::ptr::null_mut();
    }
    let none_bits = MoltObject::none().bits();
    let ptr = alloc_exception_obj(_py, kind_bits, msg_bits, class_bits, args_bits, none_bits);
    if tag == 8 && !ptr.is_null() {
        unsafe {
            exception_set_stop_iteration_value(_py, ptr, args_bits);
        }
    }
    dec_ref_bits(_py, msg_bits);
    ptr
}

fn alloc_builtin_exception_from_tag_single(_py: &PyToken<'_>, tag: u64, arg_bits: u64) -> *mut u8 {
    let Some(class_bits) = builtin_exception_class_bits_for_tag(_py, tag) else {
        return std::ptr::null_mut();
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    let kind_bits = unsafe { class_name_bits(class_ptr) };
    let msg_bits = exception_message_for_builtin_tag_single_storage(_py, tag, class_bits, arg_bits);
    if obj_from_bits(msg_bits).is_none() {
        return std::ptr::null_mut();
    }
    let none_bits = MoltObject::none().bits();
    let ptr = alloc_exception_obj_with_args_payload(
        _py,
        kind_bits,
        msg_bits,
        class_bits,
        exception_lazy_single_args_bits(),
        none_bits,
        arg_bits,
    );
    if tag == 8 && !ptr.is_null() {
        unsafe {
            exception_set_stop_iteration_value(_py, ptr, exception_lazy_single_args_bits());
        }
    }
    dec_ref_bits(_py, msg_bits);
    ptr
}

pub(crate) fn exception_normalize_args(_py: &PyToken<'_>, args_bits: u64) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    if args_obj.is_none() || args_bits == 0 {
        let ptr = alloc_tuple(_py, &[]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    if let Some(ptr) = args_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE {
                inc_ref_bits(_py, args_bits);
                return args_bits;
            }
            if type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let out_ptr = alloc_tuple(_py, elems);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
    }
    let ptr = alloc_tuple(_py, &[args_bits]);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

pub(crate) fn exception_message_from_args(_py: &PyToken<'_>, args_bits: u64) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    if let Some(ptr) = args_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                match elems.len() {
                    0 => {
                        let ptr = alloc_string(_py, b"");
                        if ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(ptr).bits();
                    }
                    1 => return molt_str_from_obj(elems[0]),
                    _ => return molt_str_from_obj(args_bits),
                }
            }
        }
    }
    molt_str_from_obj(args_bits)
}

fn exception_message_from_exception_args(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let args_bits = unsafe { exception_args_bits(ptr) };
    if exception_args_is_lazy_single(args_bits) {
        let value_bits = unsafe { exception_args_payload_bits(ptr) };
        let kind_bits = unsafe { exception_kind_bits(ptr) };
        if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("KeyError") {
            return molt_repr_from_obj(value_bits);
        }
        return molt_str_from_obj(value_bits);
    }
    exception_message_from_args(_py, args_bits)
}

pub(crate) fn exception_args_from_iterable(_py: &PyToken<'_>, bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE {
                inc_ref_bits(_py, bits);
                return bits;
            }
            if type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let out_ptr = alloc_tuple(_py, elems);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
    }
    let iter_bits = molt_iter_checked(bits);
    if obj_from_bits(iter_bits).is_none() {
        return MoltObject::none().bits();
    }
    let mut elems: Vec<u64> = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let pair = seq_vec_ref(pair_ptr);
            if pair.len() < 2 {
                return MoltObject::none().bits();
            }
            if is_truthy(_py, obj_from_bits(pair[1])) {
                break;
            }
            elems.push(pair[0]);
        }
    }
    let out_ptr = alloc_tuple(_py, &elems);
    if out_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(out_ptr).bits()
    }
}

pub(crate) unsafe fn exception_store_args_and_message(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
    msg_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        let args_slot = ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
        let old_args = *args_slot;
        if old_args != args_bits {
            dec_ref_bits(_py, old_args);
            *args_slot = args_bits;
            if exception_args_is_lazy_single(old_args) {
                exception_set_args_payload_slot(_py, ptr, MoltObject::none().bits());
            }
        }
        let msg_slot = ptr.add(std::mem::size_of::<u64>()) as *mut u64;
        let old_msg = *msg_slot;
        if old_msg != msg_bits {
            dec_ref_bits(_py, old_msg);
            *msg_slot = msg_bits;
        }
    }
}

unsafe fn exception_set_value_slot(_py: &PyToken<'_>, ptr: *mut u8, value_bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != value_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, value_bits);
            *slot = value_bits;
        }
    }
}

unsafe fn exception_set_args_payload_slot(_py: &PyToken<'_>, ptr: *mut u8, value_bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != value_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, value_bits);
            *slot = value_bits;
        }
    }
}

pub(crate) unsafe fn exception_set_stop_iteration_value(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr))).unwrap_or_default();
        if kind != "StopIteration" {
            return;
        }
        let mut value_bits = MoltObject::none().bits();
        if exception_args_is_lazy_single(args_bits) {
            value_bits = exception_args_payload_bits(ptr);
        } else {
            let args_obj = obj_from_bits(args_bits);
            if let Some(args_ptr) = args_obj.as_ptr() {
                let type_id = object_type_id(args_ptr);
                if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                    let elems = seq_vec_ref(args_ptr);
                    if let Some(first) = elems.first() {
                        value_bits = *first;
                    }
                } else if !args_obj.is_none() {
                    value_bits = args_bits;
                }
            } else if !args_obj.is_none() {
                value_bits = args_bits;
            }
        }
        exception_set_value_slot(_py, ptr, value_bits);
    }
}

pub(crate) unsafe fn exception_set_system_exit_code(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr))).unwrap_or_default();
        if kind != "SystemExit" {
            return;
        }
        let mut code_bits = MoltObject::none().bits();
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            let type_id = object_type_id(args_ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(args_ptr);
                if elems.len() == 1 {
                    code_bits = elems[0];
                } else if elems.len() > 1 {
                    code_bits = args_bits;
                }
            }
        } else if !args_obj.is_none() {
            code_bits = args_bits;
        }
        let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != code_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, code_bits);
            *slot = code_bits;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new(kind_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let kind_obj = obj_from_bits(kind_bits);
        if let Some(ptr) = kind_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "exception kind must be a str",
                    );
                }
            }
        } else {
            return raise_exception::<u64>(_py, "TypeError", "exception kind must be a str");
        }
        let args_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(args_bits).is_none() {
            return MoltObject::none().bits();
        }
        let class_bits = exception_type_bits(_py, kind_bits);
        let msg_bits = exception_message_for_storage(_py, kind_bits, class_bits, args_bits);
        if obj_from_bits(msg_bits).is_none() {
            dec_ref_bits(_py, args_bits);
            return MoltObject::none().bits();
        }
        let none_bits = MoltObject::none().bits();
        let ptr = alloc_exception_obj(_py, kind_bits, msg_bits, class_bits, args_bits, none_bits);
        let out = if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                exception_set_stop_iteration_value(_py, ptr, args_bits);
                exception_set_system_exit_code(_py, ptr, args_bits);
            }
            MoltObject::from_ptr(ptr).bits()
        };
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, msg_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new_builtin(tag: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if builtin_exception_name_for_tag(tag).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "unknown builtin exception tag");
        }
        let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "builtin exception constructor expects tuple args",
            );
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "builtin exception constructor expects tuple args",
                );
            }
        }
        let ptr = alloc_builtin_exception_from_tag(_py, tag, args_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new_builtin_empty(tag: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if builtin_exception_name_for_tag(tag).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "unknown builtin exception tag");
        }
        let args_ptr = alloc_tuple(_py, &[]);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let ptr = alloc_builtin_exception_from_tag(_py, tag, args_bits);
        dec_ref_bits(_py, args_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new_builtin_one(tag: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if builtin_exception_name_for_tag(tag).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "unknown builtin exception tag");
        }
        let ptr = alloc_builtin_exception_from_tag_single(_py, tag, arg_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_match_builtin(exc_bits: u64, tag: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(target_class_bits) = builtin_exception_class_bits_for_tag(_py, tag) else {
            return raise_exception::<u64>(_py, "RuntimeError", "unknown builtin exception tag");
        };
        let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
                return MoltObject::from_bool(false).bits();
            }
            let class_bits = exception_class_bits(exc_ptr);
            if class_bits == target_class_bits {
                return MoltObject::from_bool(true).bits();
            }
            MoltObject::from_bool(issubclass_bits(class_bits, target_class_bits)).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new_from_class(class_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "exception class must be a type");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<u64>(_py, "TypeError", "exception class must be a type");
            }
        }
        let builtins = builtin_classes(_py);
        let is_sub = issubclass_bits(class_bits, builtins.base_exception);
        if !is_sub {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        }
        let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_new_bound(class_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let out = molt_exception_new_from_class(class_bits, args_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_init(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exception init expects exception instance",
            );
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception init expects exception instance",
                );
            }
        }
        let norm_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(norm_bits).is_none() {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return MoltObject::none().bits();
        }
        let kind_bits = unsafe { exception_kind_bits(self_ptr) };
        let class_bits = unsafe { exception_class_bits(self_ptr) };
        let msg_bits = exception_message_for_storage(_py, kind_bits, class_bits, norm_bits);
        if obj_from_bits(msg_bits).is_none() {
            dec_ref_bits(_py, norm_bits);
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return MoltObject::none().bits();
        }
        let existing_bits = unsafe { exception_args_bits(self_ptr) };
        let existing_len = if exception_args_is_lazy_single(existing_bits) {
            1
        } else if let Some(ptr) = obj_from_bits(existing_bits).as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                    seq_vec_ref(ptr).len()
                } else {
                    0
                }
            }
        } else {
            0
        };
        let new_len = if let Some(ptr) = obj_from_bits(norm_bits).as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    seq_vec_ref(ptr).len()
                } else {
                    0
                }
            }
        } else {
            0
        };
        let preserve_existing = existing_len > 0 && new_len > existing_len;
        if !preserve_existing {
            let mut class_bits = unsafe { exception_class_bits(self_ptr) };
            if obj_from_bits(class_bits).is_none() || class_bits == 0 {
                class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
            }
            let mut unicode_fields = None;
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            {
                unsafe {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE
                        && let Some(name) =
                            string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
                        && let Some(kind) = unicode_error_kind(&name)
                    {
                        let fields = match unicode_error_fields_from_args(_py, kind, norm_bits) {
                            Ok(fields) => fields,
                            Err(()) => {
                                dec_ref_bits(_py, norm_bits);
                                dec_ref_bits(_py, msg_bits);
                                if !obj_from_bits(args_bits).is_none() {
                                    dec_ref_bits(_py, args_bits);
                                }
                                return MoltObject::none().bits();
                            }
                        };
                        unicode_fields = Some(fields);
                    }
                }
            }
            unsafe {
                inc_ref_bits(_py, norm_bits);
                inc_ref_bits(_py, msg_bits);
                exception_store_args_and_message(_py, self_ptr, norm_bits, msg_bits);
                exception_set_stop_iteration_value(_py, self_ptr, norm_bits);
                exception_set_system_exit_code(_py, self_ptr, norm_bits);
            }
            let oserror_bits = exception_type_bits_from_name(_py, "OSError");
            if class_bits != 0 && oserror_bits != 0 && issubclass_bits(class_bits, oserror_bits) {
                let (errno_val, strerror_bits, filename_bits) = unsafe { oserror_args(norm_bits) };
                let mut dict_bits = unsafe { exception_dict_bits(self_ptr) };
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        unsafe {
                            let slot = self_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                            let old_bits = *slot;
                            if old_bits != dict_bits {
                                dec_ref_bits(_py, old_bits);
                                *slot = dict_bits;
                            }
                        }
                    }
                }
                if !obj_from_bits(dict_bits).is_none()
                    && dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
                {
                    let errno_name =
                        intern_static_name(_py, &exceptions_state(_py).errno_attr_name, b"errno");
                    let strerror_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).strerror_attr_name,
                        b"strerror",
                    );
                    let filename_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).filename_attr_name,
                        b"filename",
                    );
                    let errno_bits = match errno_val {
                        Some(val) => MoltObject::from_int(val).bits(),
                        None => MoltObject::none().bits(),
                    };
                    unsafe {
                        dict_set_in_place(_py, dict_ptr, errno_name, errno_bits);
                        dict_set_in_place(_py, dict_ptr, strerror_name, strerror_bits);
                        dict_set_in_place(_py, dict_ptr, filename_name, filename_bits);
                    }
                }
            }
            if let Some(fields) = unicode_fields {
                let mut dict_bits = unsafe { exception_dict_bits(self_ptr) };
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        unsafe {
                            let slot = self_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                            let old_bits = *slot;
                            if old_bits != dict_bits {
                                dec_ref_bits(_py, old_bits);
                                *slot = dict_bits;
                            }
                        }
                    }
                }
                if !obj_from_bits(dict_bits).is_none()
                    && dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
                {
                    let encoding_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).unicode_encoding_attr_name,
                        b"encoding",
                    );
                    let object_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).unicode_object_attr_name,
                        b"object",
                    );
                    let start_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).unicode_start_attr_name,
                        b"start",
                    );
                    let end_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).unicode_end_attr_name,
                        b"end",
                    );
                    let reason_name = intern_static_name(
                        _py,
                        &exceptions_state(_py).unicode_reason_attr_name,
                        b"reason",
                    );
                    unsafe {
                        dict_set_in_place(_py, dict_ptr, encoding_name, fields.encoding_bits);
                        dict_set_in_place(_py, dict_ptr, object_name, fields.object_bits);
                        dict_set_in_place(_py, dict_ptr, start_name, fields.start_bits);
                        dict_set_in_place(_py, dict_ptr, end_name, fields.end_bits);
                        dict_set_in_place(_py, dict_ptr, reason_name, fields.reason_bits);
                    }
                }
            }
        }
        dec_ref_bits(_py, norm_bits);
        dec_ref_bits(_py, msg_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_add_note(self_bits: u64, note_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "add_note expects exception instance");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "add_note expects exception instance",
                );
            }
        }
        let note_obj = obj_from_bits(note_bits);
        let Some(note_ptr) = note_obj.as_ptr() else {
            let note_type = type_name(_py, note_obj);
            let msg = format!("add_note() argument must be str, not {note_type}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(note_ptr) != TYPE_ID_STRING {
                let note_type = type_name(_py, note_obj);
                let msg = format!("add_note() argument must be str, not {note_type}");
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
        }
        let mut dict_bits = unsafe { exception_dict_bits(self_ptr) };
        if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let new_bits = MoltObject::from_ptr(dict_ptr).bits();
            unsafe {
                let slot = self_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != new_bits {
                    dec_ref_bits(_py, old_bits);
                    *slot = new_bits;
                }
            }
            dict_bits = new_bits;
        }
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<u64>(_py, "TypeError", "exception dict missing");
            }
        }
        let notes_name =
            intern_static_name(_py, &runtime_state(_py).interned.notes_name, b"__notes__");
        if let Some(list_bits) = unsafe { dict_get_in_place(_py, dict_ptr, notes_name) } {
            let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Cannot add note: __notes__ is not a list",
                );
            };
            unsafe {
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "Cannot add note: __notes__ is not a list",
                    );
                }
                let elems = seq_vec(list_ptr);
                elems.push(note_bits);
            }
            inc_ref_bits(_py, note_bits);
            return MoltObject::none().bits();
        }
        let list_ptr = alloc_list(_py, &[note_bits]);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        unsafe {
            dict_set_in_place(_py, dict_ptr, notes_name, list_bits);
        }
        dec_ref_bits(_py, list_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_with_traceback(self_bits: u64, traceback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "with_traceback expects exception instance",
            );
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "with_traceback expects exception instance",
                );
            }
        }
        let traceback_obj = obj_from_bits(traceback_bits);
        if !traceback_obj.is_none() {
            let Some(_traceback_ptr) = traceback_obj.as_ptr() else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__traceback__ must be a traceback or None",
                );
            };
            let traceback_type = builtin_classes(_py).traceback;
            if traceback_type == 0 || !isinstance_bits(_py, traceback_bits, traceback_type) {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__traceback__ must be a traceback or None",
                );
            }
        }
        unsafe {
            let slot = self_ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *slot;
            if old_bits != traceback_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, traceback_bits);
                *slot = traceback_bits;
            }
        }
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_kind(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
            let bits = exception_kind_bits(ptr);
            inc_ref_bits(_py, bits);
            bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_class(kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let kind_obj = obj_from_bits(kind_bits);
        let Some(ptr) = kind_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "exception kind must be a str");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                return raise_exception::<u64>(_py, "TypeError", "exception kind must be a str");
            }
        }
        let class_bits = exception_type_bits(_py, kind_bits);
        inc_ref_bits(_py, class_bits);
        class_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_message(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
            let bits = exception_materialized_message_bits(_py, ptr);
            inc_ref_bits(_py, bits);
            bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_set_cause(exc_bits: u64, cause_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let cause_obj = obj_from_bits(cause_bits);
        if !cause_obj.is_none() {
            let Some(cause_ptr) = cause_obj.as_ptr() else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception cause must be an exception or None",
                );
            };
            unsafe {
                if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "exception cause must be an exception or None",
                    );
                }
            }
        }
        unsafe {
            let old_bits = exception_cause_bits(ptr);
            if old_bits != cause_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, cause_bits);
                *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = cause_bits;
            }
            let suppress_bits = MoltObject::from_bool(true).bits();
            let old_suppress = exception_suppress_bits(ptr);
            if old_suppress != suppress_bits {
                dec_ref_bits(_py, old_suppress);
                inc_ref_bits(_py, suppress_bits);
                *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = suppress_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_set_value(exc_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
            let old_bits = exception_value_bits(ptr);
            if old_bits != value_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, value_bits);
                *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = value_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_context_set(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        if !exc_obj.is_none() {
            let Some(ptr) = exc_obj.as_ptr() else {
                exception_context_set(_py, MoltObject::none().bits());
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                    exception_context_set(_py, MoltObject::none().bits());
                    return MoltObject::none().bits();
                }
            }
        }
        exception_context_set(_py, exc_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_set_last(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        if exc_obj.is_none() || exc_bits == 0 {
            clear_exception(_py);
            return MoltObject::none().bits();
        }
        let Some(ptr) = exc_obj.as_ptr() else {
            clear_exception(_py);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                clear_exception(_py);
                return MoltObject::none().bits();
            }
        }
        if debug_exception_flow() {
            let kind_bits = unsafe { exception_kind_bits(ptr) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            eprintln!("molt exc set_last task=0x{:x} kind={}", task, kind);
        }
        let trace_bits = unsafe { exception_trace_bits(ptr) };
        if obj_from_bits(trace_bits).is_none() {
            record_exception_with_caller_frame(_py, ptr, true);
            return MoltObject::none().bits();
        }
        let new_bits = MoltObject::from_ptr(ptr).bits();
        if let Some(task_key) = current_task_key() {
            let state = runtime_state(_py);
            let old_ptr = {
                let mut guard = task_last_exceptions(_py).lock().unwrap();
                guard.insert(task_key, PtrSlot(ptr))
            };
            if let Some(old_ptr) = old_ptr {
                if old_ptr.0 != ptr {
                    let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
                    dec_ref_bits(_py, old_bits);
                    inc_ref_bits(_py, new_bits);
                }
            } else {
                inc_ref_bits(_py, new_bits);
            }
            state
                .task_last_exception_pending
                .store(true, AtomicOrdering::Relaxed);
        } else {
            global_last_exception_replace_borrowed(_py, ptr, new_bits);
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        alloc_exception, clear_exception, exception_context_set, exception_last_pending_bits,
        exception_last_public_bits, exception_method_bits, exception_pending, exception_stack_pop,
        exception_stack_push, exceptions_clear_runtime_state, format_exception,
        format_exception_message, generator_exception_stack_drop, generator_exception_stack_store,
        generator_exception_stack_take, molt_exception_new_builtin_one, record_exception,
        task_exception_stack_drop, task_exception_stack_store, task_exception_stack_take,
    };
    use crate::builtins::containers::tuple_len;
    use crate::{dec_ref_bits, intern_static_name, obj_from_bits, runtime_state, seq_vec_ref};
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    #[test]
    fn exceptions_runtime_state_is_owned_and_clearable() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            let value_error_bits =
                super::builtin_exception_class_bits_for_tag(_py, 5).expect("ValueError class");
            assert_ne!(value_error_bits, 0);
            assert_ne!(
                state
                    .exceptions
                    .value_error_class_cache
                    .load(Ordering::Acquire),
                0
            );
            let traceback_method =
                exception_method_bits(_py, "with_traceback").expect("with_traceback method");
            assert_ne!(traceback_method, 0);
            assert_ne!(
                state
                    .exceptions
                    .exception_with_traceback
                    .load(Ordering::Acquire),
                0
            );
            let errno_name = intern_static_name(_py, &state.exceptions.errno_attr_name, b"errno");
            assert_ne!(errno_name, 0);
            assert_ne!(state.exceptions.errno_attr_name.load(Ordering::Acquire), 0);

            exceptions_clear_runtime_state(_py, state);

            for slot in state.exceptions.object_slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }

    #[test]
    fn generator_exception_stack_drop_clears_entries() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let boxed = Box::new(0_u8);
            let ptr = Box::into_raw(boxed);
            let bits = vec![MoltObject::none().bits(), MoltObject::none().bits()];
            generator_exception_stack_store(ptr, bits);
            generator_exception_stack_drop(_py, ptr);
            let after = generator_exception_stack_take(ptr);
            assert!(
                after.is_empty(),
                "generator exception stack should be cleared on drop"
            );
            unsafe {
                drop(Box::from_raw(ptr));
            }
        });
    }

    #[test]
    fn task_exception_stack_drop_clears_entries() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let boxed = Box::new(0_u8);
            let ptr = Box::into_raw(boxed);
            let bits = vec![MoltObject::none().bits()];
            task_exception_stack_store(_py, ptr, bits);
            task_exception_stack_drop(_py, ptr);
            let after = task_exception_stack_take(_py, ptr);
            assert!(after.is_empty(), "task exception stack should be cleared");
            unsafe {
                drop(Box::from_raw(ptr));
            }
        });
    }

    #[test]
    fn exception_last_ignores_non_pending_slots_inside_handler() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_ptr = alloc_exception(_py, "RuntimeError", "stale");
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();

            exception_stack_push();
            record_exception(_py, exc_ptr);

            let first_bits = exception_last_public_bits(_py);
            assert!(!obj_from_bits(first_bits).is_none());
            dec_ref_bits(_py, first_bits);

            exception_context_set(_py, MoltObject::none().bits());
            let stale_bits = exception_last_public_bits(_py);
            assert!(
                obj_from_bits(stale_bits).is_none(),
                "non-pending last-exception slots must not be resurrected by handler state"
            );

            clear_exception(_py);
            exception_stack_pop(_py);
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    fn exception_last_pending_ignores_active_handler_context() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let outer_ptr = alloc_exception(_py, "ValueError", "outer");
            let outer_bits = MoltObject::from_ptr(outer_ptr).bits();
            let inner_ptr = alloc_exception(_py, "TypeError", "inner");
            let inner_bits = MoltObject::from_ptr(inner_ptr).bits();

            exception_stack_push();
            exception_context_set(_py, outer_bits);
            record_exception(_py, inner_ptr);

            let pending_bits = exception_last_pending_bits(_py);
            assert_eq!(pending_bits, inner_bits);
            assert!(exception_pending(_py));
            dec_ref_bits(_py, pending_bits);

            exception_context_set(_py, MoltObject::none().bits());
            clear_exception(_py);
            exception_stack_pop(_py);
            dec_ref_bits(_py, inner_bits);
            dec_ref_bits(_py, outer_bits);
        });
    }

    #[test]
    fn builtin_exception_one_arg_materializes_args_on_demand() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_bits = molt_exception_new_builtin_one(5, MoltObject::from_int(42).bits());
            let exc_ptr = obj_from_bits(exc_bits).as_ptr().expect("exception object");
            assert!(unsafe {
                super::exception_args_is_lazy_single(super::exception_args_bits(exc_ptr))
            });
            assert_eq!(format_exception_message(_py, exc_ptr), "42");
            assert_eq!(format_exception(_py, exc_ptr), "ValueError(42)");

            let args_bits = super::exception_materialized_args_bits(_py, exc_ptr);
            let args_ptr = obj_from_bits(args_bits)
                .as_ptr()
                .expect("materialized args tuple");
            unsafe {
                assert_eq!(tuple_len(args_ptr), 1);
                assert_eq!(seq_vec_ref(args_ptr)[0], MoltObject::from_int(42).bits());
                assert!(!super::exception_args_is_lazy_single(
                    super::exception_args_bits(exc_ptr)
                ));
                assert_eq!(super::exception_args_bits(exc_ptr), args_bits);
                assert_eq!(
                    super::exception_materialized_args_bits(_py, exc_ptr),
                    args_bits
                );
            }
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    fn stop_iteration_lazy_arg_keeps_public_value_after_args_materialization() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_bits = molt_exception_new_builtin_one(8, MoltObject::from_int(99).bits());
            let exc_ptr = obj_from_bits(exc_bits).as_ptr().expect("exception object");
            unsafe {
                assert!(super::exception_args_is_lazy_single(
                    super::exception_args_bits(exc_ptr)
                ));
                assert_eq!(
                    super::exception_value_bits(exc_ptr),
                    MoltObject::from_int(99).bits()
                );
            }

            let _args_bits = super::exception_materialized_args_bits(_py, exc_ptr);
            unsafe {
                assert!(!super::exception_args_is_lazy_single(
                    super::exception_args_bits(exc_ptr)
                ));
                assert_eq!(
                    super::exception_value_bits(exc_ptr),
                    MoltObject::from_int(99).bits()
                );
            }
            dec_ref_bits(_py, exc_bits);
        });
    }
}
