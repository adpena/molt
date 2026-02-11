macro_rules! fn_addr {
    ($func:path) => {
        $func as *const () as usize as u64
    };
}

fn debug_oom() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| matches!(std::env::var("MOLT_DEBUG_OOM").ok().as_deref(), Some("1")))
}

use crate::builtins::frames::FrameEntry;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::PyToken;
use crate::{
    alloc_class_obj, alloc_dict_with_pairs, alloc_instance_for_class_no_pool, alloc_list,
    alloc_object, alloc_string, alloc_tuple, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes, builtin_classes, builtin_func_bits, bytes_like_slice,
    call_callable1, call_class_init_with_args, class_break_cycles, class_name_bits,
    class_name_for_error, code_filename_bits, code_firstlineno, code_linetable_bits,
    code_name_bits, context_stack_unwind, current_task_key, current_task_ptr, current_token_id,
    dec_ref_bits, dict_find_entry_fast, dict_get_in_place, dict_order, dict_set_in_place,
    dict_table, format_obj, format_obj_str, header_from_obj_ptr, inc_ref_bits,
    index_bigint_from_obj, instance_dict_bits, instance_set_dict_bits, int_bits_from_i64,
    intern_static_name, is_truthy, isinstance_bits, issubclass_bits, maybe_ptr_from_bits,
    module_dict_bits, molt_class_set_base, molt_dec_ref, molt_index, molt_is_callable,
    molt_iter_checked, molt_iter_next, molt_repr_from_obj, molt_str_from_obj, obj_from_bits,
    object_class_bits, object_mark_has_ptrs, object_type_id, profile_enabled, runtime_state,
    seq_vec, seq_vec_ref, string_bytes, string_len, string_obj_to_owned, task_exception_depths,
    task_exception_handler_stacks, task_exception_stacks, task_last_exceptions, to_i64,
    token_is_cancelled, traceback_suppressed, type_name, type_of_bits, MoltHeader, PtrSlot,
    RuntimeState, FRAME_STACK, HEADER_FLAG_TRACEBACK_SUPPRESSED, TRACEBACK_BUILD_COUNT,
    TRACEBACK_BUILD_FRAMES, TRACEBACK_SUPPRESS_COUNT, TYPE_ID_CODE, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE,
};
use molt_obj_model::MoltObject;
use num_traits::ToPrimitive;
use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};
use wtf8::Wtf8;

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

static STOPASYNC_BT_PRINTED: AtomicBool = AtomicBool::new(false);
static TB_LASTI_NAME: AtomicU64 = AtomicU64::new(0);
static F_BACK_NAME: AtomicU64 = AtomicU64::new(0);
static F_GLOBALS_NAME: AtomicU64 = AtomicU64::new(0);
static FILE_NAME: AtomicU64 = AtomicU64::new(0);

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
fn trace_exception_stack() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_EXCEPTION_STACK").as_deref() == Ok("1"))
}

thread_local! {
    static EXCEPTION_CLEAR_REASON: RefCell<Option<&'static str>> = const { RefCell::new(None) };
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
    use super::{AtomicU64, HashMap, Mutex};
    use crate::{runtime_state, PyToken};

    pub(crate) fn module_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).module_cache
    }

    pub(crate) fn exception_type_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).exception_type_cache
    }

    pub(crate) static ERRNO_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static STRERROR_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FILENAME_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CHARACTERS_WRITTEN_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static EXC_GROUP_MESSAGE_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static EXC_GROUP_EXCEPTIONS_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNICODE_ENCODING_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNICODE_OBJECT_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNICODE_START_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNICODE_END_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNICODE_REASON_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
}

use internals::{
    exception_type_cache, module_cache, CHARACTERS_WRITTEN_ATTR_NAME, ERRNO_ATTR_NAME,
    EXC_GROUP_EXCEPTIONS_NAME, EXC_GROUP_MESSAGE_NAME, FILENAME_ATTR_NAME, STRERROR_ATTR_NAME,
    UNICODE_ENCODING_ATTR_NAME, UNICODE_END_ATTR_NAME, UNICODE_OBJECT_ATTR_NAME,
    UNICODE_REASON_ATTR_NAME, UNICODE_START_ATTR_NAME,
};

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
        record_exception(_py, ptr);
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
        record_exception(_py, ptr);
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
        record_exception(_py, ptr);
    }
    dec_ref_bits(_py, encoding_bits);
    dec_ref_bits(_py, reason_bits);
    T::exception_sentinel()
}

pub(crate) fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken<'_>, bits: u64) -> T {
    let msg = format!(
        "'{}' object is not iterable",
        type_name(_py, obj_from_bits(bits))
    );
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
    record_exception(_py, ptr);
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
    let total = std::mem::size_of::<MoltHeader>() + 10 * std::mem::size_of::<u64>();
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
    }
    ptr
}

pub(crate) unsafe fn exception_kind_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn exception_msg_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_cause_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_context_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_suppress_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_trace_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_value_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_class_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_args_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(8 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn exception_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(9 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) fn exception_pending(_py: &PyToken<'_>) -> bool {
    let state = runtime_state(_py);
    let debug_pending = debug_exception_pending();
    if let Some(task_key) = current_task_key() {
        let pending_ptr = if state
            .task_last_exception_pending
            .load(AtomicOrdering::Relaxed)
        {
            let guard = task_last_exceptions(_py).lock().unwrap();
            guard.get(&task_key).copied()
        } else {
            None
        };
        let pending =
            pending_ptr.is_some() || state.last_exception_pending.load(AtomicOrdering::Relaxed);
        if debug_pending && pending {
            if let Some(ptr) = pending_ptr {
                let kind_bits = unsafe { exception_kind_bits(ptr.0) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!(
                    "molt exc pending task=0x{:x} kind={}",
                    task_key.0 as usize, kind
                );
            }
        }
        return pending;
    }
    let pending = state.last_exception_pending.load(AtomicOrdering::Relaxed);
    if debug_pending && pending {
        let guard = state.last_exception.lock().unwrap();
        if let Some(ptr) = *guard {
            let kind_bits = unsafe { exception_kind_bits(ptr.0) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            eprintln!("molt exc pending task=0x0 kind={}", kind);
        }
    }
    pending
}

pub(crate) fn exception_last_bits_noinc(_py: &PyToken<'_>) -> Option<u64> {
    if let Some(task_key) = current_task_key() {
        if let Some(ptr) = task_last_exceptions(_py)
            .lock()
            .unwrap()
            .get(&task_key)
            .copied()
        {
            return Some(MoltObject::from_ptr(ptr.0).bits());
        }
    }
    let guard = runtime_state(_py).last_exception.lock().unwrap();
    guard.map(|ptr| MoltObject::from_ptr(ptr.0).bits())
}

pub(crate) fn clear_exception_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    let state = runtime_state(_py);
    let ptr = {
        let mut guard = state.last_exception.lock().unwrap();
        let ptr = guard.take();
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Relaxed);
        ptr
    };
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

pub(crate) fn exception_handler_active() -> bool {
    EXCEPTION_STACK.with(|stack| !stack.borrow().is_empty())
}

pub(crate) fn exception_stack_baseline_get() -> usize {
    EXCEPTION_STACK_BASELINE.with(|cell| cell.get())
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
    if obj_from_bits(bits).is_none() {
        return;
    }
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(slot) = stack.last_mut() else {
            return;
        };
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
            if let Some(bits) = stack.pop() {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
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
    let underflow = EXCEPTION_STACK.with(|stack| stack.borrow_mut().pop().is_none());
    if underflow {
        if token_is_cancelled(_py, current_token_id()) {
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                for bits in stack.drain(..) {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
            });
            exception_context_align_depth(_py, 0);
            return;
        }
        raise_exception::<()>(_py, "RuntimeError", "exception handler stack underflow");
    }
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(bits) = stack.pop() {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
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

pub(crate) fn record_exception(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    let state = runtime_state(_py);
    let task_key = current_task_key();
    let mut prior_ptr = None;
    let mut context_bits: Option<u64> = None;
    let mut same_ptr = false;
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
    } else {
        let mut guard = state.last_exception.lock().unwrap();
        if let Some(old_ptr) = guard.take() {
            prior_ptr = Some(old_ptr.0);
        }
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Relaxed);
    }
    if let Some(old_ptr) = prior_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr).bits();
        if old_ptr == ptr {
            same_ptr = true;
        } else {
            context_bits = Some(old_bits);
            dec_ref_bits(_py, old_bits);
        }
    }
    if context_bits.is_none() {
        context_bits = exception_context_active_bits();
    }
    if let Some(ctx_bits) = context_bits {
        let new_bits = MoltObject::from_ptr(ptr).bits();
        if ctx_bits != new_bits {
            let existing = unsafe { exception_context_bits(ptr) };
            if obj_from_bits(existing).is_none() {
                unsafe {
                    inc_ref_bits(_py, ctx_bits);
                    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = ctx_bits;
                }
            }
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
        let cause_bits = unsafe { exception_cause_bits(ptr) };
        let include_caller_frame = !obj_from_bits(cause_bits).is_none();
        if let Some(new_bits) =
            frame_stack_trace_bits(_py, handler_frame_index, include_caller_frame)
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
        task_last_exceptions(_py)
            .lock()
            .unwrap()
            .insert(task_key, PtrSlot(ptr));
        state
            .task_last_exception_pending
            .store(true, AtomicOrdering::Relaxed);
    } else {
        let mut guard = state.last_exception.lock().unwrap();
        *guard = Some(PtrSlot(ptr));
        state
            .last_exception_pending
            .store(true, AtomicOrdering::Relaxed);
    }
    if std::env::var("MOLT_DEBUG_EXCEPTIONS").as_deref() == Ok("1") {
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
    let new_bits = MoltObject::from_ptr(ptr).bits();
    if !same_ptr {
        inc_ref_bits(_py, new_bits);
    }
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
    let mut guard = state.last_exception.lock().unwrap();
    if let Some(old_ptr) = guard.take() {
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Relaxed);
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
    } else {
        state
            .last_exception_pending
            .store(false, AtomicOrdering::Relaxed);
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
        "NotImplementedError" | "RecursionError" => Some(ExceptionBaseSpec::One("RuntimeError")),
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
    let order = dict_order(dict_ptr);
    let table = dict_table(dict_ptr);
    let found = dict_find_entry_fast(_py, order, table, key_bits);
    found.map(|idx| order[idx * 2 + 1])
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
    class_ptr
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

struct ExceptionGroupItems {
    items: Vec<u64>,
    all_exception: bool,
}

struct ExceptionGroupItem {
    bits: u64,
    owned: bool,
}

fn exception_group_message_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let key_bits = intern_static_name(_py, &EXC_GROUP_MESSAGE_NAME, b"message");
                    if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, key_bits) {
                        return val_bits;
                    }
                }
            }
        }
    }
    unsafe { exception_msg_bits(ptr) }
}

fn exception_group_exceptions_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let key_bits = intern_static_name(_py, &EXC_GROUP_EXCEPTIONS_NAME, b"exceptions");
        dict_get_in_place(_py, dict_ptr, key_bits)
    }
}

fn exception_dict_attr_bits(_py: &PyToken<'_>, ptr: *mut u8, name: &[u8]) -> Option<u64> {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let key_bits = attr_name_bits_from_bytes(_py, name)?;
        let out = dict_get_in_place(_py, dict_ptr, key_bits);
        dec_ref_bits(_py, key_bits);
        out
    }
}

fn exception_group_collect_exceptions(
    _py: &PyToken<'_>,
    exceptions_bits: u64,
) -> Option<ExceptionGroupItems> {
    let builtins = builtin_classes(_py);
    let mut items: Vec<u64> = Vec::new();
    let mut all_exception = true;
    let exceptions_obj = obj_from_bits(exceptions_bits);
    if let Some(ptr) = exceptions_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                if elems.is_empty() {
                    let _ = raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "second argument (exceptions) must be a non-empty sequence",
                    );
                    return None;
                }
                for (idx, &item_bits) in elems.iter().enumerate() {
                    let item_class = type_of_bits(_py, item_bits);
                    if !issubclass_bits(item_class, builtins.base_exception) {
                        let msg = format!(
                            "Item {idx} of second argument (exceptions) is not an exception"
                        );
                        let _ = raise_exception::<u64>(_py, "ValueError", &msg);
                        return None;
                    }
                    if !issubclass_bits(item_class, builtins.exception) {
                        all_exception = false;
                    }
                    items.push(item_bits);
                }
                return Some(ExceptionGroupItems {
                    items,
                    all_exception,
                });
            }
        }
        let getitem_name = attr_name_bits_from_bytes(_py, b"__getitem__")?;
        let getitem_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, getitem_name) };
        dec_ref_bits(_py, getitem_name);
        if let Some(bits) = getitem_bits {
            dec_ref_bits(_py, bits);
        } else {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "second argument (exceptions) must be a sequence",
            );
            return None;
        }
        let mut index = 0i64;
        loop {
            let idx_bits = MoltObject::from_int(index).bits();
            let item_bits = molt_index(exceptions_bits, idx_bits);
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let exc_obj = obj_from_bits(exc_bits);
                let mut is_index = false;
                if let Some(exc_ptr) = exc_obj.as_ptr() {
                    unsafe {
                        if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                            let kind_bits = exception_kind_bits(exc_ptr);
                            let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                            if kind.as_deref() == Some("IndexError") {
                                is_index = true;
                            }
                        }
                    }
                }
                if is_index {
                    clear_exception(_py);
                    dec_ref_bits(_py, exc_bits);
                    if items.is_empty() {
                        let _ = raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "second argument (exceptions) must be a non-empty sequence",
                        );
                        return None;
                    }
                    break;
                }
                dec_ref_bits(_py, exc_bits);
                return None;
            }
            let item_class = type_of_bits(_py, item_bits);
            if !issubclass_bits(item_class, builtins.base_exception) {
                let msg =
                    format!("Item {index} of second argument (exceptions) is not an exception");
                let _ = raise_exception::<u64>(_py, "ValueError", &msg);
                return None;
            }
            if !issubclass_bits(item_class, builtins.exception) {
                all_exception = false;
            }
            items.push(item_bits);
            index += 1;
        }
        return Some(ExceptionGroupItems {
            items,
            all_exception,
        });
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "second argument (exceptions) must be a sequence",
    );
    None
}

fn exception_group_alloc(
    _py: &PyToken<'_>,
    class_bits: u64,
    message_bits: u64,
    args_exceptions_bits: u64,
    items: &[u64],
    exceptions_tuple_bits: Option<u64>,
) -> Option<u64> {
    let tuple_bits = if let Some(bits) = exceptions_tuple_bits {
        bits
    } else {
        let tuple_ptr = alloc_tuple(_py, items);
        if tuple_ptr.is_null() {
            return None;
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    };
    let args_ptr = alloc_tuple(_py, &[message_bits, args_exceptions_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, tuple_bits);
        return None;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let msg_name_bits = intern_static_name(_py, &EXC_GROUP_MESSAGE_NAME, b"message");
    let exceptions_name_bits = intern_static_name(_py, &EXC_GROUP_EXCEPTIONS_NAME, b"exceptions");
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            msg_name_bits,
            message_bits,
            exceptions_name_bits,
            tuple_bits,
        ],
    );
    if dict_ptr.is_null() {
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, tuple_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let kind_bits = if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
        unsafe { class_name_bits(class_ptr) }
    } else {
        0
    };
    let ptr = alloc_exception_obj(
        _py,
        kind_bits,
        message_bits,
        class_bits,
        args_bits,
        dict_bits,
    );
    dec_ref_bits(_py, dict_bits);
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, tuple_bits);
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

unsafe fn exception_group_set_slot_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    slot_idx: usize,
    bits: u64,
) {
    let slot = ptr.add(slot_idx * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
    }
}

unsafe fn exception_group_copy_metadata(
    _py: &PyToken<'_>,
    dest_ptr: *mut u8,
    src_ptr: *mut u8,
    copy_context: bool,
    copy_trace: bool,
    suppress: bool,
) {
    if copy_context {
        let cause_bits = exception_cause_bits(src_ptr);
        let context_bits = exception_context_bits(src_ptr);
        exception_group_set_slot_bits(_py, dest_ptr, 2, cause_bits);
        exception_group_set_slot_bits(_py, dest_ptr, 3, context_bits);
    }
    if copy_trace {
        let trace_bits = exception_trace_bits(src_ptr);
        exception_group_set_slot_bits(_py, dest_ptr, 5, trace_bits);
    }
    let suppress_bits = MoltObject::from_bool(suppress).bits();
    exception_group_set_slot_bits(_py, dest_ptr, 4, suppress_bits);
}

enum ExceptionGroupMatcher {
    Type(u64),
    Callable(u64),
}

fn exception_group_parse_matcher(
    _py: &PyToken<'_>,
    matcher_bits: u64,
) -> Option<ExceptionGroupMatcher> {
    let builtins = builtin_classes(_py);
    let matcher_obj = obj_from_bits(matcher_bits);
    let Some(ptr) = matcher_obj.as_ptr() else {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
        );
        return None;
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => {
                if !issubclass_bits(matcher_bits, builtins.base_exception) {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                    );
                    return None;
                }
                return Some(ExceptionGroupMatcher::Type(matcher_bits));
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                if elems.is_empty() {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                    );
                    return None;
                }
                for &elem_bits in elems.iter() {
                    let Some(elem_ptr) = obj_from_bits(elem_bits).as_ptr() else {
                        let _ = raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                        );
                        return None;
                    };
                    if object_type_id(elem_ptr) != TYPE_ID_TYPE
                        || !issubclass_bits(elem_bits, builtins.base_exception)
                    {
                        let _ = raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                        );
                        return None;
                    }
                }
                return Some(ExceptionGroupMatcher::Type(matcher_bits));
            }
            _ => {}
        }
    }
    let callable_bits = molt_is_callable(matcher_bits);
    if is_truthy(_py, obj_from_bits(callable_bits)) {
        return Some(ExceptionGroupMatcher::Callable(matcher_bits));
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
    );
    None
}

fn exception_group_parse_except_star_matcher(_py: &PyToken<'_>, matcher_bits: u64) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let matcher_obj = obj_from_bits(matcher_bits);
    let Some(ptr) = matcher_obj.as_ptr() else {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "catching classes that do not inherit from BaseException is not allowed",
        );
        return None;
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_TYPE {
            if !issubclass_bits(matcher_bits, builtins.base_exception) {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching classes that do not inherit from BaseException is not allowed",
                );
                return None;
            }
            if issubclass_bits(matcher_bits, builtins.base_exception_group) {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching ExceptionGroup with except* is not allowed. Use except instead.",
                );
                return None;
            }
            return Some(matcher_bits);
        }
        if type_id == TYPE_ID_TUPLE {
            let elems = seq_vec_ref(ptr);
            if elems.is_empty() {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching classes that do not inherit from BaseException is not allowed",
                );
                return None;
            }
            for &elem_bits in elems.iter() {
                let Some(elem_ptr) = obj_from_bits(elem_bits).as_ptr() else {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching classes that do not inherit from BaseException is not allowed",
                    );
                    return None;
                };
                if object_type_id(elem_ptr) != TYPE_ID_TYPE
                    || !issubclass_bits(elem_bits, builtins.base_exception)
                {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching classes that do not inherit from BaseException is not allowed",
                    );
                    return None;
                }
            }
            for &elem_bits in elems.iter() {
                if issubclass_bits(elem_bits, builtins.base_exception_group) {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching ExceptionGroup with except* is not allowed. Use except instead.",
                    );
                    return None;
                }
            }
            return Some(matcher_bits);
        }
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "catching classes that do not inherit from BaseException is not allowed",
    );
    None
}

fn exception_group_matcher_matches(
    _py: &PyToken<'_>,
    matcher: &ExceptionGroupMatcher,
    exc_bits: u64,
) -> Option<bool> {
    match matcher {
        ExceptionGroupMatcher::Type(class_bits) => {
            Some(isinstance_bits(_py, exc_bits, *class_bits))
        }
        ExceptionGroupMatcher::Callable(call_bits) => {
            let res_bits = unsafe { call_callable1(_py, *call_bits, exc_bits) };
            if exception_pending(_py) {
                return None;
            }
            Some(is_truthy(_py, obj_from_bits(res_bits)))
        }
    }
}

fn exception_group_split_node(
    _py: &PyToken<'_>,
    exc_bits: u64,
    matcher: &ExceptionGroupMatcher,
) -> Option<(Option<ExceptionGroupItem>, Option<ExceptionGroupItem>)> {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return Some((None, None));
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return Some((
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            ));
        }
    }
    if let Some(matches) = exception_group_matcher_matches(_py, matcher, exc_bits) {
        if matches {
            return Some((
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
                None,
            ));
        }
    } else {
        return None;
    }
    let class_bits = unsafe { exception_class_bits(exc_ptr) };
    let base_group_bits = builtin_classes(_py).base_exception_group;
    if !issubclass_bits(class_bits, base_group_bits) {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    }
    let Some(exceptions_bits) = exception_group_exceptions_bits(_py, exc_ptr) else {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    };
    let exceptions_obj = obj_from_bits(exceptions_bits);
    let Some(ex_ptr) = exceptions_obj.as_ptr() else {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    };
    unsafe {
        if object_type_id(ex_ptr) != TYPE_ID_TUPLE && object_type_id(ex_ptr) != TYPE_ID_LIST {
            return Some((
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            ));
        }
        let elems = seq_vec_ref(ex_ptr);
        let mut match_items: Vec<ExceptionGroupItem> = Vec::new();
        let mut rest_items: Vec<ExceptionGroupItem> = Vec::new();
        for &item_bits in elems.iter() {
            let (match_part, rest_part) = exception_group_split_node(_py, item_bits, matcher)?;
            if let Some(bits) = match_part {
                match_items.push(bits);
            }
            if let Some(bits) = rest_part {
                rest_items.push(bits);
            }
        }
        let message_bits = exception_group_message_bits(_py, exc_ptr);
        let mut match_bits = None;
        let mut rest_bits = None;
        if !match_items.is_empty() {
            let match_vals: Vec<u64> = match_items.iter().map(|item| item.bits).collect();
            let list_ptr = alloc_list(_py, &match_vals);
            if list_ptr.is_null() {
                return None;
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            match_bits =
                exception_group_alloc(_py, class_bits, message_bits, list_bits, &match_vals, None);
            dec_ref_bits(_py, list_bits);
            if let Some(bits) = match_bits {
                if let Some(new_ptr) = obj_from_bits(bits).as_ptr() {
                    exception_group_copy_metadata(_py, new_ptr, exc_ptr, true, true, true);
                }
            }
            for item in match_items.into_iter() {
                if item.owned {
                    dec_ref_bits(_py, item.bits);
                }
            }
        }
        if !rest_items.is_empty() {
            let rest_vals: Vec<u64> = rest_items.iter().map(|item| item.bits).collect();
            let list_ptr = alloc_list(_py, &rest_vals);
            if list_ptr.is_null() {
                return None;
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            rest_bits =
                exception_group_alloc(_py, class_bits, message_bits, list_bits, &rest_vals, None);
            dec_ref_bits(_py, list_bits);
            if let Some(bits) = rest_bits {
                if let Some(new_ptr) = obj_from_bits(bits).as_ptr() {
                    exception_group_copy_metadata(_py, new_ptr, exc_ptr, true, true, true);
                }
            }
            for item in rest_items.into_iter() {
                if item.owned {
                    dec_ref_bits(_py, item.bits);
                }
            }
        }
        Some((
            match_bits.map(|bits| ExceptionGroupItem { bits, owned: true }),
            rest_bits.map(|bits| ExceptionGroupItem { bits, owned: true }),
        ))
    }
}

fn exception_group_make_pair_tuple(
    _py: &PyToken<'_>,
    match_item: Option<ExceptionGroupItem>,
    rest_item: Option<ExceptionGroupItem>,
) -> u64 {
    let none_bits = MoltObject::none().bits();
    let match_bits = match_item
        .as_ref()
        .map(|item| item.bits)
        .unwrap_or(none_bits);
    let rest_bits = rest_item
        .as_ref()
        .map(|item| item.bits)
        .unwrap_or(none_bits);
    let tuple_ptr = alloc_tuple(_py, &[match_bits, rest_bits]);
    if tuple_ptr.is_null() {
        if let Some(item) = match_item {
            if item.owned {
                dec_ref_bits(_py, item.bits);
            }
        }
        if let Some(item) = rest_item {
            if item.owned {
                dec_ref_bits(_py, item.bits);
            }
        }
        return MoltObject::none().bits();
    }
    if let Some(item) = match_item {
        if item.owned {
            dec_ref_bits(_py, item.bits);
        }
    }
    if let Some(item) = rest_item {
        if item.owned {
            dec_ref_bits(_py, item.bits);
        }
    }
    MoltObject::from_ptr(tuple_ptr).bits()
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
    crate::gil_assert();
    let args_slot = ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
    let old_args = *args_slot;
    if old_args != args_bits {
        dec_ref_bits(_py, old_args);
        *args_slot = args_bits;
    }
    let msg_slot = ptr.add(std::mem::size_of::<u64>()) as *mut u64;
    let old_msg = *msg_slot;
    if old_msg != msg_bits {
        dec_ref_bits(_py, old_msg);
        *msg_slot = msg_bits;
    }
}

pub(crate) unsafe fn exception_set_stop_iteration_value(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) {
    crate::gil_assert();
    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr))).unwrap_or_default();
    if kind != "StopIteration" {
        return;
    }
    let mut value_bits = MoltObject::none().bits();
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
    let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != value_bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, value_bits);
        *slot = value_bits;
    }
}

pub(crate) unsafe fn exception_set_system_exit_code(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) {
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

fn oserror_root_name(name: &str) -> bool {
    matches!(name, "OSError" | "EnvironmentError" | "IOError")
}

fn oserror_subclass_for_errno(errno: i64) -> Option<&'static str> {
    if errno == libc::EAGAIN as i64
        || errno == libc::EALREADY as i64
        || errno == libc::EWOULDBLOCK as i64
        || errno == libc::EINPROGRESS as i64
    {
        return Some("BlockingIOError");
    }
    if errno == libc::ECHILD as i64 {
        return Some("ChildProcessError");
    }
    if errno == libc::EPIPE as i64 {
        return Some("BrokenPipeError");
    }
    #[cfg(not(target_arch = "wasm32"))]
    if errno == libc::ESHUTDOWN as i64 {
        return Some("BrokenPipeError");
    }
    if errno == libc::ECONNABORTED as i64 {
        return Some("ConnectionAbortedError");
    }
    if errno == libc::ECONNREFUSED as i64 {
        return Some("ConnectionRefusedError");
    }
    if errno == libc::ECONNRESET as i64 {
        return Some("ConnectionResetError");
    }
    if errno == libc::EEXIST as i64 {
        return Some("FileExistsError");
    }
    if errno == libc::ENOENT as i64 {
        return Some("FileNotFoundError");
    }
    if errno == libc::EINTR as i64 {
        return Some("InterruptedError");
    }
    if errno == libc::EISDIR as i64 {
        return Some("IsADirectoryError");
    }
    if errno == libc::ENOTDIR as i64 {
        return Some("NotADirectoryError");
    }
    if errno == libc::EACCES as i64 || errno == libc::EPERM as i64 {
        return Some("PermissionError");
    }
    #[cfg(target_os = "freebsd")]
    if errno == libc::ENOTCAPABLE as i64 {
        return Some("PermissionError");
    }
    if errno == libc::ESRCH as i64 {
        return Some("ProcessLookupError");
    }
    if errno == libc::ETIMEDOUT as i64 {
        return Some("TimeoutError");
    }
    None
}

pub(crate) unsafe fn oserror_args(args_bits: u64) -> (Option<i64>, u64, u64) {
    let mut errno_val = None;
    let mut strerror_bits = MoltObject::none().bits();
    let mut filename_bits = MoltObject::none().bits();
    let args_obj = obj_from_bits(args_bits);
    if let Some(args_ptr) = args_obj.as_ptr() {
        let type_id = object_type_id(args_ptr);
        if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
            let elems = seq_vec_ref(args_ptr);
            if let Some(first) = elems.first() {
                errno_val = to_i64(obj_from_bits(*first));
            }
            if let Some(second) = elems.get(1) {
                strerror_bits = *second;
            }
            if let Some(third) = elems.get(2) {
                filename_bits = *third;
            }
        }
    }
    (errno_val, strerror_bits, filename_bits)
}

pub(crate) fn raise_os_error_errno<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    errno: i64,
    message: &str,
) -> T {
    let errno_bits = MoltObject::from_int(errno).bits();
    let msg_ptr = alloc_string(_py, message.as_bytes());
    if msg_ptr.is_null() {
        return T::exception_sentinel();
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[errno_bits, msg_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, msg_bits);
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let class_bits = exception_type_bits_from_name(_py, "OSError");
    let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
    dec_ref_bits(_py, args_bits);
    if !ptr.is_null() {
        let dict_bits = unsafe { exception_dict_bits(ptr) };
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let errno_name = intern_static_name(_py, &ERRNO_ATTR_NAME, b"errno");
                        let errno_bits = MoltObject::from_int(errno).bits();
                        dict_set_in_place(_py, dict_ptr, errno_name, errno_bits);
                    }
                }
            }
        }
        record_exception(_py, ptr);
    }
    T::exception_sentinel()
}

pub(crate) fn raise_os_error<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    err: std::io::Error,
    context: &str,
) -> T {
    let errno = err
        .raw_os_error()
        .map(|val| val as i64)
        .unwrap_or(libc::EIO as i64);
    let msg = if context.is_empty() {
        err.to_string()
    } else {
        format!("{context}: {}", err)
    };
    let msg = if msg.contains("Errno") {
        msg
    } else {
        format!("[Errno {errno}] {msg}")
    };
    raise_os_error_errno(_py, errno, &msg)
}

unsafe fn oserror_attr_dict(
    _py: &PyToken<'_>,
    errno_val: Option<i64>,
    strerror_bits: u64,
    filename_bits: u64,
) -> u64 {
    let errno_name = intern_static_name(_py, &ERRNO_ATTR_NAME, b"errno");
    let strerror_name = intern_static_name(_py, &STRERROR_ATTR_NAME, b"strerror");
    let filename_name = intern_static_name(_py, &FILENAME_ATTR_NAME, b"filename");
    let errno_bits = match errno_val {
        Some(val) => MoltObject::from_int(val).bits(),
        None => MoltObject::none().bits(),
    };
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            errno_name,
            errno_bits,
            strerror_name,
            strerror_bits,
            filename_name,
            filename_bits,
        ],
    );
    if dict_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(dict_ptr).bits()
}

#[derive(Clone, Copy)]
enum UnicodeErrorKind {
    Encode,
    Decode,
    Translate,
}

#[derive(Clone, Copy)]
struct UnicodeErrorFields {
    encoding_bits: u64,
    object_bits: u64,
    start_bits: u64,
    end_bits: u64,
    reason_bits: u64,
}

fn unicode_error_kind(name: &str) -> Option<UnicodeErrorKind> {
    match name {
        "UnicodeEncodeError" => Some(UnicodeErrorKind::Encode),
        "UnicodeDecodeError" => Some(UnicodeErrorKind::Decode),
        "UnicodeTranslateError" => Some(UnicodeErrorKind::Translate),
        _ => None,
    }
}

fn unicode_error_index_bits(_py: &PyToken<'_>, obj_bits: u64) -> Result<u64, ()> {
    let type_label = type_name(_py, obj_from_bits(obj_bits));
    let err = format!(
        "'{}' object cannot be interpreted as an integer",
        type_label
    );
    let Some(value) = index_bigint_from_obj(_py, obj_bits, &err) else {
        return Err(());
    };
    if let Some(val) = value.to_i64() {
        return Ok(int_bits_from_i64(_py, val));
    }
    let _ = raise_exception::<u64>(
        _py,
        "OverflowError",
        "Python int too large to convert to C ssize_t",
    );
    Err(())
}

fn unicode_error_fields_from_args(
    _py: &PyToken<'_>,
    kind: UnicodeErrorKind,
    args_bits: u64,
) -> Result<UnicodeErrorFields, ()> {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        return Err(());
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            return Err(());
        }
        let elems = seq_vec_ref(args_ptr);
        let expected = match kind {
            UnicodeErrorKind::Translate => 4,
            UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => 5,
        };
        if elems.len() != expected {
            let msg = format!(
                "function takes exactly {expected} arguments ({} given)",
                elems.len()
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        let (encoding_bits, object_bits, start_bits, end_bits, reason_bits, object_idx) = match kind
        {
            UnicodeErrorKind::Translate => (
                MoltObject::none().bits(),
                elems[0],
                elems[1],
                elems[2],
                elems[3],
                1,
            ),
            UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => {
                (elems[0], elems[1], elems[2], elems[3], elems[4], 2)
            }
        };
        let builtins = builtin_classes(_py);
        if matches!(kind, UnicodeErrorKind::Encode | UnicodeErrorKind::Decode)
            && !isinstance_bits(_py, encoding_bits, builtins.str)
        {
            let msg = format!(
                "argument 1 must be str, not {}",
                type_name(_py, obj_from_bits(encoding_bits))
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        match kind {
            UnicodeErrorKind::Decode => {
                let is_bytes_like = obj_from_bits(object_bits)
                    .as_ptr()
                    .is_some_and(|ptr| bytes_like_slice(ptr).is_some());
                if !is_bytes_like {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, obj_from_bits(object_bits))
                    );
                    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                    return Err(());
                }
            }
            UnicodeErrorKind::Encode | UnicodeErrorKind::Translate => {
                if !isinstance_bits(_py, object_bits, builtins.str) {
                    let msg = format!(
                        "argument {object_idx} must be str, not {}",
                        type_name(_py, obj_from_bits(object_bits))
                    );
                    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                    return Err(());
                }
            }
        }
        if !isinstance_bits(_py, reason_bits, builtins.str) {
            let arg_index = match kind {
                UnicodeErrorKind::Translate => 4,
                UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => 5,
            };
            let msg = format!(
                "argument {arg_index} must be str, not {}",
                type_name(_py, obj_from_bits(reason_bits))
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        let start_bits = unicode_error_index_bits(_py, start_bits)?;
        let end_bits = unicode_error_index_bits(_py, end_bits)?;
        Ok(UnicodeErrorFields {
            encoding_bits,
            object_bits,
            start_bits,
            end_bits,
            reason_bits,
        })
    }
}

fn unicode_error_attr_dict(_py: &PyToken<'_>, fields: UnicodeErrorFields) -> u64 {
    let encoding_name = intern_static_name(_py, &UNICODE_ENCODING_ATTR_NAME, b"encoding");
    let object_name = intern_static_name(_py, &UNICODE_OBJECT_ATTR_NAME, b"object");
    let start_name = intern_static_name(_py, &UNICODE_START_ATTR_NAME, b"start");
    let end_name = intern_static_name(_py, &UNICODE_END_ATTR_NAME, b"end");
    let reason_name = intern_static_name(_py, &UNICODE_REASON_ATTR_NAME, b"reason");
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            encoding_name,
            fields.encoding_bits,
            object_name,
            fields.object_bits,
            start_name,
            fields.start_bits,
            end_name,
            fields.end_bits,
            reason_name,
            fields.reason_bits,
        ],
    );
    if dict_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(dict_ptr).bits()
    }
}

fn alloc_exception_group_from_class_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> *mut u8 {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        dec_ref_bits(_py, args_bits);
        return std::ptr::null_mut();
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let args_elems = seq_vec_ref(args_ptr);
        let argc = args_elems.len();
        if argc != 2 {
            let msg = format!(
                "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
                argc
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let message_bits = args_elems[0];
        let exceptions_bits = args_elems[1];
        let message_obj = obj_from_bits(message_bits);
        if let Some(msg_ptr) = message_obj.as_ptr() {
            if object_type_id(msg_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                    type_name(_py, message_obj)
                );
                let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                dec_ref_bits(_py, args_bits);
                return std::ptr::null_mut();
            }
        } else {
            let msg = format!(
                "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                type_name(_py, message_obj)
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let Some(collected) = exception_group_collect_exceptions(_py, exceptions_bits) else {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        };
        let builtins = builtin_classes(_py);
        let strict_exception = issubclass_bits(class_bits, builtins.exception);
        if strict_exception && !collected.all_exception {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "Cannot nest BaseExceptions in an ExceptionGroup",
            );
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let Some(bits) = exception_group_alloc(
            _py,
            class_bits,
            message_bits,
            exceptions_bits,
            &collected.items,
            None,
        ) else {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        };
        dec_ref_bits(_py, args_bits);
        obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
    }
}

pub(crate) fn alloc_exception_from_class_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> *mut u8 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return std::ptr::null_mut();
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return std::ptr::null_mut();
        }
        let mut class_bits = class_bits;
        let mut class_ptr = class_ptr;
        let mut kind_bits = class_name_bits(class_ptr);
        let args_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(args_bits).is_none() {
            return std::ptr::null_mut();
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits != 0 && issubclass_bits(class_bits, base_group_bits) {
            return alloc_exception_group_from_class_bits(_py, class_bits, args_bits);
        }
        let (errno_val, strerror_bits, filename_bits) = oserror_args(args_bits);
        let oserror_bits = exception_type_bits_from_name(_py, "OSError");
        let mut dict_bits = MoltObject::none().bits();
        if issubclass_bits(class_bits, oserror_bits) {
            let name = string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_default();
            if oserror_root_name(&name) {
                if let Some(errno_val) = errno_val {
                    if let Some(subclass) = oserror_subclass_for_errno(errno_val) {
                        let mapped_bits = exception_type_bits_from_name(_py, subclass);
                        if mapped_bits != 0 {
                            if let Some(mapped_ptr) = obj_from_bits(mapped_bits).as_ptr() {
                                class_bits = mapped_bits;
                                class_ptr = mapped_ptr;
                                kind_bits = class_name_bits(class_ptr);
                            }
                        }
                    }
                }
            }
            dict_bits = oserror_attr_dict(_py, errno_val, strerror_bits, filename_bits);
            let blocking_bits = exception_type_bits_from_name(_py, "BlockingIOError");
            if blocking_bits != 0 && issubclass_bits(class_bits, blocking_bits) {
                let mut chars_bits = MoltObject::none().bits();
                let args_obj = obj_from_bits(args_bits);
                if let Some(args_ptr) = args_obj.as_ptr() {
                    let type_id = object_type_id(args_ptr);
                    if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                        let elems = seq_vec_ref(args_ptr);
                        if let Some(third) = elems.get(2) {
                            chars_bits = *third;
                        }
                    }
                }
                let chars_obj = obj_from_bits(chars_bits);
                if (chars_obj.is_int() || chars_obj.is_bool()) && dict_bits != 0 {
                    let name_bits = intern_static_name(
                        _py,
                        &CHARACTERS_WRITTEN_ATTR_NAME,
                        b"characters_written",
                    );
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            dict_set_in_place(_py, dict_ptr, name_bits, chars_bits);
                        }
                    }
                }
            }
        }
        if let Some(name) = string_obj_to_owned(obj_from_bits(kind_bits)) {
            if let Some(kind) = unicode_error_kind(&name) {
                let fields = match unicode_error_fields_from_args(_py, kind, args_bits) {
                    Ok(fields) => fields,
                    Err(()) => {
                        dec_ref_bits(_py, args_bits);
                        return std::ptr::null_mut();
                    }
                };
                dict_bits = unicode_error_attr_dict(_py, fields);
            }
        }
        let msg_bits = exception_message_from_args(_py, args_bits);
        if obj_from_bits(msg_bits).is_none() {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let none_bits = MoltObject::none().bits();
        let ptr = alloc_exception_obj(_py, kind_bits, msg_bits, class_bits, args_bits, dict_bits);
        if !ptr.is_null() {
            exception_set_stop_iteration_value(_py, ptr, args_bits);
            exception_set_system_exit_code(_py, ptr, args_bits);
        }
        if dict_bits != none_bits {
            dec_ref_bits(_py, dict_bits);
        }
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, msg_bits);
        ptr
    }
}

fn exception_args_vec(ptr: *mut u8) -> Vec<u64> {
    unsafe {
        let args_bits = exception_args_bits(ptr);
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            let type_id = object_type_id(args_ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                return seq_vec_ref(args_ptr).clone();
            }
        }
        if args_obj.is_none() {
            Vec::new()
        } else {
            vec![args_bits]
        }
    }
}

fn exception_class_name(ptr: *mut u8) -> String {
    unsafe {
        let class_bits = exception_class_bits(ptr);
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                let name_bits = class_name_bits(class_ptr);
                if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                    return name;
                }
            }
        }
        string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr)))
            .unwrap_or_else(|| "Exception".to_string())
    }
}

pub(crate) fn format_exception(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let kind = exception_class_name(ptr);
    let args = exception_args_vec(ptr);
    if args.is_empty() {
        return format!("{kind}()");
    }
    if args.len() == 1 {
        let arg_repr = format_obj(_py, obj_from_bits(args[0]));
        return format!("{kind}({arg_repr})");
    }
    let args_repr = format_obj(_py, obj_from_bits(unsafe { exception_args_bits(ptr) }));
    format!("{kind}{args_repr}")
}

pub(crate) fn format_exception_with_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let mut out = String::new();
    if let Some(trace) = format_traceback(_py, ptr) {
        out.push_str(&trace);
    }
    let kind = exception_class_name(ptr);
    let message = format_exception_message(_py, ptr);
    if message.is_empty() {
        out.push_str(&kind);
    } else {
        out.push_str(&format!("{kind}: {message}"));
    }
    out
}

pub(crate) fn format_exception_message(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let mut class_bits = unsafe { exception_class_bits(ptr) };
    if obj_from_bits(class_bits).is_none() || class_bits == 0 {
        class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(ptr)) };
    }
    let kind = exception_class_name(ptr);
    if kind == "UnicodeDecodeError" {
        if let Some(msg) = format_unicode_decode_error(_py, ptr) {
            return msg;
        }
    }
    if kind == "UnicodeEncodeError" {
        if let Some(msg) = format_unicode_encode_error(_py, ptr) {
            return msg;
        }
    }
    if kind == "HTTPError" {
        if let (Some(code_bits), Some(msg_bits)) = (
            exception_dict_attr_bits(_py, ptr, b"code"),
            exception_dict_attr_bits(_py, ptr, b"msg"),
        ) {
            let code = format_obj_str(_py, obj_from_bits(code_bits));
            let msg = format_obj_str(_py, obj_from_bits(msg_bits));
            return format!("HTTP Error {code}: {msg}");
        }
    }
    if kind == "URLError" || kind == "ContentTooShortError" {
        if let Some(reason_bits) = exception_dict_attr_bits(_py, ptr, b"reason") {
            let reason = format_obj_str(_py, obj_from_bits(reason_bits));
            return format!("<urlopen error {reason}>");
        }
    }
    let base_group_bits = builtin_classes(_py).base_exception_group;
    if base_group_bits != 0 && issubclass_bits(class_bits, base_group_bits) {
        let msg_bits = exception_group_message_bits(_py, ptr);
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        let mut count = 0usize;
        if let Some(ex_bits) = exception_group_exceptions_bits(_py, ptr) {
            if let Some(ex_ptr) = obj_from_bits(ex_bits).as_ptr() {
                unsafe {
                    let type_id = object_type_id(ex_ptr);
                    if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                        count = seq_vec_ref(ex_ptr).len();
                    }
                }
            }
        }
        let suffix = if count == 1 {
            "1 sub-exception".to_string()
        } else {
            format!("{count} sub-exceptions")
        };
        if msg.is_empty() {
            return format!(" ({suffix})");
        }
        return format!("{msg} ({suffix})");
    }
    let args = exception_args_vec(ptr);
    if args.is_empty() {
        return String::new();
    }
    if kind == "KeyError" && args.len() == 1 {
        return format_obj(_py, obj_from_bits(args[0]));
    }
    if args.len() == 1 {
        return format_obj_str(_py, obj_from_bits(args[0]));
    }
    format_obj_str(_py, obj_from_bits(unsafe { exception_args_bits(ptr) }))
}

fn format_unicode_decode_error(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let args = exception_args_vec(ptr);
    if args.len() != 5 {
        return None;
    }
    let encoding = string_obj_to_owned(obj_from_bits(args[0]))?;
    let reason = string_obj_to_owned(obj_from_bits(args[4]))?;
    let start = to_i64(obj_from_bits(args[2]))?;
    let end = to_i64(obj_from_bits(args[3]))?;
    if start < 0 || end < 0 {
        return None;
    }
    let start = start as usize;
    let end = end as usize;
    if end <= start {
        return None;
    }
    if end == start + 1 {
        let obj = obj_from_bits(args[1]);
        let ptr = obj.as_ptr()?;
        let bytes = unsafe { bytes_like_slice(ptr) }?;
        if start >= bytes.len() {
            return None;
        }
        let byte = bytes[start];
        return Some(format!(
            "'{encoding}' codec can't decode byte 0x{byte:02x} in position {start}: {reason}"
        ));
    }
    let end_pos = end.saturating_sub(1);
    Some(format!(
        "'{encoding}' codec can't decode bytes in position {start}-{end_pos}: {reason}"
    ))
}

fn unicode_escape_codepoint(code: u32) -> String {
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn wtf8_from_bytes(bytes: &[u8]) -> &Wtf8 {
    // SAFETY: Molt string bytes are constructed as well-formed WTF-8.
    unsafe { &*(bytes as *const [u8] as *const Wtf8) }
}

fn wtf8_codepoint_at_index(bytes: &[u8], idx: usize) -> Option<u32> {
    wtf8_from_bytes(bytes)
        .code_points()
        .nth(idx)
        .map(|cp| cp.to_u32())
}

fn format_unicode_encode_error(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let args = exception_args_vec(ptr);
    if args.len() != 5 {
        return None;
    }
    let encoding = string_obj_to_owned(obj_from_bits(args[0]))?;
    let reason = string_obj_to_owned(obj_from_bits(args[4]))?;
    let start = to_i64(obj_from_bits(args[2]))?;
    let end = to_i64(obj_from_bits(args[3]))?;
    if start < 0 || end < 0 {
        return None;
    }
    let start = start as usize;
    let end = end as usize;
    if end <= start {
        return None;
    }
    let obj = obj_from_bits(args[1]);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
        if end == start + 1 {
            let code = wtf8_codepoint_at_index(bytes, start)?;
            let escaped = unicode_escape_codepoint(code);
            return Some(format!(
                "'{encoding}' codec can't encode character '{escaped}' in position {start}: {reason}"
            ));
        }
    }
    let end_pos = end.saturating_sub(1);
    Some(format!(
        "'{encoding}' codec can't encode characters in position {start}-{end_pos}: {reason}"
    ))
}

fn format_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let trace_bits = unsafe { exception_trace_bits(ptr) };
    if obj_from_bits(trace_bits).is_none() {
        return None;
    }
    let mut out = String::from("Traceback (most recent call last):\n");
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut current_bits = trace_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 512 {
            out.push_str("  <traceback truncated>\n");
            break;
        }
        let tb_obj = obj_from_bits(current_bits);
        let Some(tb_ptr) = tb_obj.as_ptr() else {
            break;
        };
        let (frame_bits, line, next_bits) = unsafe {
            let dict_bits = instance_dict_bits(tb_ptr);
            let mut frame_bits = MoltObject::none().bits();
            let mut line = 0i64;
            let mut next_bits = MoltObject::none().bits();
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_frame_bits) {
                        frame_bits = bits;
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_lineno_bits) {
                        if let Some(val) = to_i64(obj_from_bits(bits)) {
                            line = val;
                        }
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_next_bits) {
                        next_bits = bits;
                    }
                }
            }
            (frame_bits, line, next_bits)
        };
        let (filename, func_name, frame_line) = unsafe {
            let mut filename = "<unknown>".to_string();
            let mut func_name = "<module>".to_string();
            let mut frame_line = line;
            if let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() {
                let dict_bits = instance_dict_bits(frame_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_bits) {
                            if let Some(val) = to_i64(obj_from_bits(bits)) {
                                frame_line = val;
                            }
                        }
                        if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_bits) {
                            if let Some(code_ptr) = obj_from_bits(bits).as_ptr() {
                                if object_type_id(code_ptr) == TYPE_ID_CODE {
                                    let filename_bits = code_filename_bits(code_ptr);
                                    if let Some(name) =
                                        string_obj_to_owned(obj_from_bits(filename_bits))
                                    {
                                        filename = name;
                                    }
                                    let name_bits = code_name_bits(code_ptr);
                                    if let Some(name) =
                                        string_obj_to_owned(obj_from_bits(name_bits))
                                    {
                                        if !name.is_empty() {
                                            func_name = name;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (filename, func_name, frame_line)
        };
        let final_line = if line > 0 { line } else { frame_line };
        out.push_str(&format!(
            "  File \"{filename}\", line {final_line}, in {func_name}\n"
        ));
        current_bits = next_bits;
        depth += 1;
    }
    Some(out)
}

// --- Frame stack and traceback helpers ---

pub(crate) fn frame_stack_push(_py: &PyToken<'_>, code_bits: u64) {
    crate::gil_assert();
    if code_bits != 0 {
        inc_ref_bits(_py, code_bits);
    }
    let line = if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_CODE {
                code_firstlineno(ptr)
            } else {
                0
            }
        }
    } else {
        0
    };
    FRAME_STACK.with(|stack| {
        stack.borrow_mut().push(FrameEntry { code_bits, line });
    });
}

pub(crate) fn frame_stack_set_line(line: i64) {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().last_mut() {
            entry.line = line;
        }
    });
}

pub(crate) fn frame_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().pop() {
            if entry.code_bits != 0 {
                dec_ref_bits(_py, entry.code_bits);
            }
        }
    });
}

#[derive(Clone, Copy)]
struct FrameField {
    bits: u64,
    owned: bool,
}

unsafe fn alloc_empty_dict_field(_py: &PyToken<'_>) -> Option<FrameField> {
    let ptr = alloc_dict_with_pairs(_py, &[]);
    if ptr.is_null() {
        None
    } else {
        Some(FrameField {
            bits: MoltObject::from_ptr(ptr).bits(),
            owned: true,
        })
    }
}

unsafe fn frame_line_from_entry(entry: FrameEntry) -> Option<i64> {
    if entry.code_bits == 0 {
        return None;
    }
    let code_ptr = obj_from_bits(entry.code_bits).as_ptr()?;
    if object_type_id(code_ptr) != TYPE_ID_CODE {
        return None;
    }
    let mut line = entry.line;
    if line <= 0 {
        line = code_firstlineno(code_ptr);
    }
    Some(line)
}

unsafe fn code_is_module(code_bits: u64) -> bool {
    let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
        return false;
    };
    if object_type_id(code_ptr) != TYPE_ID_CODE {
        return false;
    }
    let name_bits = code_name_bits(code_ptr);
    string_obj_to_owned(obj_from_bits(name_bits)).is_some_and(|name| name == "<module>")
}

unsafe fn frame_globals_field_for_code(_py: &PyToken<'_>, code_bits: u64) -> Option<FrameField> {
    let mut filename: Option<String> = None;
    if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
        if object_type_id(code_ptr) == TYPE_ID_CODE {
            let filename_bits = code_filename_bits(code_ptr);
            filename = string_obj_to_owned(obj_from_bits(filename_bits));
        }
    }
    let (module_bits, main_bits) = {
        let cache = runtime_state(_py).module_cache.lock().unwrap();
        (
            cache.values().copied().collect::<Vec<u64>>(),
            cache.get("__main__").copied(),
        )
    };
    if let Some(filename) = filename {
        let file_name_bits = intern_static_name(_py, &FILE_NAME, b"__file__");
        for module_bits in &module_bits {
            let Some(module_ptr) = obj_from_bits(*module_bits).as_ptr() else {
                continue;
            };
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                continue;
            }
            let dict_bits = module_dict_bits(module_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(file_bits) = dict_get_in_place(_py, dict_ptr, file_name_bits) else {
                continue;
            };
            if string_obj_to_owned(obj_from_bits(file_bits)).is_some_and(|value| value == filename)
            {
                return Some(FrameField {
                    bits: dict_bits,
                    owned: false,
                });
            }
        }
    }
    if let Some(main_bits) = main_bits {
        if let Some(module_ptr) = obj_from_bits(main_bits).as_ptr() {
            if object_type_id(module_ptr) == TYPE_ID_MODULE {
                let dict_bits = module_dict_bits(module_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        return Some(FrameField {
                            bits: dict_bits,
                            owned: false,
                        });
                    }
                }
            }
        }
    }
    alloc_empty_dict_field(_py)
}

unsafe fn frame_locals_field_for_code(
    _py: &PyToken<'_>,
    code_bits: u64,
    globals: FrameField,
) -> Option<FrameField> {
    if code_is_module(code_bits) {
        return Some(FrameField {
            bits: globals.bits,
            owned: false,
        });
    }
    alloc_empty_dict_field(_py)
}

unsafe fn alloc_frame_obj(
    _py: &PyToken<'_>,
    code_bits: u64,
    line: i64,
    back_bits: u64,
) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let class_obj = obj_from_bits(builtins.frame);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let frame_bits = alloc_instance_for_class_no_pool(_py, class_ptr);
    let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let f_lasti_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lasti_name, b"f_lasti");
    let f_back_bits = intern_static_name(_py, &F_BACK_NAME, b"f_back");
    let f_globals_bits = intern_static_name(_py, &F_GLOBALS_NAME, b"f_globals");
    let f_locals_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_locals_name, b"f_locals");
    let globals = frame_globals_field_for_code(_py, code_bits)?;
    let locals = frame_locals_field_for_code(_py, code_bits, globals)?;
    let line_bits = MoltObject::from_int(line).bits();
    let lasti_bits = MoltObject::from_int(-1).bits();
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            f_code_bits,
            code_bits,
            f_lineno_bits,
            line_bits,
            f_lasti_bits,
            lasti_bits,
            f_back_bits,
            back_bits,
            f_globals_bits,
            globals.bits,
            f_locals_bits,
            locals.bits,
        ],
    );
    if globals.owned {
        dec_ref_bits(_py, globals.bits);
    }
    if locals.owned && locals.bits != globals.bits {
        dec_ref_bits(_py, locals.bits);
    }
    if dict_ptr.is_null() {
        dec_ref_bits(_py, frame_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(_py, frame_ptr, dict_bits);
    object_mark_has_ptrs(_py, frame_ptr);
    Some(frame_bits)
}

unsafe fn alloc_traceback_obj(
    _py: &PyToken<'_>,
    frame_bits: u64,
    line: i64,
    next_bits: u64,
) -> Option<u64> {
    fn compute_tb_lasti(_py: &PyToken<'_>, frame_bits: u64, line: i64) -> i64 {
        let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() else {
            return -1;
        };
        unsafe {
            let dict_bits = instance_dict_bits(frame_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return -1;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return -1;
            }
            let f_code_bits =
                intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
            let Some(code_bits) = dict_get_in_place(_py, dict_ptr, f_code_bits) else {
                return -1;
            };
            let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
                return -1;
            };
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                return -1;
            }
            let linetable_bits = code_linetable_bits(code_ptr);
            let Some(linetable_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                return -1;
            };
            if object_type_id(linetable_ptr) != TYPE_ID_TUPLE {
                return -1;
            }
            let mut best: Option<(usize, i64)> = None;
            for (idx, entry_bits) in seq_vec_ref(linetable_ptr).iter().copied().enumerate() {
                let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(entry_ptr) != TYPE_ID_TUPLE {
                    continue;
                }
                let parts = seq_vec_ref(entry_ptr);
                if parts.len() < 4 {
                    continue;
                }
                let Some(start_line) = to_i64(obj_from_bits(parts[0])) else {
                    continue;
                };
                if start_line != line {
                    continue;
                }
                let start_col = to_i64(obj_from_bits(parts[2])).unwrap_or(-1);
                let end_col = to_i64(obj_from_bits(parts[3])).unwrap_or(start_col);
                let span = if start_col >= 0 && end_col >= start_col {
                    end_col - start_col
                } else {
                    -1
                };
                match best {
                    Some((_, best_span)) if span <= best_span => {}
                    _ => best = Some((idx, span)),
                }
            }
            if let Some((idx, _)) = best {
                return (idx as i64) * 2;
            }
            -1
        }
    }

    let builtins = builtin_classes(_py);
    let class_obj = obj_from_bits(builtins.traceback);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let tb_bits = alloc_instance_for_class_no_pool(_py, class_ptr);
    let tb_ptr = obj_from_bits(tb_bits).as_ptr()?;
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let tb_lasti_bits = intern_static_name(_py, &TB_LASTI_NAME, b"tb_lasti");
    let line_bits = MoltObject::from_int(line).bits();
    let lasti_bits = MoltObject::from_int(compute_tb_lasti(_py, frame_bits, line)).bits();
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            tb_frame_bits,
            frame_bits,
            tb_lineno_bits,
            line_bits,
            tb_next_bits,
            next_bits,
            tb_lasti_bits,
            lasti_bits,
        ],
    );
    if dict_ptr.is_null() {
        dec_ref_bits(_py, tb_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(_py, tb_ptr, dict_bits);
    object_mark_has_ptrs(_py, tb_ptr);
    Some(tb_bits)
}

unsafe fn build_frame_chain(_py: &PyToken<'_>, entries: &[FrameEntry]) -> Option<Vec<(u64, i64)>> {
    let mut out: Vec<(u64, i64)> = Vec::with_capacity(entries.len());
    let mut back_bits = MoltObject::none().bits();
    for entry in entries {
        let Some(line) = frame_line_from_entry(*entry) else {
            continue;
        };
        let frame_bits = match alloc_frame_obj(_py, entry.code_bits, line, back_bits) {
            Some(bits) => bits,
            None => {
                for (bits, _) in out {
                    dec_ref_bits(_py, bits);
                }
                return None;
            }
        };
        back_bits = frame_bits;
        out.push((frame_bits, line));
    }
    Some(out)
}

pub(crate) fn frame_stack_trace_bits(
    _py: &PyToken<'_>,
    handler_frame_index: Option<usize>,
    include_caller_frame: bool,
) -> Option<u64> {
    FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        if stack.is_empty() {
            return None;
        }
        let start = handler_frame_index
            .map(|idx| {
                if include_caller_frame {
                    idx.saturating_sub(1)
                } else {
                    idx
                }
            })
            .unwrap_or(0)
            .min(stack.len());
        let active = stack[start..].to_vec();
        if active.is_empty() {
            return None;
        }
        let frames = unsafe {
            match build_frame_chain(_py, &active) {
                Some(frames) => frames,
                None => return None,
            }
        };
        if frames.is_empty() {
            return None;
        }
        let mut next_bits = MoltObject::none().bits();
        let mut built_any = false;
        let mut frames_built: u64 = 0;
        for (frame_bits, line) in frames.iter().rev().copied() {
            unsafe {
                let Some(tb_bits) = alloc_traceback_obj(_py, frame_bits, line, next_bits) else {
                    if !obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(_py, next_bits);
                    }
                    for (bits, _) in frames.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                    return None;
                };
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(_py, next_bits);
                }
                next_bits = tb_bits;
                built_any = true;
                frames_built += 1;
            }
        }
        for (bits, _) in frames.iter().copied() {
            dec_ref_bits(_py, bits);
        }
        if !built_any || obj_from_bits(next_bits).is_none() {
            if !obj_from_bits(next_bits).is_none() {
                dec_ref_bits(_py, next_bits);
            }
            return None;
        }
        if profile_enabled(_py) {
            TRACEBACK_BUILD_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
            TRACEBACK_BUILD_FRAMES.fetch_add(frames_built, AtomicOrdering::Relaxed);
        }
        Some(next_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_getframe(depth_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let depth_val = obj_from_bits(depth_bits);
        let Some(depth) = to_i64(depth_val) else {
            return raise_exception::<u64>(_py, "TypeError", "depth must be an integer");
        };
        if depth < 0 {
            return raise_exception::<u64>(_py, "ValueError", "depth must be >= 0");
        }
        let depth = depth as usize;
        let entries = FRAME_STACK.with(|stack| {
            let stack = stack.borrow();
            if depth >= stack.len() {
                None
            } else {
                Some(stack[..=stack.len() - 1 - depth].to_vec())
            }
        });
        let Some(entries) = entries else {
            return MoltObject::none().bits();
        };
        unsafe {
            if let Some(frames) = build_frame_chain(_py, &entries) {
                if let Some((frame_bits, _)) = frames.last().copied() {
                    inc_ref_bits(_py, frame_bits);
                    for (bits, _) in frames {
                        dec_ref_bits(_py, bits);
                    }
                    return frame_bits;
                }
                for (bits, _) in frames {
                    dec_ref_bits(_py, bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_new(kind_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let msg_bits = exception_message_from_args(_py, args_bits);
        if obj_from_bits(msg_bits).is_none() {
            dec_ref_bits(_py, args_bits);
            return MoltObject::none().bits();
        }
        let class_bits = exception_type_bits(_py, kind_bits);
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

#[no_mangle]
pub extern "C" fn molt_exception_new_from_class(class_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        if !issubclass_bits(class_bits, builtins.base_exception) {
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

#[no_mangle]
pub extern "C" fn molt_exception_new_bound(class_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let out = molt_exception_new_from_class(class_bits, args_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_init(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let msg_bits = exception_message_from_args(_py, norm_bits);
        if obj_from_bits(msg_bits).is_none() {
            dec_ref_bits(_py, norm_bits);
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return MoltObject::none().bits();
        }
        let existing_bits = unsafe { exception_args_bits(self_ptr) };
        let existing_len = if let Some(ptr) = obj_from_bits(existing_bits).as_ptr() {
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
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    unsafe {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            if let Some(name) =
                                string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
                            {
                                if let Some(kind) = unicode_error_kind(&name) {
                                    let fields = match unicode_error_fields_from_args(
                                        _py, kind, norm_bits,
                                    ) {
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
                if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                            let errno_name =
                                intern_static_name(_py, &internals::ERRNO_ATTR_NAME, b"errno");
                            let strerror_name = intern_static_name(
                                _py,
                                &internals::STRERROR_ATTR_NAME,
                                b"strerror",
                            );
                            let filename_name = intern_static_name(
                                _py,
                                &internals::FILENAME_ATTR_NAME,
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
                if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                            let encoding_name = intern_static_name(
                                _py,
                                &internals::UNICODE_ENCODING_ATTR_NAME,
                                b"encoding",
                            );
                            let object_name = intern_static_name(
                                _py,
                                &internals::UNICODE_OBJECT_ATTR_NAME,
                                b"object",
                            );
                            let start_name = intern_static_name(
                                _py,
                                &internals::UNICODE_START_ATTR_NAME,
                                b"start",
                            );
                            let end_name =
                                intern_static_name(_py, &internals::UNICODE_END_ATTR_NAME, b"end");
                            let reason_name = intern_static_name(
                                _py,
                                &internals::UNICODE_REASON_ATTR_NAME,
                                b"reason",
                            );
                            unsafe {
                                dict_set_in_place(
                                    _py,
                                    dict_ptr,
                                    encoding_name,
                                    fields.encoding_bits,
                                );
                                dict_set_in_place(_py, dict_ptr, object_name, fields.object_bits);
                                dict_set_in_place(_py, dict_ptr, start_name, fields.start_bits);
                                dict_set_in_place(_py, dict_ptr, end_name, fields.end_bits);
                                dict_set_in_place(_py, dict_ptr, reason_name, fields.reason_bits);
                            }
                        }
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

#[no_mangle]
pub extern "C" fn molt_exception_add_note(self_bits: u64, note_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_init(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        unsafe {
            inc_ref_bits(_py, norm_bits);
            let args_slot = self_ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *args_slot;
            if old_bits != norm_bits {
                dec_ref_bits(_py, old_bits);
                *args_slot = norm_bits;
            }
        }
        dec_ref_bits(_py, norm_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_subgroup(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'subgroup' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(matcher) = exception_group_parse_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        if let Some(matches) = exception_group_matcher_matches(_py, &matcher, self_bits) {
            if matches {
                inc_ref_bits(_py, self_bits);
                return self_bits;
            }
        } else {
            return MoltObject::none().bits();
        }
        let Some((match_item, _rest_item)) = exception_group_split_node(_py, self_bits, &matcher)
        else {
            return MoltObject::none().bits();
        };
        if let Some(item) = match_item {
            if !item.owned {
                inc_ref_bits(_py, item.bits);
            }
            return item.bits;
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_split(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'split' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(matcher) = exception_group_parse_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        if let Some(matches) = exception_group_matcher_matches(_py, &matcher, self_bits) {
            if matches {
                return exception_group_make_pair_tuple(
                    _py,
                    Some(ExceptionGroupItem {
                        bits: self_bits,
                        owned: false,
                    }),
                    None,
                );
            }
        } else {
            return MoltObject::none().bits();
        }
        let Some((match_item, rest_item)) = exception_group_split_node(_py, self_bits, &matcher)
        else {
            return MoltObject::none().bits();
        };
        exception_group_make_pair_tuple(_py, match_item, rest_item)
    })
}

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_derive(self_bits: u64, exceptions_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'derive' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(collected) = exception_group_collect_exceptions(_py, exceptions_bits) else {
            return MoltObject::none().bits();
        };
        let builtins = builtin_classes(_py);
        let mut target_class = class_bits;
        if issubclass_bits(class_bits, builtins.exception) && !collected.all_exception {
            target_class = builtins.base_exception_group;
        }
        let message_bits = exception_group_message_bits(_py, self_ptr);
        exception_group_alloc(
            _py,
            target_class,
            message_bits,
            exceptions_bits,
            &collected.items,
            None,
        )
        .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_match(exc_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        if obj_from_bits(exc_bits).is_none() {
            return exception_group_make_pair_tuple(_py, None, None);
        }
        let Some(match_bits) = exception_group_parse_except_star_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        let exc_obj = obj_from_bits(exc_bits);
        let Some(exc_ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut exc_class_bits = unsafe { exception_class_bits(exc_ptr) };
        if obj_from_bits(exc_class_bits).is_none() || exc_class_bits == 0 {
            exc_class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(exc_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if issubclass_bits(exc_class_bits, base_group_bits) {
            let is_match = isinstance_bits(_py, exc_bits, match_bits);
            if is_match {
                return exception_group_make_pair_tuple(
                    _py,
                    Some(ExceptionGroupItem {
                        bits: exc_bits,
                        owned: false,
                    }),
                    None,
                );
            }
            let matcher = ExceptionGroupMatcher::Type(match_bits);
            let Some((match_item, rest_item)) = exception_group_split_node(_py, exc_bits, &matcher)
            else {
                return MoltObject::none().bits();
            };
            return exception_group_make_pair_tuple(_py, match_item, rest_item);
        }
        if !isinstance_bits(_py, exc_bits, match_bits) {
            return exception_group_make_pair_tuple(
                _py,
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            );
        }
        let exc_type_bits = type_of_bits(_py, exc_bits);
        let builtins = builtin_classes(_py);
        let group_class_bits = if issubclass_bits(exc_type_bits, builtins.exception) {
            builtins.exception_group
        } else {
            builtins.base_exception_group
        };
        let tuple_ptr = alloc_tuple(_py, &[exc_bits]);
        if tuple_ptr.is_null() {
            return none_bits;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let msg_ptr = alloc_string(_py, b"");
        if msg_ptr.is_null() {
            dec_ref_bits(_py, tuple_bits);
            return none_bits;
        }
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let group_bits = exception_group_alloc(
            _py,
            group_class_bits,
            msg_bits,
            tuple_bits,
            &[exc_bits],
            Some(tuple_bits),
        );
        dec_ref_bits(_py, msg_bits);
        let Some(bits) = group_bits else {
            return none_bits;
        };
        if let Some(group_ptr) = obj_from_bits(bits).as_ptr() {
            unsafe {
                exception_group_copy_metadata(_py, group_ptr, exc_ptr, false, true, false);
            }
        }
        exception_group_make_pair_tuple(_py, Some(ExceptionGroupItem { bits, owned: true }), None)
    })
}

#[no_mangle]
pub extern "C" fn molt_exceptiongroup_combine(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            let type_id = object_type_id(list_ptr);
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "second argument (exceptions) must be a sequence",
                );
            }
            let elems = seq_vec_ref(list_ptr);
            if elems.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "second argument (exceptions) must be a non-empty sequence",
                );
            }
            let builtins = builtin_classes(_py);
            let mut all_exception = true;
            for (idx, &item_bits) in elems.iter().enumerate() {
                let item_class = type_of_bits(_py, item_bits);
                if !issubclass_bits(item_class, builtins.base_exception) {
                    let msg =
                        format!("Item {idx} of second argument (exceptions) is not an exception");
                    return raise_exception::<u64>(_py, "ValueError", &msg);
                }
                if !issubclass_bits(item_class, builtins.exception) {
                    all_exception = false;
                }
            }
            let group_class = if all_exception {
                builtins.exception_group
            } else {
                builtins.base_exception_group
            };
            let msg_ptr = alloc_string(_py, b"");
            if msg_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let out = exception_group_alloc(_py, group_class, msg_bits, list_bits, elems, None)
                .unwrap_or_else(|| MoltObject::none().bits());
            dec_ref_bits(_py, msg_bits);
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        generator_exception_stack_drop, generator_exception_stack_store,
        generator_exception_stack_take, task_exception_stack_drop, task_exception_stack_store,
        task_exception_stack_take,
    };
    use molt_obj_model::MoltObject;

    #[test]
    fn generator_exception_stack_drop_clears_entries() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
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
        crate::with_gil_entry!(_py, {
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
}

#[no_mangle]
pub extern "C" fn molt_exception_kind(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_exception_class(kind_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_exception_message(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
            let bits = exception_msg_bits(ptr);
            inc_ref_bits(_py, bits);
            bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_set_cause(exc_bits: u64, cause_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_exception_set_value(exc_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_exception_context_set(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        if !exc_obj.is_none() {
            let Some(ptr) = exc_obj.as_ptr() else {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                    return raise_exception::<u64>(_py, "TypeError", "expected exception object");
                }
            }
        }
        exception_context_set(_py, exc_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_set_last(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        if debug_exception_flow() {
            let kind_bits = unsafe { exception_kind_bits(ptr) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            eprintln!("molt exc set_last task=0x{:x} kind={}", task, kind);
        }
        record_exception(_py, ptr);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_last() -> u64 {
    crate::with_gil_entry!(_py, {
        let debug_flow = debug_exception_flow();
        if let Some(task_key) = current_task_key() {
            if let Some(ptr) = task_last_exceptions(_py)
                .lock()
                .unwrap()
                .get(&task_key)
                .copied()
            {
                if debug_flow {
                    let kind_bits = unsafe { exception_kind_bits(ptr.0) };
                    let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                        .unwrap_or_else(|| "<unknown>".to_string());
                    eprintln!(
                        "molt exc last task=0x{:x} kind={}",
                        task_key.0 as usize, kind
                    );
                }
                let bits = MoltObject::from_ptr(ptr.0).bits();
                inc_ref_bits(_py, bits);
                return bits;
            }
        }
        let guard = runtime_state(_py).last_exception.lock().unwrap();
        if let Some(ptr) = *guard {
            if debug_flow {
                let kind_bits = unsafe { exception_kind_bits(ptr.0) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!("molt exc last task=0x0 kind={}", kind);
            }
            let bits = MoltObject::from_ptr(ptr.0).bits();
            inc_ref_bits(_py, bits);
            return bits;
        }
        if debug_flow {
            eprintln!("molt exc last task=0x0 kind=none");
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_active() -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(bits) = exception_context_active_bits() {
            inc_ref_bits(_py, bits);
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_clear() -> u64 {
    crate::with_gil_entry!(_py, {
        let debug_clear = debug_exception_clear();
        let reason = exception_clear_reason_take();
        let cleared_bits = if debug_clear && exception_pending(_py) {
            molt_exception_last()
        } else {
            MoltObject::none().bits()
        };
        if debug_clear && !obj_from_bits(cleared_bits).is_none() {
            if let Some(ptr) = maybe_ptr_from_bits(cleared_bits) {
                let kind_bits = unsafe { exception_kind_bits(ptr) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
                let reason_str = reason.unwrap_or("<unset>");
                eprintln!(
                    "molt exc clear task=0x{:x} kind={} reason={}",
                    task, kind, reason_str
                );
                if reason_str == "<unset>" {
                    eprintln!("molt exc clear backtrace (reason unset):");
                    eprintln!("{:?}", Backtrace::force_capture());
                }
                let frame = FRAME_STACK.with(|stack| stack.borrow().last().copied());
                if let Some(frame) = frame {
                    if let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits) {
                        let (name_bits, file_bits) =
                            unsafe { (code_name_bits(code_ptr), code_filename_bits(code_ptr)) };
                        let name = string_obj_to_owned(obj_from_bits(name_bits))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        let file = string_obj_to_owned(obj_from_bits(file_bits))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, file_bits);
                        eprintln!(
                            "molt exc clear frame name={} file={} line={}",
                            name, file, frame.line
                        );
                    }
                }
                if kind == "GeneratorExit" {
                    let task_ptr = current_task_ptr();
                    if !task_ptr.is_null() {
                        let (poll_fn, type_id, class_name) = unsafe {
                            let header = header_from_obj_ptr(task_ptr);
                            let poll_fn = (*header).poll_fn;
                            let type_id = object_type_id(task_ptr);
                            let class_name = class_name_for_error(object_class_bits(task_ptr));
                            (poll_fn, type_id, class_name)
                        };
                        eprintln!(
                            "molt exc clear ctx task=0x{:x} poll=0x{:x} type_id={} class={}",
                            task_ptr as usize, poll_fn, type_id, class_name
                        );
                    } else {
                        eprintln!("molt exc clear ctx task=none");
                    }
                    eprintln!("molt exc clear backtrace (GeneratorExit):");
                    eprintln!("{:?}", Backtrace::force_capture());
                }
            }
        }
        clear_exception(_py);
        if debug_clear && !obj_from_bits(cleared_bits).is_none() {
            dec_ref_bits(_py, cleared_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_pending() -> u64 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            1
        } else {
            0
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_pending_fast() -> u64 {
    let Some(state) = crate::state::runtime_state::runtime_state_for_gil() else {
        return 0;
    };
    if let Some(task_key) = current_task_key() {
        if state
            .task_last_exception_pending
            .load(AtomicOrdering::Relaxed)
            && state
                .task_last_exceptions
                .lock()
                .unwrap()
                .contains_key(&task_key)
        {
            return 1;
        }
    }
    if state.last_exception_pending.load(AtomicOrdering::Relaxed) {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_stack_enter() -> u64 {
    crate::with_gil_entry!(_py, {
        let prev = exception_stack_baseline_get();
        let depth = exception_stack_depth();
        exception_stack_baseline_set(depth);
        int_bits_from_i64(_py, prev as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_stack_exit(prev_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let prev = match to_i64(obj_from_bits(prev_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception baseline must be a non-negative int",
                )
            }
        };
        exception_stack_baseline_set(prev);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_stack_depth() -> u64 {
    crate::with_gil_entry!(_py, {
        int_bits_from_i64(_py, exception_stack_depth() as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_stack_set_depth(depth_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let depth = match to_i64(obj_from_bits(depth_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception depth must be a non-negative int",
                )
            }
        };
        exception_stack_set_depth(_py, depth);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_push() -> u64 {
    crate::with_gil_entry!(_py, {
        exception_stack_push();
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_pop() -> u64 {
    crate::with_gil_entry!(_py, {
        exception_stack_pop(_py);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_exception_stack_clear() -> u64 {
    crate::with_gil_entry!(_py, {
        exception_stack_set_depth(_py, 0);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_raise(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        let Some(ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        };
        let mut exc_ptr = ptr;
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_EXCEPTION => {}
                TYPE_ID_TYPE => {
                    let class_bits = MoltObject::from_ptr(ptr).bits();
                    if !issubclass_bits(class_bits, builtin_classes(_py).base_exception) {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "exceptions must derive from BaseException",
                        );
                    }
                    let inst_bits = call_class_init_with_args(_py, ptr, &[]);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(inst_ptr) != TYPE_ID_EXCEPTION {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "exceptions must derive from BaseException",
                        );
                    }
                    exc_ptr = inst_ptr;
                }
                _ => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "exceptions must derive from BaseException",
                    );
                }
            }
        }
        if debug_exception_flow() || debug_exception_raise() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            let depth = exception_stack_depth();
            eprintln!(
                "molt exc raise task=0x{:x} kind={} handler_active={} depth={} task_raise_active={}",
                task,
                kind,
                exception_handler_active(),
                depth,
                task_raise_active()
            );
        }
        record_exception(_py, exc_ptr);
        if !exception_handler_active() && !generator_raise_active() && !task_raise_active() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("SystemExit") {
                handle_system_exit(_py, exc_ptr);
            }
            context_stack_unwind(_py, MoltObject::from_ptr(exc_ptr).bits());
            let formatted = format_exception_with_traceback(_py, exc_ptr);
            eprintln!("{}", formatted);
            if let Ok(path) = std::env::var("MOLT_EXCEPTION_LOG_PATH") {
                let _ = std::fs::write(path, formatted.as_bytes());
            }
            std::process::exit(1);
        }
        MoltObject::none().bits()
    })
}
