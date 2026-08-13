use crate::PyToken;
use crate::builtins::frames::{
    frame_stack_top_info, frame_stack_trace_payload_bits, traceback_payload_is_lazy,
};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::heap_lifecycle::DetachedEdgeSink;
use crate::object::{
    ClassEdgeOwnership, ObjectAuxPreselection, alloc_object_with_aux,
    object_init_class_edge_unpublished,
};
use crate::{
    FRAME_STACK, HEADER_FLAG_TRACEBACK_SUPPRESSED, MoltHeader, PtrSlot, RuntimeState,
    TRACEBACK_SUPPRESS_COUNT, TYPE_ID_CODE, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_bytes, alloc_class_obj,
    alloc_list, alloc_string, alloc_tuple, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes, builtin_classes, builtin_func_bits, bytes_like_slice,
    call_callable1, call_class_init_with_args, class_break_cycles, class_dict_bits,
    class_name_bits, class_name_for_error, code_filename_bits, code_name_bits,
    context_stack_unwind, current_task_key, current_task_ptr, current_token_id, dec_ref_bits,
    dict_find_entry_fast, dict_get_in_place, dict_hashes, dict_order, dict_set_in_place,
    dict_table, format_obj, format_obj_str, header_from_obj_ptr, inc_ref_bits,
    index_bigint_from_obj, init_atomic_bits, instance_dict_bits, int_bits_from_i64,
    intern_static_name, is_truthy, isinstance_bits, issubclass_bits, maybe_ptr_from_bits,
    module_dict_bits, molt_class_set_base, molt_dec_ref, molt_index, molt_is_callable,
    molt_iter_checked, molt_iter_next, molt_repr_from_obj, molt_str_from_obj, obj_from_bits,
    object_class_bits, object_type_id, profile_enabled, runtime_state, string_bytes, string_len,
    string_obj_to_owned, task_exception_depths, task_exception_handler_stacks,
    task_exception_stacks, task_last_exceptions, to_i64, token_is_cancelled, traceback_suppressed,
    tuple_from_iter_bits, type_name, type_of_bits,
};
use molt_obj_model::{
    ExceptionBaseSpec, ExceptionFieldStorage, ExceptionLayoutKind, ExceptionLayoutRoot,
    ExceptionMissingRead, ExceptionTypedField, MAX_EXCEPTION_TYPED_FIELDS,
    MAX_EXCEPTION_TYPED_TAIL_WORDS, MoltObject, builtin_exception_spec,
};
use num_traits::ToPrimitive;
use std::backtrace::Backtrace;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Common payload prefix shared by every exception.  Typed builtin state starts
/// immediately after this prefix and is sized by `ExceptionLayoutKind::tail_policies`.
/// Slot 9 is an immutable, non-reference layout tag.
pub(crate) const EXCEPTION_FIXED_WORDS: usize = 10;
const EXCEPTION_MSG_SLOT: usize = 0;
const EXCEPTION_CAUSE_SLOT: usize = 1;
const EXCEPTION_CONTEXT_SLOT: usize = 2;
const EXCEPTION_SUPPRESS_SLOT: usize = 3;
const EXCEPTION_TRACE_SLOT: usize = 4;
const EXCEPTION_ARGS_SLOT: usize = 5;
const EXCEPTION_DICT_SLOT: usize = 6;
const EXCEPTION_ARGS_PAYLOAD_SLOT: usize = 7;
const EXCEPTION_NOTES_SLOT: usize = 8;
const EXCEPTION_LAYOUT_KIND_SLOT: usize = 9;
const EXCEPTION_TYPED_TAIL_SLOT: usize = 10;
const EXCEPTION_OWNED_EDGE_SLOTS: [usize; 8] = [
    EXCEPTION_MSG_SLOT,
    EXCEPTION_DICT_SLOT,
    EXCEPTION_ARGS_SLOT,
    EXCEPTION_ARGS_PAYLOAD_SLOT,
    EXCEPTION_NOTES_SLOT,
    EXCEPTION_TRACE_SLOT,
    EXCEPTION_CAUSE_SLOT,
    EXCEPTION_CONTEXT_SLOT,
];

// Keep detachment allocation-free so cycle collection and failure cleanup
// remain valid under memory pressure. The schema owns the maximum tail width.
const MAX_EXCEPTION_OWNED_EDGES: usize =
    EXCEPTION_OWNED_EDGE_SLOTS.len() + MAX_EXCEPTION_TYPED_TAIL_WORDS;
const PY_SSIZE_MISSING: u64 = (-1isize) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DetachedExceptionEdges {
    bits: [u64; MAX_EXCEPTION_OWNED_EDGES],
    len: usize,
}

mod exception_group;
pub(crate) use exception_group::{
    alloc_exception_group_from_class_bits, exception_group_exceptions_bits,
    exception_group_message_bits,
};

mod exception_payload;
mod unraisable;
pub(crate) use exception_group::{
    molt_exceptiongroup_derive, molt_exceptiongroup_init, molt_exceptiongroup_split,
    molt_exceptiongroup_subgroup,
};
pub(crate) use exception_payload::{
    alloc_exception_from_class_bits, format_exception, format_exception_message,
    format_exception_with_traceback, raise_os_error, raise_os_error_errno,
};
use exception_payload::{oserror_fields_from_args, unicode_error_fields_from_args};
pub(crate) use unraisable::{
    context_repr as unraisable_context_repr, molt_unraisable_hook_args_is_exact,
    report_captured_unraisable, run_unraisable, run_unraisable_with_policy,
};

mod exception_state_abi;
#[cfg(test)]
use exception_state_abi::{exception_last_pending_bits, exception_last_public_bits};
pub(crate) use exception_state_abi::{
    molt_async_work_poll_and_exception_pending, molt_exception_active, molt_exception_clear,
    molt_exception_last, molt_exception_last_pending, molt_exception_pending, molt_raise,
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

mod state;

use internals::{exception_type_cache, module_cache};
pub(crate) use state::{
    ACTIVE_EXCEPTION_FALLBACK, ACTIVE_EXCEPTION_STACK, CURRENT_EXCEPTION_PENDING, EXCEPTION_STACK,
    EXCEPTION_STACK_BASELINE, ExceptionContextFallback, ExceptionsRuntimeState,
    GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, TASK_RAISE_ACTIVE, THREAD_LAST_EXCEPTION,
    exception_clear_reason_set, internals,
};
use state::{
    LAST_EXCEPTION_COL, STOPASYNC_BT_PRINTED, debug_exception_clear, debug_exception_flow,
    debug_exception_pending, debug_exception_raise, debug_exception_rc, debug_exceptions,
    debug_oom, exception_clear_reason_take, exceptions_state, trace_exception_stack,
};

pub(crate) fn exception_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__init__" => {
            exception_method_bits_for_owner(_py, builtin_classes(_py).base_exception, name)
        }
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

/// Materialize the builtin ``__init__`` descriptor for its declaring exception
/// class. The callable closure is the descriptor-owner authority; sharing one
/// unowned function object across the exception hierarchy loses CPython's
/// receiver checks and lets subclass layout leak into base-class keyword rules.
pub(crate) fn exception_method_bits_for_owner(
    _py: &PyToken<'_>,
    owner_bits: u64,
    name: &str,
) -> Option<u64> {
    if name != "__init__" {
        return exception_method_bits(_py, name);
    }
    let owner_ptr = obj_from_bits(owner_bits).as_ptr()?;
    let root = if owner_bits == builtin_classes(_py).base_exception {
        ExceptionLayoutRoot::Base
    } else {
        let root = unsafe { crate::object::class_exception_layout_root(owner_ptr) };
        if root == ExceptionLayoutRoot::Base
            || owner_bits != exception_type_bits_from_name(_py, root.owner_name())
        {
            return None;
        }
        root
    };
    let cache = &runtime_state(_py).method_cache;
    let slot = match root {
        ExceptionLayoutRoot::Base => &cache.exception_init,
        ExceptionLayoutRoot::BaseExceptionGroup => &cache.exception_init_base_exception_group,
        ExceptionLayoutRoot::SyntaxError => &cache.exception_init_syntax_error,
        ExceptionLayoutRoot::ImportError => &cache.exception_init_import_error,
        ExceptionLayoutRoot::UnicodeDecodeError => &cache.exception_init_unicode_decode_error,
        ExceptionLayoutRoot::UnicodeEncodeError => &cache.exception_init_unicode_encode_error,
        ExceptionLayoutRoot::UnicodeTranslateError => &cache.exception_init_unicode_translate_error,
        ExceptionLayoutRoot::SystemExit => &cache.exception_init_system_exit,
        ExceptionLayoutRoot::OSError => &cache.exception_init_os_error,
        ExceptionLayoutRoot::StopIteration => &cache.exception_init_stop_iteration,
        ExceptionLayoutRoot::NameError => &cache.exception_init_name_error,
        ExceptionLayoutRoot::AttributeError => &cache.exception_init_attribute_error,
    };
    Some(init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            fn_addr!(molt_exception_init_owned),
            4,
        );
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
            crate::function_set_closure_bits(
                _py,
                ptr,
                MoltObject::from_int(i64::from(root as u8)).bits(),
            );
            let vararg_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_vararg,
                b"__molt_vararg__",
            );
            crate::function_set_attr_bits(
                _py,
                ptr,
                vararg_name,
                MoltObject::from_bool(true).bits(),
            );
            crate::call::bind::refresh_function_requires_binder_flag(_py, ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            if !object_init_class_edge_unpublished(
                _py,
                ptr,
                builtin_bits,
                ClassEdgeOwnership::Owned,
            ) {
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_IMMORTAL);
                dec_ref_bits(_py, bits);
                return MoltObject::none().bits();
            }
        }
        bits
    }))
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
    } else {
        // CPython keeps a preallocated MemoryError for precisely this case.
        // Molt's heap exception representation cannot be constructed without
        // allocation, so retain the same semantic fact in the canonical
        // exception state without allocating or recursively bootstrapping the
        // runtime. A later successful raise replaces this marker normally.
        record_emergency_memory_error(_py);
    }
    T::exception_sentinel()
}

#[inline]
fn emergency_memory_error_pending_for_current() -> bool {
    THREAD_LAST_EXCEPTION.with(|state| state.emergency_memory_error_is_pending(current_task_ptr()))
}

#[inline]
fn record_emergency_memory_error(_py: &PyToken<'_>) {
    if emergency_memory_error_pending_for_current()
        || current_task_key().map_or_else(
            || thread_last_exception_raw_slot().is_some(),
            |task_key| {
                task_last_exceptions(_py)
                    .lock()
                    .unwrap()
                    .contains_key(&task_key)
            },
        )
    {
        return;
    }
    THREAD_LAST_EXCEPTION.with(|state| state.set_emergency_memory_error(current_task_ptr()));
    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
}

/// Record MemoryError without touching the heap, runtime class bootstrap, or
/// the resource tracker. Raw ABI allocation failures use this entrypoint
/// because attempting to allocate their exception can recursively fail before
/// the runtime is warm.
#[inline]
pub(crate) fn record_memory_error_without_allocation(_py: &PyToken<'_>) {
    record_emergency_memory_error(_py);
}

#[inline]
fn clear_emergency_memory_error_for_current() {
    THREAD_LAST_EXCEPTION.with(|state| state.clear_emergency_memory_error(current_task_ptr()));
}

#[inline]
fn clear_all_emergency_memory_error_state() {
    let _ = THREAD_LAST_EXCEPTION
        .try_with(state::ThreadExceptionState::clear_all_emergency_memory_error);
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
    let args_ptr = alloc_tuple(_py, &[key_bits]);
    if args_ptr.is_null() {
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let msg_bits = molt_repr_from_obj(key_bits);
    if obj_from_bits(msg_bits).is_none() {
        dec_ref_bits(_py, args_bits);
        return T::exception_sentinel();
    }
    let class_bits = exception_type_bits_from_name(_py, "KeyError");
    let none_bits = MoltObject::none().bits();
    let ptr = alloc_exception_obj(_py, class_bits, msg_bits, args_bits, none_bits);
    if ptr.is_null() {
        dec_ref_bits(_py, msg_bits);
        dec_ref_bits(_py, args_bits);
        return T::exception_sentinel();
    }
    record_exception_owned(_py, ptr);
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

pub(crate) fn system_exit_code(_py: &PyToken<'_>, ptr: *mut u8) -> i32 {
    let code_bits = unsafe {
        exception_typed_field_raw_bits(ptr, ExceptionTypedField::SystemExitCode)
            .unwrap_or_else(|| MoltObject::none().bits())
    };
    let code_obj = obj_from_bits(code_bits);
    if code_obj.is_none() {
        return 0;
    }
    if isinstance_bits(_py, code_bits, builtin_classes(_py).int) {
        // CPython first converts to a signed platform long. Values that fit
        // that carrier reach the process boundary, where only the low C-int
        // status bits survive; a conversion overflow is cleared and becomes
        // the conventional all-bits-set failure status without presentation.
        return to_i64(code_obj).map_or(-1, |code| code as i32);
    }
    let message = format_obj(_py, code_obj);
    eprintln!("{message}");
    1
}

pub(crate) fn alloc_exception(_py: &PyToken<'_>, kind: &str, message: &str) -> *mut u8 {
    let class_bits = exception_type_bits_from_name(_py, kind);
    if class_bits == 0 {
        return std::ptr::null_mut();
    }
    let msg_ptr = alloc_string(_py, message.as_bytes());
    if msg_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let args_ptr = if message.is_empty() {
        alloc_tuple(_py, &[])
    } else {
        alloc_tuple(_py, &[msg_bits])
    };
    if args_ptr.is_null() {
        unsafe { molt_dec_ref(msg_ptr) };
        return std::ptr::null_mut();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let none_bits = MoltObject::none().bits();
    let mut ptr = alloc_exception_obj(_py, class_bits, msg_bits, args_bits, none_bits);
    if !ptr.is_null()
        && unsafe { exception_initialize_typed_fields_from_args(_py, ptr, args_bits) }.is_err()
    {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        ptr = std::ptr::null_mut();
    }
    dec_ref_bits(_py, msg_bits);
    dec_ref_bits(_py, args_bits);
    ptr
}

pub(crate) fn alloc_exception_obj(
    _py: &PyToken<'_>,
    class_bits: u64,
    msg_bits: u64,
    args_bits: u64,
    dict_bits: u64,
) -> *mut u8 {
    alloc_exception_obj_with_args_payload(
        _py,
        class_bits,
        msg_bits,
        args_bits,
        dict_bits,
        MoltObject::none().bits(),
    )
}

fn alloc_exception_obj_with_args_payload(
    _py: &PyToken<'_>,
    class_bits: u64,
    msg_bits: u64,
    args_bits: u64,
    dict_bits: u64,
    args_payload_bits: u64,
) -> *mut u8 {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return std::ptr::null_mut();
    }
    let layout = exception_layout_kind_for_class(_py, class_bits);
    let payload_words = EXCEPTION_FIXED_WORDS + layout.tail_word_count();
    let Some(total) = payload_words
        .checked_mul(std::mem::size_of::<u64>())
        .and_then(|payload| payload.checked_add(std::mem::size_of::<MoltHeader>()))
    else {
        return std::ptr::null_mut();
    };
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_EXCEPTION,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        return ptr;
    }
    let none = MoltObject::none().bits();
    let common_edges = [
        msg_bits,
        none,
        none,
        MoltObject::from_bool(false).bits(),
        none,
        args_bits,
        dict_bits,
        args_payload_bits,
        none,
    ];
    unsafe {
        for (offset, bits) in common_edges.into_iter().enumerate() {
            *(ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64) = bits;
            inc_ref_bits(_py, bits);
        }
        *(ptr.add(EXCEPTION_LAYOUT_KIND_SLOT * std::mem::size_of::<u64>()) as *mut u64) =
            layout as u8 as u64;
        for (index, policy) in layout.tail_policies().iter().enumerate() {
            let raw = typed_field_default_raw(policy.field);
            *(ptr.add((EXCEPTION_TYPED_TAIL_SLOT + index) * std::mem::size_of::<u64>())
                as *mut u64) = raw;
            if policy.storage == ExceptionFieldStorage::Object {
                inc_ref_bits(_py, raw);
            }
        }
        if !object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned) {
            for (offset, bits) in common_edges.into_iter().enumerate() {
                *(ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64) = 0;
                dec_ref_bits(_py, bits);
            }
            for (index, policy) in layout.tail_policies().iter().enumerate() {
                let slot = ptr.add((EXCEPTION_TYPED_TAIL_SLOT + index) * std::mem::size_of::<u64>())
                    as *mut u64;
                let raw = *slot;
                *slot = 0;
                if policy.storage == ExceptionFieldStorage::Object {
                    dec_ref_bits(_py, raw);
                }
            }
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        }
    }
    ptr
}

pub(crate) fn exception_layout_kind_for_class(
    _py: &PyToken<'_>,
    class_bits: u64,
) -> ExceptionLayoutKind {
    exception_layout_root_for_class(_py, class_bits)
        .map(ExceptionLayoutRoot::kind)
        .unwrap_or(ExceptionLayoutKind::Base)
}

/// Resolve the concrete builtin root that owns an exception instance layout.
/// Root identity is stricter than family kind: the three Unicode concrete
/// roots deliberately remain distinct so class construction can reject
/// incompatible multiple inheritance without hard-coded name switches.
pub(crate) fn exception_layout_root_for_class(
    _py: &PyToken<'_>,
    class_bits: u64,
) -> Option<ExceptionLayoutRoot> {
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return None;
    }
    let root = unsafe { crate::object::class_exception_layout_root(class_ptr) };
    (root != ExceptionLayoutRoot::Base).then_some(root)
}

#[inline]
pub(crate) unsafe fn exception_layout_kind(ptr: *mut u8) -> ExceptionLayoutKind {
    let raw = unsafe {
        *(ptr.add(EXCEPTION_LAYOUT_KIND_SLOT * std::mem::size_of::<u64>()) as *const u64)
    };
    ExceptionLayoutKind::from_u8(raw as u8).unwrap_or(ExceptionLayoutKind::Base)
}

/// Allocation-free ABI hook primitive for both exception instances and their
/// runtime class identities. Invalid/non-exception handles return `u8::MAX`;
/// plain BaseException-family classes return the real base layout (zero).
pub(crate) unsafe fn exception_layout_kind_from_bits(
    _py: &PyToken<'_>,
    exception_or_class_bits: u64,
) -> u8 {
    let Some(ptr) = obj_from_bits(exception_or_class_bits).as_ptr() else {
        return u8::MAX;
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_EXCEPTION => (unsafe { exception_layout_kind(ptr) }) as u8,
        TYPE_ID_TYPE
            if issubclass_bits(exception_or_class_bits, builtin_classes(_py).base_exception) =>
        {
            unsafe { crate::object::class_exception_layout_root(ptr) }.kind() as u8
        }
        _ => u8::MAX,
    }
}

#[inline]
pub(crate) unsafe fn exception_payload_words(ptr: *mut u8) -> usize {
    EXCEPTION_FIXED_WORDS + unsafe { exception_layout_kind(ptr) }.tail_word_count()
}

#[inline]
fn typed_field_default_raw(field: ExceptionTypedField) -> u64 {
    match field {
        ExceptionTypedField::OSErrorCharactersWritten => PY_SSIZE_MISSING,
        ExceptionTypedField::UnicodeStart | ExceptionTypedField::UnicodeEnd => 0,
        _ => MoltObject::none().bits(),
    }
}

#[inline]
fn exception_typed_tail_slot(
    layout: ExceptionLayoutKind,
    field: ExceptionTypedField,
) -> Option<usize> {
    layout
        .tail_policies()
        .iter()
        .position(|policy| policy.field == field)
        .map(|index| EXCEPTION_TYPED_TAIL_SLOT + index)
}

#[inline]
pub(crate) unsafe fn exception_typed_field_raw_bits(
    ptr: *mut u8,
    field: ExceptionTypedField,
) -> Option<u64> {
    let layout = unsafe { exception_layout_kind(ptr) };
    let policy = layout.field_policy(field)?;
    let offset = if policy.storage == ExceptionFieldStorage::RuntimeMessage {
        EXCEPTION_MSG_SLOT
    } else {
        exception_typed_tail_slot(layout, field)?
    };
    Some(unsafe { *(ptr.add(offset * std::mem::size_of::<u64>()) as *const u64) })
}

pub(crate) unsafe fn exception_kind_bits(ptr: *mut u8) -> u64 {
    unsafe {
        obj_from_bits(object_class_bits(ptr))
            .as_ptr()
            .map(|class_ptr| class_name_bits(class_ptr))
            .unwrap_or(0)
    }
}

pub(crate) unsafe fn exception_msg_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_MSG_SLOT * std::mem::size_of::<u64>()) as *const u64) }
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

fn exception_should_defer_message(_py: &PyToken<'_>, class_bits: u64) -> bool {
    if matches!(
        exception_layout_kind_for_class(_py, class_bits),
        ExceptionLayoutKind::Group | ExceptionLayoutKind::Syntax | ExceptionLayoutKind::Import
    ) {
        return false;
    }
    unsafe { crate::object::ops_format::exception_class_uses_cached_message_str(_py, class_bits) }
}

pub(crate) fn exception_message_for_storage(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> u64 {
    let layout = exception_layout_kind_for_class(_py, class_bits);
    if matches!(
        layout,
        ExceptionLayoutKind::Syntax | ExceptionLayoutKind::Import
    ) {
        let none = MoltObject::none().bits();
        let mut message = none;
        if let Some(args_ptr) = obj_from_bits(args_bits).as_ptr()
            && unsafe { object_type_id(args_ptr) } == TYPE_ID_TUPLE
        {
            message = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                    if layout == ExceptionLayoutKind::Syntax || args.len() == 1 {
                        args.first().copied().unwrap_or(none)
                    } else {
                        none
                    }
                })
                .unwrap_or(none)
            };
        }
        inc_ref_bits(_py, message);
        return message;
    }
    if exception_should_defer_message(_py, class_bits) {
        exception_lazy_message_bits()
    } else {
        exception_message_from_args(_py, args_bits)
    }
}

#[inline]
pub(super) fn exception_message_storage_is_valid(
    _py: &PyToken<'_>,
    class_bits: u64,
    message_bits: u64,
) -> bool {
    !obj_from_bits(message_bits).is_none()
        || matches!(
            exception_layout_kind_for_class(_py, class_bits),
            ExceptionLayoutKind::Syntax | ExceptionLayoutKind::Import
        )
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
    if unsafe { !exception_publish_owned_slot(_py, ptr, EXCEPTION_MSG_SLOT, msg_bits) } {
        return MoltObject::none().bits();
    }
    msg_bits
}

pub(crate) unsafe fn exception_cause_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_CAUSE_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_context_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_CONTEXT_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_suppress_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_SUPPRESS_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_trace_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_TRACE_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) fn exception_match_class_bits(_py: &PyToken<'_>, exc_bits: u64) -> u64 {
    let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return type_of_bits(_py, exc_bits);
        }
        object_class_bits(exc_ptr)
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
    target_bits != 0 && exception_matches_type(_py, exc_bits, target_bits)
}

pub(crate) unsafe fn exception_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_ARGS_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

#[derive(Clone, Copy)]
pub(crate) enum ExceptionFieldSlot {
    Cause,
    Context,
    Traceback,
    Args,
    Dict,
    Notes,
}

impl ExceptionFieldSlot {
    const fn offset(self) -> usize {
        match self {
            Self::Cause => EXCEPTION_CAUSE_SLOT,
            Self::Context => EXCEPTION_CONTEXT_SLOT,
            Self::Traceback => EXCEPTION_TRACE_SLOT,
            Self::Args => EXCEPTION_ARGS_SLOT,
            Self::Dict => EXCEPTION_DICT_SLOT,
            Self::Notes => EXCEPTION_NOTES_SLOT,
        }
    }
}

unsafe fn exception_publish_borrowed_slot(
    _py: &PyToken<'_>,
    exception_ptr: *mut u8,
    offset: usize,
    value_bits: u64,
) -> bool {
    unsafe {
        let slot = exception_ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits == value_bits {
            return true;
        }
        inc_ref_bits(_py, value_bits);
        *slot = value_bits;
        let header = crate::header_from_obj_ptr(exception_ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .refresh_exception_view(MoltObject::from_ptr(exception_ptr).bits());
        if !pushed {
            *slot = old_bits;
            dec_ref_bits(_py, value_bits);
            return false;
        }
        dec_ref_bits(_py, old_bits);
        true
    }
}

/// Publish a newly owned reference without manufacturing a second temporary
/// owner. Failure consumes the incoming ownership and restores the old slot.
unsafe fn exception_publish_owned_slot(
    _py: &PyToken<'_>,
    exception_ptr: *mut u8,
    offset: usize,
    value_bits: u64,
) -> bool {
    unsafe {
        let slot = exception_ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits == value_bits {
            dec_ref_bits(_py, value_bits);
            return true;
        }
        *slot = value_bits;
        let header = crate::header_from_obj_ptr(exception_ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .refresh_exception_view(MoltObject::from_ptr(exception_ptr).bits());
        if !pushed {
            *slot = old_bits;
            dec_ref_bits(_py, value_bits);
            return false;
        }
        dec_ref_bits(_py, old_bits);
        true
    }
}

/// The only post-construction writer for exception reference fields. Publish
/// the new owned edge before releasing the old edge so self/cyclic assignment
/// cannot transiently destroy the object being installed.
pub(crate) unsafe fn exception_publish_field_slot(
    _py: &PyToken<'_>,
    exception_ptr: *mut u8,
    field: ExceptionFieldSlot,
    value_bits: u64,
) -> bool {
    unsafe { exception_publish_borrowed_slot(_py, exception_ptr, field.offset(), value_bits) }
}

fn exception_typed_field_policy_by_name(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    name: &str,
) -> Option<&'static molt_obj_model::ExceptionFieldPolicy> {
    let layout = unsafe { exception_layout_kind(ptr) };
    let policy = layout
        .field_policies()
        .iter()
        .find(|policy| policy.python_name == name)?;
    let class_bits = unsafe { object_class_bits(ptr) };
    let root = exception_layout_root_for_class(_py, class_bits)?;
    let owner_bits = exception_type_bits_from_name(_py, root.owner_name());
    (owner_bits != 0 && class_bits != 0 && issubclass_bits(class_bits, owner_bits))
        .then_some(policy)
}

#[derive(Clone, Copy)]
struct PreparedTypedFieldUpdate {
    offset: usize,
    storage: ExceptionFieldStorage,
    old_raw: u64,
    new_raw: u64,
}

fn typed_py_ssize_raw_from_bits(
    _py: &PyToken<'_>,
    field: ExceptionTypedField,
    value_bits: u64,
) -> Result<u64, &'static str> {
    let type_error = if matches!(
        field,
        ExceptionTypedField::UnicodeStart | ExceptionTypedField::UnicodeEnd
    ) {
        "an integer is required".to_string()
    } else {
        format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(value_bits))
        )
    };
    let Some(value) = index_bigint_from_obj(_py, value_bits, &type_error) else {
        return Err("typed exception integer conversion failed");
    };
    let Some(value) = value.to_isize() else {
        if matches!(
            field,
            ExceptionTypedField::UnicodeStart | ExceptionTypedField::UnicodeEnd
        ) {
            let _ = raise_exception::<u64>(
                _py,
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            );
        } else {
            let value_type = class_name_for_error(type_of_bits(_py, value_bits));
            let _ = raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("cannot fit '{value_type}' into an index-sized integer"),
            );
        }
        return Err("typed exception integer conversion failed");
    };
    Ok(value as u64)
}

fn prepare_typed_field_update(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    field: ExceptionTypedField,
    value_bits: u64,
) -> Result<PreparedTypedFieldUpdate, &'static str> {
    let layout = unsafe { exception_layout_kind(ptr) };
    let Some(policy) = layout.field_policy(field) else {
        return Err("typed field does not belong to exception layout");
    };
    let offset = if policy.storage == ExceptionFieldStorage::RuntimeMessage {
        EXCEPTION_MSG_SLOT
    } else {
        exception_typed_tail_slot(layout, field).ok_or("typed field has no runtime storage slot")?
    };
    let old_raw = unsafe { *(ptr.add(offset * std::mem::size_of::<u64>()) as *const u64) };
    let new_raw = match policy.storage {
        ExceptionFieldStorage::RuntimeMessage | ExceptionFieldStorage::Object => value_bits,
        ExceptionFieldStorage::PySsize => typed_py_ssize_raw_from_bits(_py, field, value_bits)?,
    };
    Ok(PreparedTypedFieldUpdate {
        offset,
        storage: policy.storage,
        old_raw,
        new_raw,
    })
}

/// Atomically publish a typed-field batch. Inputs are borrowed. All conversion
/// and reference pinning completes before the first word changes; the ABI view
/// observes either the entire old snapshot or the entire new snapshot.
///
/// The transaction uses the same canonical runtime mutation critical section
/// as common exception fields (`PyToken`/GIL today, the runtime-wide lock lane
/// under `free-threaded`). It deliberately does not introduce a second lock
/// registry that could invert ordering with object destruction or ABI refresh.
pub(crate) fn exception_typed_fields_replace_internal(
    _py: &PyToken<'_>,
    exception_bits: u64,
    fields: &[(ExceptionTypedField, u64)],
) -> Result<(), &'static str> {
    crate::gil_assert();
    let Some(exception_ptr) = obj_from_bits(exception_bits).as_ptr() else {
        return Err("expected exception object");
    };
    if unsafe { object_type_id(exception_ptr) } != TYPE_ID_EXCEPTION {
        return Err("expected exception object");
    }
    let mut updates: [Option<PreparedTypedFieldUpdate>; MAX_EXCEPTION_TYPED_FIELDS] =
        [None; MAX_EXCEPTION_TYPED_FIELDS];
    if fields.len() > updates.len() {
        return Err("typed exception update exceeds schema field bound");
    }
    let mut update_len = 0usize;
    for &(field, value_bits) in fields {
        let update = prepare_typed_field_update(_py, exception_ptr, field, value_bits)?;
        if updates[..update_len]
            .iter()
            .flatten()
            .any(|existing| existing.offset == update.offset)
        {
            return Err("duplicate typed exception field update");
        }
        updates[update_len] = Some(update);
        update_len += 1;
    }
    let updates = &updates[..update_len];
    unsafe {
        for update in updates.iter().flatten() {
            if update.storage != ExceptionFieldStorage::PySsize && update.old_raw != update.new_raw
            {
                inc_ref_bits(_py, update.new_raw);
            }
        }
        for update in updates.iter().flatten() {
            if update.old_raw != update.new_raw {
                *(exception_ptr.add(update.offset * std::mem::size_of::<u64>()) as *mut u64) =
                    update.new_raw;
            }
        }
        let header = crate::header_from_obj_ptr(exception_ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE.refresh_exception_view(exception_bits);
        if !pushed {
            for update in updates.iter().flatten() {
                if update.old_raw != update.new_raw {
                    *(exception_ptr.add(update.offset * std::mem::size_of::<u64>()) as *mut u64) =
                        update.old_raw;
                }
            }
            for update in updates.iter().flatten() {
                if update.storage != ExceptionFieldStorage::PySsize
                    && update.old_raw != update.new_raw
                {
                    dec_ref_bits(_py, update.new_raw);
                }
            }
            return Err("exception ABI sidecar synchronization failed");
        }
        for update in updates.iter().flatten() {
            if update.storage != ExceptionFieldStorage::PySsize && update.old_raw != update.new_raw
            {
                dec_ref_bits(_py, update.old_raw);
            }
        }
    }
    Ok(())
}

pub(crate) fn exception_typed_field_replace_internal(
    _py: &PyToken<'_>,
    exception_bits: u64,
    field: ExceptionTypedField,
    value_bits: u64,
) -> Result<(), &'static str> {
    exception_typed_fields_replace_internal(_py, exception_bits, &[(field, value_bits)])
}

/// Return an owned Python reference for a CPython-visible typed descriptor.
/// `None` means this class has no such descriptor; `Some(Err)` means the
/// descriptor exists but its scalar sentinel represents a missing value.
pub(crate) fn exception_typed_field_get(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    name: &str,
) -> Option<Result<u64, &'static str>> {
    let policy = exception_typed_field_policy_by_name(_py, ptr, name)?;
    let layout = unsafe { exception_layout_kind(ptr) };
    let raw = match policy.storage {
        ExceptionFieldStorage::RuntimeMessage => exception_materialized_message_bits(_py, ptr),
        ExceptionFieldStorage::Object | ExceptionFieldStorage::PySsize => {
            let offset = exception_typed_tail_slot(layout, policy.field)?;
            unsafe { *(ptr.add(offset * std::mem::size_of::<u64>()) as *const u64) }
        }
    };
    if policy.storage == ExceptionFieldStorage::PySsize {
        if policy.missing_read == ExceptionMissingRead::AttributeError && raw == PY_SSIZE_MISSING {
            return Some(Err(policy.python_name));
        }
        return Some(Ok(int_bits_from_i64(_py, raw as isize as i64)));
    }
    inc_ref_bits(_py, raw);
    Some(Ok(raw))
}

pub(crate) fn exception_typed_field_replace(
    _py: &PyToken<'_>,
    exception_bits: u64,
    name: &str,
    value_bits: u64,
) -> Option<Result<(), &'static str>> {
    let ptr = obj_from_bits(exception_bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_EXCEPTION {
        return None;
    }
    let policy = exception_typed_field_policy_by_name(_py, ptr, name)?;
    if !policy.writable {
        return Some(Err("readonly attribute"));
    }
    Some(exception_typed_field_replace_internal(
        _py,
        exception_bits,
        policy.field,
        value_bits,
    ))
}

pub(crate) fn exception_typed_field_delete(
    _py: &PyToken<'_>,
    exception_bits: u64,
    name: &str,
) -> Option<Result<(), &'static str>> {
    let ptr = obj_from_bits(exception_bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_EXCEPTION {
        return None;
    }
    let policy = exception_typed_field_policy_by_name(_py, ptr, name)?;
    if !policy.deletable {
        let message = if policy.storage == ExceptionFieldStorage::PySsize {
            "can't delete numeric/char attribute"
        } else {
            "readonly attribute"
        };
        if policy.storage == ExceptionFieldStorage::PySsize {
            let _ = raise_exception::<u64>(_py, "TypeError", message);
        }
        return Some(Err(message));
    }
    let result = if policy.storage == ExceptionFieldStorage::PySsize {
        let layout = unsafe { exception_layout_kind(ptr) };
        let offset = exception_typed_tail_slot(layout, policy.field)
            .expect("schema typed scalar must have a tail slot");
        unsafe {
            let slot = ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64;
            let old = *slot;
            *slot = typed_field_default_raw(policy.field);
            let header = crate::header_from_obj_ptr(ptr);
            let pushed = ((*header).load_synchronized_flags()
                & crate::object::HEADER_FLAG_HAS_ABI_VIEW)
                == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE.refresh_exception_view(exception_bits);
            if !pushed {
                *slot = old;
                Err("exception ABI sidecar synchronization failed")
            } else {
                Ok(())
            }
        }
    } else {
        exception_typed_field_replace_internal(
            _py,
            exception_bits,
            policy.field,
            MoltObject::none().bits(),
        )
    };
    Some(result)
}

/// Validate and replace one public exception field. Inputs are borrowed; the
/// slot takes its own reference. Cause assignment also publishes
/// `__suppress_context__ = True`, matching CPython.
pub(crate) fn exception_replace_field_bits(
    _py: &PyToken<'_>,
    exception_bits: u64,
    field: ExceptionFieldSlot,
    value_bits: u64,
) -> Result<(), &'static str> {
    let Some(exception_ptr) = obj_from_bits(exception_bits).as_ptr() else {
        return Err("expected exception object");
    };
    if unsafe { object_type_id(exception_ptr) } != TYPE_ID_EXCEPTION {
        return Err("expected exception object");
    }
    let value = obj_from_bits(value_bits);
    match field {
        ExceptionFieldSlot::Cause | ExceptionFieldSlot::Context => {
            if !value.is_none() {
                let Some(value_ptr) = value.as_ptr() else {
                    return Err("exception cause/context must be an exception or None");
                };
                if unsafe { object_type_id(value_ptr) } != TYPE_ID_EXCEPTION {
                    return Err("exception cause/context must be an exception or None");
                }
            }
        }
        ExceptionFieldSlot::Traceback => {
            if !value.is_none() {
                let Some(_traceback_ptr) = value.as_ptr() else {
                    return Err("__traceback__ must be a traceback or None");
                };
                let traceback_type = builtin_classes(_py).traceback;
                if traceback_type == 0 || !isinstance_bits(_py, value_bits, traceback_type) {
                    return Err("__traceback__ must be a traceback or None");
                }
            }
        }
        ExceptionFieldSlot::Args => {
            let Some(args_ptr) = value.as_ptr() else {
                return Err("exception args must be a tuple");
            };
            if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
                return Err("exception args must be a tuple");
            }
        }
        ExceptionFieldSlot::Dict => {
            if !value.is_none() {
                let Some(dict_ptr) = value.as_ptr() else {
                    return Err("exception dict must be a dict or None");
                };
                if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                    return Err("exception dict must be a dict or None");
                }
            }
        }
        ExceptionFieldSlot::Notes => {}
    }
    // Publish the complete semantic update before releasing any old edge.  If
    // the object has a C ABI view, push the resulting full snapshot while both
    // old and new graphs remain pinned; a failed sidecar transaction rolls the
    // runtime slots back without forking the two representations.
    unsafe {
        let slot = exception_ptr.add(field.offset() * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        let payload_slot =
            exception_ptr.add(EXCEPTION_ARGS_PAYLOAD_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_payload = *payload_slot;
        let suppress_slot =
            exception_ptr.add(EXCEPTION_SUPPRESS_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_suppress = *suppress_slot;
        let new_suppress = if matches!(field, ExceptionFieldSlot::Cause) {
            MoltObject::from_bool(true).bits()
        } else {
            old_suppress
        };
        if old_bits != value_bits {
            inc_ref_bits(_py, value_bits);
            *slot = value_bits;
        }
        if matches!(field, ExceptionFieldSlot::Args) && old_payload != MoltObject::none().bits() {
            inc_ref_bits(_py, MoltObject::none().bits());
            *payload_slot = MoltObject::none().bits();
        }
        if old_suppress != new_suppress {
            inc_ref_bits(_py, new_suppress);
            *suppress_slot = new_suppress;
        }
        let header = crate::header_from_obj_ptr(exception_ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .refresh_exception_view(MoltObject::from_ptr(exception_ptr).bits());
        if !pushed {
            if old_bits != value_bits {
                *slot = old_bits;
                dec_ref_bits(_py, value_bits);
            }
            if matches!(field, ExceptionFieldSlot::Args) && old_payload != MoltObject::none().bits()
            {
                *payload_slot = old_payload;
                dec_ref_bits(_py, MoltObject::none().bits());
            }
            if old_suppress != new_suppress {
                *suppress_slot = old_suppress;
                dec_ref_bits(_py, new_suppress);
            }
            return Err("exception ABI sidecar synchronization failed");
        }
        if old_bits != value_bits {
            dec_ref_bits(_py, old_bits);
        }
        if matches!(field, ExceptionFieldSlot::Args) && old_payload != MoltObject::none().bits() {
            dec_ref_bits(_py, old_payload);
        }
        if old_suppress != new_suppress {
            dec_ref_bits(_py, old_suppress);
        }
    }
    Ok(())
}

pub(crate) fn exception_replace_suppress_context(
    _py: &PyToken<'_>,
    exception_bits: u64,
    suppress: bool,
) -> Result<(), &'static str> {
    let Some(exception_ptr) = obj_from_bits(exception_bits).as_ptr() else {
        return Err("expected exception object");
    };
    if unsafe { object_type_id(exception_ptr) } != TYPE_ID_EXCEPTION {
        return Err("expected exception object");
    }
    let new_bits = MoltObject::from_bool(suppress).bits();
    unsafe {
        let slot =
            exception_ptr.add(EXCEPTION_SUPPRESS_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits == new_bits {
            return Ok(());
        }
        inc_ref_bits(_py, new_bits);
        *slot = new_bits;
        let header = crate::header_from_obj_ptr(exception_ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE.refresh_exception_view(exception_bits);
        if !pushed {
            *slot = old_bits;
            dec_ref_bits(_py, new_bits);
            return Err("exception ABI sidecar synchronization failed");
        }
        dec_ref_bits(_py, old_bits);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExceptionTypedSnapshotState {
    pub(crate) layout_kind: ExceptionLayoutKind,
    pub(crate) present_mask: u32,
    pub(crate) handles: [u64; MAX_EXCEPTION_TYPED_FIELDS],
    pub(crate) unicode_start: isize,
    pub(crate) unicode_end: isize,
    pub(crate) os_error_written: isize,
}

/// Capture and own every object edge in the typed payload in schema field
/// order. Scalar fields travel losslessly in their native Py_ssize lanes.
pub(crate) unsafe fn exception_capture_typed_snapshot(
    _py: &PyToken<'_>,
    exception_ptr: *mut u8,
) -> ExceptionTypedSnapshotState {
    let layout = unsafe { exception_layout_kind(exception_ptr) };
    let mut snapshot = ExceptionTypedSnapshotState {
        layout_kind: layout,
        present_mask: 0,
        handles: [0; MAX_EXCEPTION_TYPED_FIELDS],
        unicode_start: 0,
        unicode_end: 0,
        os_error_written: -1,
    };
    for (index, policy) in layout.field_policies().iter().enumerate() {
        let field = policy.field;
        match policy.storage {
            ExceptionFieldStorage::RuntimeMessage | ExceptionFieldStorage::Object => {
                let raw = if policy.storage == ExceptionFieldStorage::RuntimeMessage {
                    exception_materialized_message_bits(_py, exception_ptr)
                } else {
                    unsafe { exception_typed_field_raw_bits(exception_ptr, field) }
                        .expect("schema object field must have runtime storage")
                };
                if raw != 0 && !obj_from_bits(raw).is_none() {
                    inc_ref_bits(_py, raw);
                    snapshot.present_mask |= 1u32 << index;
                    snapshot.handles[index] = raw;
                }
            }
            ExceptionFieldStorage::PySsize => {
                let raw = unsafe { exception_typed_field_raw_bits(exception_ptr, field) }
                    .expect("schema scalar field must have runtime storage")
                    as isize;
                match field {
                    ExceptionTypedField::UnicodeStart => snapshot.unicode_start = raw,
                    ExceptionTypedField::UnicodeEnd => snapshot.unicode_end = raw,
                    ExceptionTypedField::OSErrorCharactersWritten => {
                        snapshot.os_error_written = raw
                    }
                    _ => unreachable!("schema has an unknown Py_ssize exception field"),
                }
            }
        }
    }
    snapshot
}

#[derive(Clone, Copy)]
struct ExceptionSnapshotUpdate {
    offset: usize,
    old_raw: u64,
    new_raw: u64,
    owns_edge: bool,
}

/// Publish the complete CPython-visible exception state after the caller has
/// validated every common and typed field. This transaction is deliberately
/// infallible: all incoming edges are pinned before the first slot changes,
/// every slot is then written while the runtime lock is held, and old edges are
/// released only after the whole snapshot is visible. It must not refresh the
/// ABI sidecar because this is the ABI-to-runtime half of that transaction.
pub(crate) unsafe fn exception_commit_snapshot_unchecked(
    _py: &PyToken<'_>,
    exception_ptr: *mut u8,
    fields: [u64; 6],
    suppress_context: bool,
    typed: &ExceptionTypedSnapshotState,
) {
    let [dict, args, notes, traceback, context, cause] = fields;
    let none = MoltObject::none().bits();
    let suppress = MoltObject::from_bool(suppress_context).bits();
    let base_updates = [
        (EXCEPTION_DICT_SLOT, dict, true),
        (EXCEPTION_ARGS_SLOT, args, true),
        (EXCEPTION_NOTES_SLOT, notes, true),
        (EXCEPTION_TRACE_SLOT, traceback, true),
        (EXCEPTION_CONTEXT_SLOT, context, true),
        (EXCEPTION_CAUSE_SLOT, cause, true),
        (EXCEPTION_ARGS_PAYLOAD_SLOT, none, true),
        (EXCEPTION_SUPPRESS_SLOT, suppress, false),
    ];
    debug_assert_eq!(
        unsafe { exception_layout_kind(exception_ptr) },
        typed.layout_kind
    );
    let mut updates: [Option<ExceptionSnapshotUpdate>;
        EXCEPTION_OWNED_EDGE_SLOTS.len() + MAX_EXCEPTION_TYPED_FIELDS] =
        [None; EXCEPTION_OWNED_EDGE_SLOTS.len() + MAX_EXCEPTION_TYPED_FIELDS];
    let mut update_len = 0usize;
    for (offset, new_raw, owns_edge) in base_updates {
        let old_raw =
            unsafe { *(exception_ptr.add(offset * std::mem::size_of::<u64>()) as *const u64) };
        updates[update_len] = Some(ExceptionSnapshotUpdate {
            offset,
            old_raw,
            new_raw,
            owns_edge,
        });
        update_len += 1;
    }
    for (index, policy) in typed.layout_kind.field_policies().iter().enumerate() {
        let field = policy.field;
        let (new_raw, owns_edge) = match policy.storage {
            ExceptionFieldStorage::RuntimeMessage | ExceptionFieldStorage::Object => (
                if typed.present_mask & (1u32 << index) != 0 {
                    typed.handles[index]
                } else {
                    none
                },
                true,
            ),
            ExceptionFieldStorage::PySsize => (
                match field {
                    ExceptionTypedField::UnicodeStart => typed.unicode_start as u64,
                    ExceptionTypedField::UnicodeEnd => typed.unicode_end as u64,
                    ExceptionTypedField::OSErrorCharactersWritten => typed.os_error_written as u64,
                    _ => unreachable!("schema has an unknown Py_ssize exception field"),
                },
                false,
            ),
        };
        let offset = if policy.storage == ExceptionFieldStorage::RuntimeMessage {
            EXCEPTION_MSG_SLOT
        } else {
            exception_typed_tail_slot(typed.layout_kind, field)
                .expect("schema typed field must have runtime storage")
        };
        let old_raw =
            unsafe { *(exception_ptr.add(offset * std::mem::size_of::<u64>()) as *const u64) };
        updates[update_len] = Some(ExceptionSnapshotUpdate {
            offset,
            old_raw,
            new_raw,
            owns_edge,
        });
        update_len += 1;
    }
    let updates = &updates[..update_len];
    for update in updates.iter().flatten() {
        if update.owns_edge && update.old_raw != update.new_raw {
            inc_ref_bits(_py, update.new_raw);
        }
    }
    for update in updates.iter().flatten() {
        if update.old_raw != update.new_raw {
            let slot = unsafe {
                exception_ptr.add(update.offset * std::mem::size_of::<u64>()) as *mut u64
            };
            unsafe { *slot = update.new_raw };
        }
    }
    for update in updates.iter().flatten() {
        if update.owns_edge && update.old_raw != update.new_raw {
            dec_ref_bits(_py, update.old_raw);
        }
    }
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
        let _ = exception_replace_field_bits(
            _py,
            MoltObject::from_ptr(ptr).bits(),
            ExceptionFieldSlot::Args,
            new_bits,
        );
        dec_ref_bits(_py, new_bits);
        return new_bits;
    }
    if obj_from_bits(raw_bits).is_none() || raw_bits == 0 {
        let tuple_ptr = alloc_tuple(_py, &[]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let new_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let _ = exception_replace_field_bits(
            _py,
            MoltObject::from_ptr(ptr).bits(),
            ExceptionFieldSlot::Args,
            new_bits,
        );
        dec_ref_bits(_py, new_bits);
        return new_bits;
    }
    raw_bits
}

pub(crate) unsafe fn exception_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_DICT_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_notes_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_NOTES_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_args_payload_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(EXCEPTION_ARGS_PAYLOAD_SLOT * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn exception_visit_owned_edges(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    unsafe {
        for offset in EXCEPTION_OWNED_EDGE_SLOTS {
            visit(*(ptr.add(offset * std::mem::size_of::<u64>()) as *const u64));
        }
        let layout = exception_layout_kind(ptr);
        for policy in layout.tail_policies() {
            if policy.storage == ExceptionFieldStorage::Object {
                let offset = exception_typed_tail_slot(layout, policy.field)
                    .expect("schema object field must have a typed-tail slot");
                visit(*(ptr.add(offset * std::mem::size_of::<u64>()) as *const u64));
            }
        }
    }
}

pub(crate) unsafe fn exception_detach_owned_edges(ptr: *mut u8) -> DetachedExceptionEdges {
    unsafe {
        let none = MoltObject::none().bits();
        let mut bits = [none; MAX_EXCEPTION_OWNED_EDGES];
        let mut len = 0usize;
        for offset in EXCEPTION_OWNED_EDGE_SLOTS {
            let slot = ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64;
            bits[len] = *slot;
            len += 1;
            *slot = none;
        }
        let layout = exception_layout_kind(ptr);
        for policy in layout.tail_policies() {
            if policy.storage == ExceptionFieldStorage::Object {
                let offset = exception_typed_tail_slot(layout, policy.field)
                    .expect("schema object field must have a typed-tail slot");
                let slot = ptr.add(offset * std::mem::size_of::<u64>()) as *mut u64;
                bits[len] = *slot;
                len += 1;
                *slot = none;
            }
        }
        DetachedExceptionEdges { bits, len }
    }
}

pub(crate) fn exception_move_detached_edges(
    detached: DetachedExceptionEdges,
    sink: &mut DetachedEdgeSink,
) {
    for bits in detached.bits[..detached.len].iter().copied() {
        sink.detach_if_heap(bits);
    }
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
fn thread_last_exception_raw_slot() -> Option<PtrSlot> {
    let ptr = THREAD_LAST_EXCEPTION.with(|slot| slot.get());
    if ptr.is_null() {
        None
    } else {
        Some(PtrSlot(ptr))
    }
}

#[inline]
fn thread_last_exception_valid_slot() -> Option<PtrSlot> {
    let ptr = THREAD_LAST_EXCEPTION.with(|slot| slot.get());
    if ptr.is_null() {
        return None;
    }
    let slot = PtrSlot(ptr);
    assert!(
        exception_slot_is_valid(slot),
        "owned thread exception slot must reference a live exception"
    );
    Some(slot)
}

#[inline]
fn thread_last_exception_pending_slot() -> Option<PtrSlot> {
    thread_last_exception_valid_slot()
}

#[inline]
fn thread_last_exception_take() -> Option<PtrSlot> {
    let ptr = THREAD_LAST_EXCEPTION.with(|slot| slot.replace(std::ptr::null_mut()));
    if current_task_key().is_none() {
        CURRENT_EXCEPTION_PENDING
            .with(|pending| pending.set(emergency_memory_error_pending_for_current()));
    }
    if ptr.is_null() {
        None
    } else {
        Some(PtrSlot(ptr))
    }
}

#[inline]
fn thread_last_exception_store_recorded(_py: &PyToken<'_>, ptr: *mut u8, reuse_existing_ref: bool) {
    clear_emergency_memory_error_for_current();
    if !reuse_existing_ref {
        let bits = MoltObject::from_ptr(ptr).bits();
        inc_ref_bits(_py, bits);
    }
    THREAD_LAST_EXCEPTION.with(|slot| slot.set(ptr));
    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
}

#[inline]
fn thread_last_exception_replace_borrowed(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    clear_emergency_memory_error_for_current();
    let old = THREAD_LAST_EXCEPTION.with(|slot| slot.get());
    if old == ptr {
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
        return;
    }
    inc_ref_bits(_py, bits);
    let old = THREAD_LAST_EXCEPTION.with(|slot| slot.replace(ptr));
    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
    if !old.is_null() && old != ptr {
        let old_bits = MoltObject::from_ptr(old).bits();
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn thread_last_exception_bits_noinc(_py: &PyToken<'_>) -> Option<u64> {
    thread_last_exception_raw_slot().map(|ptr| MoltObject::from_ptr(ptr.0).bits())
}

/// Synchronize the inline pending byte whenever the scheduler changes the
/// current execution context on this native thread.
pub(crate) fn sync_current_exception_pending(_py: &PyToken<'_>, task_ptr: *mut u8) {
    let pending = emergency_memory_error_pending_for_current()
        || if task_ptr.is_null() {
            thread_last_exception_raw_slot().is_some()
        } else {
            task_last_exceptions(_py)
                .lock()
                .unwrap()
                .contains_key(&PtrSlot(task_ptr))
        };
    CURRENT_EXCEPTION_PENDING.with(|flag| flag.set(pending));
}

pub(crate) fn exception_pending(_py: &PyToken<'_>) -> bool {
    if emergency_memory_error_pending_for_current() {
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
        return true;
    }
    if !CURRENT_EXCEPTION_PENDING.with(|pending| pending.get()) {
        return false;
    }
    let debug_pending = debug_exception_pending();
    if let Some(task_key) = current_task_key() {
        let pending_ptr = {
            let guard = task_last_exceptions(_py).lock().unwrap();
            match guard.get(&task_key).copied() {
                Some(ptr) if exception_slot_is_valid(ptr) => Some(ptr),
                Some(_) => panic!("owned task exception slot must reference a live exception"),
                None => None,
            }
        };
        let pending = pending_ptr.is_some();
        if !pending {
            CURRENT_EXCEPTION_PENDING.with(|flag| flag.set(false));
        }
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
    let pending = thread_last_exception_pending_slot().is_some();
    if !pending {
        CURRENT_EXCEPTION_PENDING.with(|flag| flag.set(false));
    }
    if debug_pending
        && pending
        && let Some(ptr) = thread_last_exception_raw_slot()
    {
        let kind_bits = unsafe { exception_kind_bits(ptr.0) };
        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!("molt exc pending task=0x0 kind={}", kind);
    }
    pending
}

pub(crate) fn exception_last_bits_noinc(_py: &PyToken<'_>) -> Option<u64> {
    if let Some(task_key) = current_task_key() {
        return task_last_exceptions(_py)
            .lock()
            .unwrap()
            .get(&task_key)
            .copied()
            .map(|ptr| MoltObject::from_ptr(ptr.0).bits());
    }
    thread_last_exception_bits_noinc(_py)
}

pub(crate) fn clear_thread_exception_for_teardown(_py: &PyToken<'_>) {
    crate::gil_assert();
    clear_all_emergency_memory_error_state();
    let ptr = THREAD_LAST_EXCEPTION
        .try_with(|slot| {
            let ptr = slot.replace(std::ptr::null_mut());
            if ptr.is_null() {
                None
            } else {
                Some(PtrSlot(ptr))
            }
        })
        .ok()
        .flatten();
    if current_task_key().is_none() {
        let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    }
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
    let unraisable_class_bits = state
        .exceptions
        .unraisable_hook_args_class
        .swap(0, AtomicOrdering::AcqRel);
    if unraisable_class_bits != 0 && !obj_from_bits(unraisable_class_bits).is_none() {
        class_break_cycles(_py, unraisable_class_bits);
        dec_ref_bits(_py, unraisable_class_bits);
    }
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
        stack.iter().rev().find_map(|entry| {
            if obj_from_bits(entry.bits).is_none() {
                None
            } else {
                Some(entry.bits)
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
    let mut old_bits = None;
    let mut retain_new = false;
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(slot) = stack.last_mut() else {
            return;
        };
        if obj_from_bits(bits).is_none() {
            if !obj_from_bits(*slot).is_none() {
                old_bits = Some(*slot);
            }
            *slot = MoltObject::none().bits();
            return;
        }
        if *slot == bits {
            return;
        }
        if !obj_from_bits(*slot).is_none() {
            old_bits = Some(*slot);
        }
        retain_new = true;
        *slot = bits;
    });
    if retain_new {
        inc_ref_bits(_py, bits);
    }
    if let Some(old_bits) = old_bits {
        dec_ref_bits(_py, old_bits);
    }
}

/// Replace the active handled exception for the CPython ABI. Interpreter
/// handlers use `ACTIVE_EXCEPTION_STACK`; calls made outside one retain a
/// single fallback root, matching CPython's per-thread base exception-stack
/// item instead of creating a parallel ABI-local `sys.exc_info()` state.
pub(crate) fn exception_context_set_abi(_py: &PyToken<'_>, bits: u64) {
    crate::gil_assert();
    if ACTIVE_EXCEPTION_STACK.with(|stack| !stack.borrow().is_empty()) {
        exception_context_set(_py, bits);
        return;
    }
    let mut old_bits = None;
    let mut retain_new = false;
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let replacement = if obj_from_bits(bits).is_none() {
            MoltObject::none().bits()
        } else {
            bits
        };
        if let Some(slot) = stack.last_mut() {
            if slot.bits == replacement {
                return;
            }
            if slot.owned && !obj_from_bits(slot.bits).is_none() {
                old_bits = Some(slot.bits);
            }
            retain_new = !obj_from_bits(replacement).is_none();
            slot.bits = replacement;
            slot.owned = retain_new;
        } else if !obj_from_bits(replacement).is_none() {
            retain_new = true;
            stack.push(ExceptionContextFallback {
                bits: replacement,
                owned: true,
            });
        }
    });
    if retain_new {
        inc_ref_bits(_py, bits);
    }
    if let Some(old_bits) = old_bits {
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn exception_context_align_depth(_py: &PyToken<'_>, target: usize) {
    crate::gil_assert();
    let detached = ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let split = target.min(stack.len());
        let detached = stack.split_off(split);
        while stack.len() < target {
            stack.push(MoltObject::none().bits());
        }
        detached
    });
    for bits in detached {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

pub(crate) fn exception_context_fallback_push(bits: u64) {
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        stack
            .borrow_mut()
            .push(ExceptionContextFallback { bits, owned: false });
    });
}

pub(crate) fn exception_context_fallback_pop(_py: &PyToken<'_>) {
    let detached = ACTIVE_EXCEPTION_FALLBACK.with(|stack| stack.borrow_mut().pop());
    if let Some(entry) = detached
        && entry.owned
        && !obj_from_bits(entry.bits).is_none()
    {
        dec_ref_bits(_py, entry.bits);
    }
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
            task, depth, baseline, code_bits, line
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
            let detached =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            for bits in detached {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
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
                task, before_depth, baseline, code_bits, line
            );
        }
        return;
    }
    EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    let detached = ACTIVE_EXCEPTION_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some(bits) = detached
        && !obj_from_bits(bits).is_none()
    {
        dec_ref_bits(_py, bits);
    }
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
            task, before_depth, after_depth, baseline, code_bits, line
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

pub(crate) fn generator_exception_stack_visit(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    GENERATOR_EXCEPTION_STACKS.with(|map| {
        if let Some(stack) = map.borrow().get(&(ptr as usize)) {
            for &bits in stack {
                visit(bits);
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

/// Publish every task exception side registry empty without running Python.
///
/// Terminal deallocation and cyclic clear use this as one phase of their
/// object-wide detach transaction.  Heap referents move into the caller's
/// pre-reserved sink; handler/depth/baseline metadata is simply retired.  The
/// caller may release the sink only after every other inline and side-registry
/// edge owned by the task has also been detached.
pub(crate) fn task_exception_detach_owned_edges(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    crate::gil_assert();
    let slot = PtrSlot(ptr);

    let stack = task_exception_stacks(_py)
        .lock()
        .unwrap()
        .remove(&slot)
        .unwrap_or_default();
    for bits in stack {
        sink.detach_if_heap(bits);
    }

    let state = runtime_state(_py);
    let last = {
        let mut exceptions = task_last_exceptions(_py).lock().unwrap();
        let last = exceptions.remove(&slot);
        if exceptions.is_empty() {
            state
                .task_last_exception_pending
                .store(false, AtomicOrdering::Release);
        }
        last
    };
    if let Some(exception) = last {
        sink.detach(MoltObject::from_ptr(exception.0).bits());
    }

    task_exception_handler_stacks(_py)
        .lock()
        .unwrap()
        .remove(&slot);
    task_exception_depths(_py).lock().unwrap().remove(&slot);
    state.task_exception_baselines.lock().unwrap().remove(&slot);
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
    let old_ptr = task_last_exceptions(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr));
    if current_task_ptr() == ptr {
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
    }
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
    clear_emergency_memory_error_for_current();
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
            (*header).ref_count_snapshot()
        };
        eprintln!("molt exc rc start ptr=0x{:x} rc={}", ptr as usize, rc);
    }
    let mut suppress_trace = unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).load_metadata_flags() & HEADER_FLAG_TRACEBACK_SUPPRESSED != 0
    };
    if !suppress_trace
        && traceback_suppressed()
        && exception_matches_builtin_name(_py, MoltObject::from_ptr(ptr).bits(), "AttributeError")
    {
        suppress_trace = true;
        unsafe {
            let header = header_from_obj_ptr(ptr);
            (*header).fetch_or_flags(HEADER_FLAG_TRACEBACK_SUPPRESSED);
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
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
    } else if let Some(old_ptr) = thread_last_exception_take() {
        prior_ptr = Some(old_ptr.0);
    }
    if let Some(old_ptr) = prior_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr).bits();
        if debug_rc {
            let old_rc = unsafe {
                let header = header_from_obj_ptr(old_ptr);
                (*header).ref_count_snapshot()
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
                let _ = exception_replace_field_bits(
                    _py,
                    new_bits,
                    ExceptionFieldSlot::Context,
                    ctx_bits,
                );
                // The field primitive borrows and takes its own edge. Consume
                // an already-owned prior last_exception edge after publication.
                if context_bits_owned {
                    dec_ref_bits(_py, ctx_bits);
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
            let _ = exception_replace_field_bits(
                _py,
                MoltObject::from_ptr(ptr).bits(),
                ExceptionFieldSlot::Traceback,
                MoltObject::none().bits(),
            );
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
            unsafe {
                exception_publish_field_slot(_py, ptr, ExceptionFieldSlot::Traceback, new_bits)
            };
            dec_ref_bits(_py, new_bits);
        } else if !obj_from_bits(trace_bits).is_none() {
            let _ = exception_replace_field_bits(
                _py,
                MoltObject::from_ptr(ptr).bits(),
                ExceptionFieldSlot::Traceback,
                MoltObject::none().bits(),
            );
        }
    }
    if let Some(task_key) = task_key {
        // The task slot owns one strong reference. Same-pointer rerecording
        // reuses the detached slot edge exactly like the thread slot.
        let bits = MoltObject::from_ptr(ptr).bits();
        if !same_ptr {
            inc_ref_bits(_py, bits);
        }
        task_last_exceptions(_py)
            .lock()
            .unwrap()
            .insert(task_key, PtrSlot(ptr));
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
    } else {
        // The global slot owns one strong reference. Re-recording the same
        // pointer reuses the reference removed from the slot above; new
        // pointers acquire a fresh slot reference.
        thread_last_exception_store_recorded(_py, ptr, same_ptr);
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
                    if object_type_id(args_ptr) == TYPE_ID_TUPLE
                        && let Some(first) = crate::object::seq_access::with_immutable_tuple_slice(
                            args_ptr,
                            |elems| elems.first().copied(),
                        )
                        .flatten()
                    {
                        out = format_obj_str(_py, obj_from_bits(first));
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
            (*header).ref_count_snapshot()
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
    clear_emergency_memory_error_for_current();
    if let Some(task_key) = current_task_key() {
        let old_ptr = task_last_exceptions(_py).lock().unwrap().remove(&task_key);
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
        if let Some(old_ptr) = old_ptr {
            let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
            dec_ref_bits(_py, old_bits);
        }
        return;
    }
    let old_ptr = thread_last_exception_take();
    if let Some(old_ptr) = old_ptr {
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
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
    let spec = builtin_exception_spec(name);
    let canonical_name = spec.map(|spec| spec.canonical_name()).unwrap_or(name);
    if canonical_name != name {
        let bits = exception_type_bits_from_name(_py, canonical_name);
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
    let base_spec = spec.map(|spec| spec.bases());
    let base_bits = match base_spec {
        Some(ExceptionBaseSpec::Root) => builtins.base_exception,
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
                if !seed_builtin_exception_layout_root(class_ptr, name) {
                    dec_ref_bits(_py, tuple_bits);
                    dec_ref_bits(_py, MoltObject::from_ptr(class_ptr).bits());
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
    if !seed_builtin_exception_layout_root(class_ptr, name) {
        dec_ref_bits(_py, MoltObject::from_ptr(class_ptr).bits());
        return fallback;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let _ = molt_class_set_base(class_bits, base_bits);
    set_exception_text_signature_none(_py, class_bits);
    cache_exception_type(_py, name, class_bits)
}

fn seed_builtin_exception_layout_root(class_ptr: *mut u8, name: &str) -> bool {
    let Some(root) = builtin_exception_spec(name).and_then(|spec| spec.introduced_layout_root())
    else {
        return true;
    };
    unsafe { crate::object::class_set_exception_layout_root(class_ptr, root) }
}

pub(crate) fn builtin_exception_type_bits_from_name(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    builtin_exception_spec(name)?;
    let bits = exception_type_bits_from_name(_py, name);
    if bits == 0 {
        return None;
    }
    inc_ref_bits(_py, bits);
    Some(bits)
}

pub(crate) fn is_builtin_exception_class_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return false;
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(class_name_bits(ptr))) else {
            return false;
        };
        builtin_exception_spec(&name).is_some() && exception_type_bits_from_name(_py, &name) == bits
    }
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
            if !object_init_class_edge_unpublished(
                _py,
                class_ptr,
                builtins.type_obj,
                ClassEdgeOwnership::Owned,
            ) {
                dec_ref_bits(_py, MoltObject::from_ptr(class_ptr).bits());
                return std::ptr::null_mut();
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
    let Some(name) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
        return 0;
    };
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
    // The tag -> name ordinal mapping has ONE authority
    // (`builtin_exception_name_for_tag`); this switch selects only the
    // per-name atomic cache slot. Do not re-hardcode the name strings here —
    // that reintroduced the drift `builtin_exception_class_cache_for_tag`
    // used to carry and is gated by tools/check_table_drift.py (category
    // `exception-ordinals`).
    let name = builtin_exception_name_for_tag(tag)?;
    let cache = match tag {
        1 => &state.base_exception_class_cache,
        2 => &state.exception_class_cache,
        3 => &state.key_error_class_cache,
        4 => &state.index_error_class_cache,
        5 => &state.value_error_class_cache,
        6 => &state.type_error_class_cache,
        7 => &state.runtime_error_class_cache,
        8 => &state.stop_iteration_class_cache,
        9 => &state.stop_async_iteration_class_cache,
        10 => &state.assertion_error_class_cache,
        11 => &state.import_error_class_cache,
        12 => &state.name_error_class_cache,
        13 => &state.unbound_local_error_class_cache,
        14 => &state.not_implemented_error_class_cache,
        _ => return None,
    };
    Some((cache, name))
}

fn builtin_exception_class_for_tag(_py: &PyToken<'_>, tag: u64) -> Option<u64> {
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
    if matches!(
        exception_layout_kind_for_class(_py, class_bits),
        ExceptionLayoutKind::Import
    ) {
        return exception_message_for_storage(_py, class_bits, args_bits);
    }
    if tag != 3
        && unsafe {
            crate::object::ops_format::exception_class_uses_cached_message_str(_py, class_bits)
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
    if matches!(
        exception_layout_kind_for_class(_py, class_bits),
        ExceptionLayoutKind::Import
    ) {
        inc_ref_bits(_py, arg_bits);
        return arg_bits;
    }
    if tag != 3
        && unsafe {
            crate::object::ops_format::exception_class_uses_cached_message_str(_py, class_bits)
        }
    {
        exception_lazy_message_bits()
    } else {
        molt_str_from_obj(arg_bits)
    }
}

fn alloc_builtin_exception_from_tag(_py: &PyToken<'_>, tag: u64, args_bits: u64) -> *mut u8 {
    let Some(class_bits) = builtin_exception_class_for_tag(_py, tag) else {
        return std::ptr::null_mut();
    };
    let msg_bits = exception_message_for_builtin_tag_storage(_py, tag, class_bits, args_bits);
    if !exception_message_storage_is_valid(_py, class_bits, msg_bits) {
        return std::ptr::null_mut();
    }
    let none_bits = MoltObject::none().bits();
    let mut ptr = alloc_exception_obj(_py, class_bits, msg_bits, args_bits, none_bits);
    if !ptr.is_null()
        && unsafe { exception_initialize_typed_fields_from_args(_py, ptr, args_bits) }.is_err()
    {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        ptr = std::ptr::null_mut();
    }
    dec_ref_bits(_py, msg_bits);
    ptr
}

fn alloc_builtin_exception_from_tag_single(_py: &PyToken<'_>, tag: u64, arg_bits: u64) -> *mut u8 {
    let Some(class_bits) = builtin_exception_class_for_tag(_py, tag) else {
        return std::ptr::null_mut();
    };
    let msg_bits = exception_message_for_builtin_tag_single_storage(_py, tag, class_bits, arg_bits);
    if !exception_message_storage_is_valid(_py, class_bits, msg_bits) {
        return std::ptr::null_mut();
    }
    let none_bits = MoltObject::none().bits();
    let mut ptr = alloc_exception_obj_with_args_payload(
        _py,
        class_bits,
        msg_bits,
        exception_lazy_single_args_bits(),
        none_bits,
        arg_bits,
    );
    if !ptr.is_null()
        && unsafe {
            exception_initialize_typed_fields_from_args(_py, ptr, exception_lazy_single_args_bits())
        }
        .is_err()
    {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        ptr = std::ptr::null_mut();
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
                let out_ptr =
                    crate::object::seq_access::with_borrowed(ptr, |elems| alloc_tuple(_py, elems));
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
                let len = if type_id == TYPE_ID_TUPLE {
                    crate::object::seq_access::len(ptr)
                } else {
                    crate::object::seq_access::locked_len(ptr)
                };
                match len {
                    0 => {
                        let ptr = alloc_string(_py, b"");
                        if ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(ptr).bits();
                    }
                    1 if type_id == TYPE_ID_TUPLE => {
                        let first =
                            crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                                items[0]
                            })
                            .unwrap_or_else(|| MoltObject::none().bits());
                        return molt_str_from_obj(first);
                    }
                    1 => {
                        let Some(item) = crate::object::seq_access::pin_item(_py, ptr, 0) else {
                            return MoltObject::none().bits();
                        };
                        return molt_str_from_obj(item.bits());
                    }
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
        if exception_matches_builtin_name(_py, MoltObject::from_ptr(ptr).bits(), "KeyError") {
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
                let out_ptr =
                    crate::object::seq_access::with_borrowed(ptr, |elems| alloc_tuple(_py, elems));
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
            let Some((item_bits, done_bits)) = crate::object::seq_access::tuple_pair(pair_ptr)
            else {
                return MoltObject::none().bits();
            };
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            elems.push(item_bits);
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
) -> bool {
    unsafe {
        crate::gil_assert();
        let args_slot = ptr.add(EXCEPTION_ARGS_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_args = *args_slot;
        let payload_slot =
            ptr.add(EXCEPTION_ARGS_PAYLOAD_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_payload = *payload_slot;
        let clear_payload =
            exception_args_is_lazy_single(old_args) && old_payload != MoltObject::none().bits();
        if clear_payload {
            inc_ref_bits(_py, MoltObject::none().bits());
        }
        if old_args != args_bits {
            *args_slot = args_bits;
        }
        if clear_payload {
            *payload_slot = MoltObject::none().bits();
        }
        let msg_slot = ptr.add(EXCEPTION_MSG_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_msg = *msg_slot;
        if old_msg != msg_bits {
            *msg_slot = msg_bits;
        }
        let header = crate::header_from_obj_ptr(ptr);
        let pushed =
            ((*header).load_synchronized_flags() & crate::object::HEADER_FLAG_HAS_ABI_VIEW) == 0
                || molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .refresh_exception_view(MoltObject::from_ptr(ptr).bits());
        if !pushed {
            *args_slot = old_args;
            *msg_slot = old_msg;
            if clear_payload {
                *payload_slot = old_payload;
                dec_ref_bits(_py, MoltObject::none().bits());
            }
            dec_ref_bits(_py, args_bits);
            dec_ref_bits(_py, msg_bits);
            return false;
        }
        if old_args != args_bits {
            dec_ref_bits(_py, old_args);
        } else {
            dec_ref_bits(_py, args_bits);
        }
        if old_msg != msg_bits {
            dec_ref_bits(_py, old_msg);
        } else {
            dec_ref_bits(_py, msg_bits);
        }
        if clear_payload {
            dec_ref_bits(_py, old_payload);
        }
        true
    }
}

/// Publish common args/message and a typed-field transition as one runtime/ABI
/// transaction. `args_bits` and `msg_bits` each transfer one owned reference;
/// typed inputs are borrowed. Failure restores both authorities before any old
/// edge is released.
unsafe fn exception_store_args_message_and_typed_updates(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
    msg_bits: u64,
    typed_fields: &[(ExceptionTypedField, u64)],
) -> bool {
    unsafe {
        crate::gil_assert();
        let args_slot = ptr.add(EXCEPTION_ARGS_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let payload_slot =
            ptr.add(EXCEPTION_ARGS_PAYLOAD_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let msg_slot = ptr.add(EXCEPTION_MSG_SLOT * std::mem::size_of::<u64>()) as *mut u64;
        let old_args = *args_slot;
        let old_payload = *payload_slot;
        let old_msg = *msg_slot;
        let clear_payload =
            exception_args_is_lazy_single(old_args) && old_payload != MoltObject::none().bits();
        *args_slot = args_bits;
        *msg_slot = msg_bits;
        if clear_payload {
            *payload_slot = MoltObject::none().bits();
        }

        let pushed = exception_typed_fields_replace_internal(
            _py,
            MoltObject::from_ptr(ptr).bits(),
            typed_fields,
        )
        .is_ok();
        if !pushed {
            *args_slot = old_args;
            *msg_slot = old_msg;
            if clear_payload {
                *payload_slot = old_payload;
            }
            dec_ref_bits(_py, args_bits);
            dec_ref_bits(_py, msg_bits);
            return false;
        }
        if old_args != args_bits {
            dec_ref_bits(_py, old_args);
        } else {
            dec_ref_bits(_py, args_bits);
        }
        if old_msg != msg_bits {
            dec_ref_bits(_py, old_msg);
        } else {
            dec_ref_bits(_py, msg_bits);
        }
        if clear_payload {
            dec_ref_bits(_py, old_payload);
        }
        true
    }
}

unsafe fn exception_reinit_commit(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
    msg_bits: u64,
    typed_fields: &[(ExceptionTypedField, u64)],
) -> Result<(), &'static str> {
    unsafe {
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, msg_bits);
        exception_store_args_message_and_typed_updates(_py, ptr, args_bits, msg_bits, typed_fields)
            .then_some(())
            .ok_or("exception reinitialization publication failed")
    }
}

fn exception_tuple_args(args_bits: u64) -> Option<(usize, u64, u64)> {
    let ptr = obj_from_bits(args_bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |args| {
            (
                args.len(),
                args.first()
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits()),
                args.get(1)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits()),
            )
        })
    }
}

unsafe fn exception_reinit_default_message(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
    typed_fields: &[(ExceptionTypedField, u64)],
) -> Result<(), &'static str> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let msg_bits = exception_message_for_storage(_py, class_bits, args_bits);
    if !exception_message_storage_is_valid(_py, class_bits, msg_bits) {
        return Err("exception message allocation failed");
    }
    let result = unsafe { exception_reinit_commit(_py, ptr, args_bits, msg_bits, typed_fields) };
    dec_ref_bits(_py, msg_bits);
    result
}

unsafe fn syntax_error_reinit(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    let Some((argc, first, second)) = exception_tuple_args(args_bits) else {
        return Err("SyntaxError reinitialization requires tuple arguments");
    };
    let msg_bits = if argc == 0 {
        unsafe { exception_msg_bits(ptr) }
    } else {
        first
    };
    if argc != 2 {
        return unsafe { exception_reinit_commit(_py, ptr, args_bits, msg_bits, &[]) };
    }

    // CPython converts the location object before touching any location field.
    // A conversion failure therefore commits args/msg but preserves all six
    // location edges and print_file_and_line.
    let Some(info_bits) = (unsafe { tuple_from_iter_bits(_py, second) }) else {
        unsafe { exception_reinit_commit(_py, ptr, args_bits, msg_bits, &[]) }?;
        return Err("invalid SyntaxError location iterable");
    };
    let info_ptr = obj_from_bits(info_bits)
        .as_ptr()
        .expect("tuple conversion returned a tuple handle");
    let mut values = [MoltObject::none().bits(); 6];
    let info_len = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(info_ptr, |info| {
            for (target, value) in values.iter_mut().zip(info.iter().copied()) {
                *target = value;
            }
            info.len()
        })
        .expect("tuple conversion returned a tuple object")
    };

    let none = MoltObject::none().bits();
    let result = if !(4..=6).contains(&info_len) {
        unsafe {
            exception_reinit_commit(
                _py,
                ptr,
                args_bits,
                msg_bits,
                &[
                    (ExceptionTypedField::SyntaxEndLineNumber, none),
                    (ExceptionTypedField::SyntaxEndOffset, none),
                ],
            )
        }
    } else {
        unsafe {
            exception_reinit_commit(
                _py,
                ptr,
                args_bits,
                msg_bits,
                &[
                    (ExceptionTypedField::SyntaxFilename, values[0]),
                    (ExceptionTypedField::SyntaxLineNumber, values[1]),
                    (ExceptionTypedField::SyntaxOffset, values[2]),
                    (ExceptionTypedField::SyntaxText, values[3]),
                    (ExceptionTypedField::SyntaxEndLineNumber, values[4]),
                    (ExceptionTypedField::SyntaxEndOffset, values[5]),
                ],
            )
        }
    };
    dec_ref_bits(_py, info_bits);
    result?;

    if info_len < 4 {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("function takes at least 4 arguments ({info_len} given)"),
        );
        return Err("SyntaxError location tuple is too short");
    }
    if info_len > 6 {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("function takes at most 6 arguments ({info_len} given)"),
        );
        return Err("SyntaxError location tuple is too long");
    }
    if info_len == 5 {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "end_offset must be provided when end_lineno is provided",
        );
        return Err("SyntaxError location tuple omits end_offset");
    }
    Ok(())
}

fn unicode_error_kind_for_class(class_bits: u64) -> Option<exception_payload::UnicodeErrorKind> {
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    let root = unsafe { crate::object::class_exception_layout_root(class_ptr) };
    match root {
        ExceptionLayoutRoot::UnicodeDecodeError => {
            Some(exception_payload::UnicodeErrorKind::Decode)
        }
        ExceptionLayoutRoot::UnicodeEncodeError => {
            Some(exception_payload::UnicodeErrorKind::Encode)
        }
        ExceptionLayoutRoot::UnicodeTranslateError => {
            Some(exception_payload::UnicodeErrorKind::Translate)
        }
        _ => None,
    }
}

unsafe fn unicode_error_reinit(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let kind = unicode_error_kind_for_class(class_bits)
        .ok_or("Unicode typed layout requires a concrete UnicodeError root")?;
    let none = MoltObject::none().bits();
    let cleared = match kind {
        exception_payload::UnicodeErrorKind::Translate => &[
            (ExceptionTypedField::UnicodeObject, none),
            (ExceptionTypedField::UnicodeReason, none),
        ][..],
        exception_payload::UnicodeErrorKind::Encode
        | exception_payload::UnicodeErrorKind::Decode => &[
            (ExceptionTypedField::UnicodeEncoding, none),
            (ExceptionTypedField::UnicodeObject, none),
            (ExceptionTypedField::UnicodeReason, none),
        ][..],
    };
    unsafe { exception_reinit_default_message(_py, ptr, args_bits, cleared) }?;
    let fields = unicode_error_fields_from_args(_py, kind, args_bits)
        .map_err(|()| "invalid UnicodeError arguments")?;
    let result = exception_typed_fields_replace_internal(
        _py,
        MoltObject::from_ptr(ptr).bits(),
        &[
            (ExceptionTypedField::UnicodeEncoding, fields.encoding_bits),
            (ExceptionTypedField::UnicodeObject, fields.object_bits),
            (ExceptionTypedField::UnicodeStart, fields.start_bits),
            (ExceptionTypedField::UnicodeEnd, fields.end_bits),
            (ExceptionTypedField::UnicodeReason, fields.reason_bits),
        ],
    );
    fields.release_owned(_py);
    result
}

unsafe fn exception_reinitialize_from_args(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    let layout = unsafe { exception_layout_kind(ptr) };
    let Some((argc, first, _)) = exception_tuple_args(args_bits) else {
        return Err("exception reinitialization requires tuple arguments");
    };
    let none = MoltObject::none().bits();
    match layout {
        ExceptionLayoutKind::OSError => Ok(()),
        ExceptionLayoutKind::Syntax => unsafe { syntax_error_reinit(_py, ptr, args_bits) },
        ExceptionLayoutKind::Unicode => unsafe { unicode_error_reinit(_py, ptr, args_bits) },
        ExceptionLayoutKind::Import => {
            let message = if argc == 1 { first } else { none };
            unsafe {
                exception_reinit_commit(
                    _py,
                    ptr,
                    args_bits,
                    message,
                    &[
                        (ExceptionTypedField::ImportName, none),
                        (ExceptionTypedField::ImportPath, none),
                        (ExceptionTypedField::ImportNameFrom, none),
                    ],
                )
            }
        }
        ExceptionLayoutKind::StopIteration => unsafe {
            exception_reinit_default_message(
                _py,
                ptr,
                args_bits,
                &[(
                    ExceptionTypedField::StopIterationValue,
                    if argc > 0 { first } else { none },
                )],
            )
        },
        ExceptionLayoutKind::SystemExit => {
            let mut updates = [(ExceptionTypedField::SystemExitCode, none); 1];
            let fields = if argc == 0 {
                &updates[..0]
            } else {
                updates[0].1 = if argc == 1 { first } else { args_bits };
                &updates[..]
            };
            unsafe { exception_reinit_default_message(_py, ptr, args_bits, fields) }
        }
        ExceptionLayoutKind::NameError => unsafe {
            exception_reinit_default_message(
                _py,
                ptr,
                args_bits,
                &[(ExceptionTypedField::NameErrorName, none)],
            )
        },
        ExceptionLayoutKind::AttributeError => unsafe {
            exception_reinit_default_message(
                _py,
                ptr,
                args_bits,
                &[
                    (ExceptionTypedField::AttributeErrorObject, none),
                    (ExceptionTypedField::AttributeErrorName, none),
                ],
            )
        },
        ExceptionLayoutKind::Base | ExceptionLayoutKind::Group => unsafe {
            exception_reinit_default_message(_py, ptr, args_bits, &[])
        },
    }
}

unsafe fn exception_reinitialize_from_owner_root(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
    owner_root: ExceptionLayoutRoot,
) -> Result<(), &'static str> {
    match owner_root {
        ExceptionLayoutRoot::Base | ExceptionLayoutRoot::BaseExceptionGroup => unsafe {
            exception_reinit_default_message(_py, ptr, args_bits, &[])
        },
        ExceptionLayoutRoot::OSError => Ok(()),
        _ => unsafe { exception_reinitialize_from_args(_py, ptr, args_bits) },
    }
}

pub(crate) unsafe fn exception_set_stop_iteration_value(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    unsafe {
        crate::gil_assert();
        if exception_layout_kind(ptr) != ExceptionLayoutKind::StopIteration {
            return Ok(());
        }
        let mut value_bits = MoltObject::none().bits();
        if exception_args_is_lazy_single(args_bits) {
            value_bits = exception_args_payload_bits(ptr);
        } else {
            let args_obj = obj_from_bits(args_bits);
            if let Some(args_ptr) = args_obj.as_ptr() {
                let type_id = object_type_id(args_ptr);
                if type_id == TYPE_ID_TUPLE {
                    value_bits =
                        crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |items| {
                            items.first().copied()
                        })
                        .flatten()
                        .unwrap_or(value_bits);
                } else if type_id == TYPE_ID_LIST {
                    if let Some(first) = crate::object::seq_access::pin_item(_py, args_ptr, 0) {
                        return exception_typed_field_replace_internal(
                            _py,
                            MoltObject::from_ptr(ptr).bits(),
                            ExceptionTypedField::StopIterationValue,
                            first.bits(),
                        );
                    }
                } else if !args_obj.is_none() {
                    value_bits = args_bits;
                }
            } else if !args_obj.is_none() {
                value_bits = args_bits;
            }
        }
        exception_typed_field_replace_internal(
            _py,
            MoltObject::from_ptr(ptr).bits(),
            ExceptionTypedField::StopIterationValue,
            value_bits,
        )
    }
}

pub(crate) unsafe fn exception_set_system_exit_code(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    unsafe {
        crate::gil_assert();
        if exception_layout_kind(ptr) != ExceptionLayoutKind::SystemExit {
            return Ok(());
        }
        let mut code_bits = MoltObject::none().bits();
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            let type_id = object_type_id(args_ptr);
            if type_id == TYPE_ID_TUPLE {
                code_bits =
                    crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |elems| {
                        match elems.len() {
                            1 => elems[0],
                            2.. => args_bits,
                            _ => MoltObject::none().bits(),
                        }
                    })
                    .unwrap_or_else(|| MoltObject::none().bits());
            } else if type_id == TYPE_ID_LIST {
                let len = crate::object::seq_access::locked_len(args_ptr);
                if len == 1 {
                    if let Some(first) = crate::object::seq_access::pin_item(_py, args_ptr, 0) {
                        return exception_typed_field_replace_internal(
                            _py,
                            MoltObject::from_ptr(ptr).bits(),
                            ExceptionTypedField::SystemExitCode,
                            first.bits(),
                        );
                    }
                } else if len > 1 {
                    code_bits = args_bits;
                }
            }
        } else if !args_obj.is_none() {
            code_bits = args_bits;
        }
        exception_typed_field_replace_internal(
            _py,
            MoltObject::from_ptr(ptr).bits(),
            ExceptionTypedField::SystemExitCode,
            code_bits,
        )
    }
}

pub(super) unsafe fn exception_initialize_simple_typed_fields(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    unsafe {
        exception_set_stop_iteration_value(_py, ptr, args_bits)?;
        exception_set_system_exit_code(_py, ptr, args_bits)
    }
}

fn syntax_location_fields(
    _py: &PyToken<'_>,
    args_bits: u64,
) -> Result<Option<([u64; 6], u64)>, &'static str> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Ok(None);
    }
    let second = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
            (args.len() == 2).then(|| args[1])
        })
        .flatten()
    };
    let Some(second) = second else {
        return Ok(None);
    };
    let Some(location_bits) = (unsafe { tuple_from_iter_bits(_py, second) }) else {
        return Err("invalid SyntaxError location iterable");
    };
    let location_ptr = obj_from_bits(location_bits)
        .as_ptr()
        .expect("tuple conversion returned a tuple handle");
    let mut fields = [MoltObject::none().bits(); 6];
    let location_len = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(location_ptr, |location| {
            for (target, value) in fields.iter_mut().zip(location.iter().copied()) {
                *target = value;
            }
            location.len()
        })
        .expect("tuple conversion returned a tuple object")
    };
    if location_len < 4 {
        dec_ref_bits(_py, location_bits);
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("function takes at least 4 arguments ({location_len} given)"),
        );
        return Err("invalid SyntaxError location tuple");
    }
    if location_len > 6 {
        dec_ref_bits(_py, location_bits);
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("function takes at most 6 arguments ({location_len} given)"),
        );
        return Err("invalid SyntaxError location tuple");
    }
    if location_len == 5 {
        dec_ref_bits(_py, location_bits);
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "end_offset must be provided when end_lineno is provided",
        );
        return Err("invalid SyntaxError location tuple");
    }
    Ok(Some((fields, location_bits)))
}

/// Sole positional initialization authority for all typed builtin-exception
/// families. Constructor keyword overrides are applied afterward through the
/// same atomic typed-field transaction.
pub(crate) unsafe fn exception_initialize_typed_fields_from_args(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    args_bits: u64,
) -> Result<(), &'static str> {
    let exception_bits = MoltObject::from_ptr(ptr).bits();
    let layout = unsafe { exception_layout_kind(ptr) };
    let none = MoltObject::none().bits();
    let lazy_single = exception_args_is_lazy_single(args_bits);
    let lazy_value = lazy_single.then(|| unsafe { exception_args_payload_bits(ptr) });
    match layout {
        ExceptionLayoutKind::Base | ExceptionLayoutKind::Group => Ok(()),
        ExceptionLayoutKind::Syntax => {
            let mut message = lazy_value.unwrap_or(none);
            if !lazy_single
                && let Some(args_ptr) = obj_from_bits(args_bits).as_ptr()
                && unsafe { object_type_id(args_ptr) } == TYPE_ID_TUPLE
            {
                message = unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                        args.first().copied().unwrap_or(none)
                    })
                    .unwrap_or(none)
                };
            }
            if let Some(([filename, line, offset, text, end_line, end_offset], owner_bits)) =
                syntax_location_fields(_py, args_bits)?
            {
                let result = exception_typed_fields_replace_internal(
                    _py,
                    exception_bits,
                    &[
                        (ExceptionTypedField::SyntaxMessage, message),
                        (ExceptionTypedField::SyntaxFilename, filename),
                        (ExceptionTypedField::SyntaxLineNumber, line),
                        (ExceptionTypedField::SyntaxOffset, offset),
                        (ExceptionTypedField::SyntaxEndLineNumber, end_line),
                        (ExceptionTypedField::SyntaxEndOffset, end_offset),
                        (ExceptionTypedField::SyntaxText, text),
                        (ExceptionTypedField::SyntaxPrintFileAndLine, none),
                    ],
                );
                dec_ref_bits(_py, owner_bits);
                result
            } else {
                exception_typed_field_replace_internal(
                    _py,
                    exception_bits,
                    ExceptionTypedField::SyntaxMessage,
                    message,
                )
            }
        }
        ExceptionLayoutKind::Import => {
            let mut message = lazy_value.unwrap_or(none);
            if !lazy_single
                && let Some(args_ptr) = obj_from_bits(args_bits).as_ptr()
                && unsafe { object_type_id(args_ptr) } == TYPE_ID_TUPLE
            {
                message = unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                        if args.len() == 1 { args[0] } else { none }
                    })
                    .unwrap_or(none)
                };
            }
            exception_typed_fields_replace_internal(
                _py,
                exception_bits,
                &[
                    (ExceptionTypedField::ImportMessage, message),
                    (ExceptionTypedField::ImportName, none),
                    (ExceptionTypedField::ImportPath, none),
                    (ExceptionTypedField::ImportNameFrom, none),
                ],
            )
        }
        ExceptionLayoutKind::Unicode => {
            let kind = unicode_error_kind_for_class(unsafe { object_class_bits(ptr) })
                .ok_or("Unicode typed layout requires a concrete UnicodeError root")?;
            let fields = unicode_error_fields_from_args(_py, kind, args_bits)
                .map_err(|()| "invalid UnicodeError arguments")?;
            let result = exception_typed_fields_replace_internal(
                _py,
                exception_bits,
                &[
                    (ExceptionTypedField::UnicodeEncoding, fields.encoding_bits),
                    (ExceptionTypedField::UnicodeObject, fields.object_bits),
                    (ExceptionTypedField::UnicodeStart, fields.start_bits),
                    (ExceptionTypedField::UnicodeEnd, fields.end_bits),
                    (ExceptionTypedField::UnicodeReason, fields.reason_bits),
                ],
            );
            fields.release_owned(_py);
            result
        }
        ExceptionLayoutKind::SystemExit | ExceptionLayoutKind::StopIteration => unsafe {
            exception_initialize_simple_typed_fields(_py, ptr, args_bits)
        },
        ExceptionLayoutKind::OSError => {
            let class_bits = unsafe { object_class_bits(ptr) };
            let fields = unsafe { oserror_fields_from_args(_py, class_bits, args_bits) };
            let written = fields
                .characters_written_bits
                .unwrap_or_else(|| MoltObject::from_int(-1).bits());
            let mut updates = [
                (ExceptionTypedField::OSErrorErrno, fields.errno_bits),
                (ExceptionTypedField::OSErrorStrError, fields.strerror_bits),
                (ExceptionTypedField::OSErrorFilename, fields.filename_bits),
                (ExceptionTypedField::OSErrorFilename2, fields.filename2_bits),
                (ExceptionTypedField::OSErrorCharactersWritten, written),
                (ExceptionTypedField::OSErrorErrno, fields.errno_bits),
            ];
            #[cfg(not(windows))]
            let len = 5usize;
            #[cfg(windows)]
            let len = {
                updates[4] = (ExceptionTypedField::OSErrorWinError, fields.winerror_bits);
                updates[5] = (ExceptionTypedField::OSErrorCharactersWritten, written);
                6usize
            };
            exception_typed_fields_replace_internal(_py, exception_bits, &updates[..len])
        }
        ExceptionLayoutKind::NameError => exception_typed_field_replace_internal(
            _py,
            exception_bits,
            ExceptionTypedField::NameErrorName,
            none,
        ),
        ExceptionLayoutKind::AttributeError => exception_typed_fields_replace_internal(
            _py,
            exception_bits,
            &[
                (ExceptionTypedField::AttributeErrorObject, none),
                (ExceptionTypedField::AttributeErrorName, none),
            ],
        ),
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
        let msg_bits = exception_message_for_storage(_py, class_bits, args_bits);
        if !exception_message_storage_is_valid(_py, class_bits, msg_bits) {
            dec_ref_bits(_py, args_bits);
            return MoltObject::none().bits();
        }
        let none_bits = MoltObject::none().bits();
        let mut ptr = alloc_exception_obj(_py, class_bits, msg_bits, args_bits, none_bits);
        if !ptr.is_null()
            && unsafe { exception_initialize_typed_fields_from_args(_py, ptr, args_bits) }.is_err()
        {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            ptr = std::ptr::null_mut();
        }
        let out = if ptr.is_null() {
            MoltObject::none().bits()
        } else {
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
        let Some(target_class_bits) = builtin_exception_class_for_tag(_py, tag) else {
            return raise_exception::<u64>(_py, "RuntimeError", "unknown builtin exception tag");
        };
        let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
                return MoltObject::from_bool(false).bits();
            }
            let class_bits = object_class_bits(exc_ptr);
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
        let reinitialized =
            unsafe { exception_reinitialize_from_args(_py, self_ptr, norm_bits) }.is_ok();
        dec_ref_bits(_py, norm_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        if !reinitialized {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn molt_exception_init_owned(
    owner_root_bits: u64,
    self_bits: u64,
    args_bits: u64,
    keyword_names_bits: u64,
    keyword_values_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(owner_root) = obj_from_bits(owner_root_bits)
            .as_int()
            .and_then(|value| u8::try_from(value).ok())
            .and_then(ExceptionLayoutRoot::from_u8)
        else {
            for bits in [args_bits, keyword_names_bits, keyword_values_bits] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            return raise_exception::<u64>(_py, "SystemError", "invalid exception init owner");
        };
        let owner_bits = exception_type_bits_from_name(_py, owner_root.owner_name());
        let cleanup = || {
            for bits in [args_bits, keyword_names_bits, keyword_values_bits] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
        };
        if !isinstance_bits(_py, self_bits, owner_bits) {
            let owner_name = class_name_for_error(owner_bits);
            let received = class_name_for_error(type_of_bits(_py, self_bits));
            let message = format!(
                "descriptor '__init__' requires a '{owner_name}' object but received a '{received}'"
            );
            cleanup();
            return raise_exception::<u64>(_py, "TypeError", &message);
        }
        let names = obj_from_bits(keyword_names_bits)
            .as_ptr()
            .and_then(|ptr| unsafe {
                crate::object::seq_access::snapshot(
                    _py,
                    ptr,
                    "exception keyword-name snapshot allocation failed",
                )
            });
        let values = obj_from_bits(keyword_values_bits)
            .as_ptr()
            .and_then(|ptr| unsafe {
                crate::object::seq_access::snapshot(
                    _py,
                    ptr,
                    "exception keyword-value snapshot allocation failed",
                )
            });
        let (Some(names), Some(values)) = (names, values) else {
            cleanup();
            return MoltObject::none().bits();
        };
        if names.len() != values.len() {
            cleanup();
            return raise_exception::<u64>(_py, "SystemError", "malformed exception keywords");
        }

        let keyword_capable = matches!(
            owner_root,
            ExceptionLayoutRoot::AttributeError
                | ExceptionLayoutRoot::NameError
                | ExceptionLayoutRoot::ImportError
        );
        if !names.is_empty() && !keyword_capable {
            let receiver_class_bits = type_of_bits(_py, self_bits);
            let receiver_name = class_name_for_error(receiver_class_bits);
            let message = format!("{receiver_name}() takes no keyword arguments");
            cleanup();
            return raise_exception::<u64>(_py, "TypeError", &message);
        }
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            cleanup();
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exception init expects exception instance",
            );
        };
        if owner_root == ExceptionLayoutRoot::BaseExceptionGroup {
            let message = unsafe { exception_msg_bits(self_ptr) };
            inc_ref_bits(_py, args_bits);
            inc_ref_bits(_py, message);
            let _ = unsafe { exception_store_args_and_message(_py, self_ptr, args_bits, message) };
            for bits in [keyword_names_bits, keyword_values_bits] {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        // Typed keyword families stage common positional state first, validate
        // the complete keyword set, then atomically replace the whole typed
        // family. A rejected keyword therefore updates .args while preserving
        // every prior typed field, exactly like CPython.
        let reinitialized = if names.is_empty() {
            unsafe { exception_reinitialize_from_owner_root(_py, self_ptr, args_bits, owner_root) }
        } else {
            unsafe { exception_reinit_default_message(_py, self_ptr, args_bits, &[]) }
        };
        if reinitialized.is_ok() && !names.is_empty() {
            let _ = crate::call::class_init::apply_builtin_exception_keywords(
                _py, owner_bits, self_bits, &names, &values,
            );
        }
        dec_ref_bits(_py, args_bits);
        for bits in [keyword_names_bits, keyword_values_bits] {
            dec_ref_bits(_py, bits);
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
        let list_bits = unsafe { exception_notes_bits(self_ptr) };
        if !obj_from_bits(list_bits).is_none() {
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
                if !crate::object::list_mutation::extend_from_slice(
                    _py,
                    list_ptr,
                    std::slice::from_ref(&note_bits),
                ) {
                    return MoltObject::none().bits();
                }
            }
            return MoltObject::none().bits();
        }
        let list_ptr = alloc_list(_py, &[note_bits]);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        if exception_replace_field_bits(_py, self_bits, ExceptionFieldSlot::Notes, list_bits)
            .is_err()
        {
            dec_ref_bits(_py, list_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, list_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_with_traceback(self_bits: u64, traceback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(message) = exception_replace_field_bits(
            _py,
            self_bits,
            ExceptionFieldSlot::Traceback,
            traceback_bits,
        ) {
            let message = if message == "expected exception object" {
                "with_traceback expects exception instance"
            } else {
                message
            };
            return raise_exception::<u64>(_py, "TypeError", message);
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
        if let Err(message) =
            exception_replace_field_bits(_py, exc_bits, ExceptionFieldSlot::Cause, cause_bits)
        {
            return raise_exception::<u64>(_py, "TypeError", message);
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
            if exception_layout_kind(ptr) != ExceptionLayoutKind::StopIteration {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception value field requires StopIteration layout",
                );
            }
            if let Err(message) = exception_typed_field_replace_internal(
                _py,
                exc_bits,
                ExceptionTypedField::StopIterationValue,
                value_bits,
            ) {
                return raise_exception::<u64>(_py, "RuntimeError", message);
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
            CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
        } else {
            thread_last_exception_replace_borrowed(_py, ptr, new_bits);
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
        format_exception_message, generator_exception_stack_store, generator_exception_stack_take,
        molt_exception_new_builtin_one, record_exception, task_exception_detach_owned_edges,
        task_exception_stack_drop, task_exception_stack_store, task_exception_stack_take,
    };
    use crate::builtins::containers::tuple_len;
    use crate::{
        dec_ref_bits, header_from_obj_ptr, intern_static_name, obj_from_bits, runtime_state,
        string_obj_to_owned,
    };
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    #[test]
    fn exception_payload_has_ten_word_prefix_and_schema_sized_tails() {
        assert_eq!(super::EXCEPTION_FIXED_WORDS, 10);
        assert_eq!(
            std::mem::size_of::<[u64; super::EXCEPTION_FIXED_WORDS]>(),
            10 * std::mem::size_of::<u64>()
        );
        assert_eq!(
            super::EXCEPTION_FIXED_WORDS
                + molt_obj_model::ExceptionLayoutKind::Syntax.tail_word_count(),
            17
        );
    }

    #[test]
    fn typed_exception_layout_root_is_class_metadata_not_a_name_heuristic() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let fake_ptr = super::alloc_class_obj_from_name(_py, "SyntaxError");
            assert!(!fake_ptr.is_null());
            let fake_bits = MoltObject::from_ptr(fake_ptr).bits();
            let exception = crate::builtin_classes(_py).exception;
            let _ = crate::molt_class_set_base(fake_bits, exception);
            assert_eq!(
                unsafe { crate::object::class_exception_layout_root(fake_ptr) },
                molt_obj_model::ExceptionLayoutRoot::Base,
                "a user-controlled class name must not select a physical payload"
            );

            let syntax = super::exception_type_bits_from_name(_py, "SyntaxError");
            let syntax_ptr = obj_from_bits(syntax).as_ptr().expect("SyntaxError class");
            assert_eq!(
                unsafe { crate::object::class_exception_layout_root(syntax_ptr) },
                molt_obj_model::ExceptionLayoutRoot::SyntaxError
            );

            let derived_ptr = super::alloc_class_obj_from_name(_py, "DerivedSyntax");
            assert!(!derived_ptr.is_null());
            let derived_bits = MoltObject::from_ptr(derived_ptr).bits();
            let _ = crate::molt_class_set_base(derived_bits, syntax);
            assert_eq!(
                unsafe { crate::object::class_exception_layout_root(derived_ptr) },
                molt_obj_model::ExceptionLayoutRoot::SyntaxError,
                "validated inheritance must publish the concrete root atomically"
            );

            dec_ref_bits(_py, derived_bits);
            dec_ref_bits(_py, fake_bits);
        });
    }

    #[test]
    fn unicode_decode_owns_bytes_and_numeric_delete_raises_typeerror() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = super::exception_type_bits_from_name(_py, "UnicodeDecodeError");
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("UnicodeDecodeError class");
            let encoding_ptr = crate::alloc_string(_py, b"utf-8");
            let source_ptr = crate::alloc_bytearray(_py, b"xy");
            let reason_ptr = crate::alloc_string(_py, b"decode");
            assert!(!encoding_ptr.is_null() && !source_ptr.is_null() && !reason_ptr.is_null());
            let encoding_bits = MoltObject::from_ptr(encoding_ptr).bits();
            let source_bits = MoltObject::from_ptr(source_ptr).bits();
            let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
            let exception_bits = unsafe {
                crate::call_class_init_with_args(
                    _py,
                    class_ptr,
                    &[
                        encoding_bits,
                        source_bits,
                        MoltObject::from_int(0).bits(),
                        MoltObject::from_int(1).bits(),
                        reason_bits,
                    ],
                )
            };
            let exception_ptr = obj_from_bits(exception_bits)
                .as_ptr()
                .expect("decode error");
            let object_bits = unsafe {
                super::exception_typed_field_raw_bits(
                    exception_ptr,
                    molt_obj_model::ExceptionTypedField::UnicodeObject,
                )
                .expect("Unicode object field")
            };
            let object_ptr = obj_from_bits(object_bits).as_ptr().expect("owned bytes");
            assert_eq!(
                unsafe { crate::object_type_id(object_ptr) },
                crate::TYPE_ID_BYTES
            );
            assert_ne!(
                object_bits, source_bits,
                "mutable input must be copied to bytes"
            );

            let name_ptr = crate::alloc_string(_py, b"start");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let deleted = crate::molt_del_attr_name(exception_bits, name_bits);
            assert!(obj_from_bits(deleted).is_none());
            let error_bits = crate::molt_exception_last();
            let error_ptr = obj_from_bits(error_bits)
                .as_ptr()
                .expect("pending TypeError");
            assert_eq!(
                super::format_exception_message(_py, error_ptr),
                "can't delete numeric/char attribute"
            );
            assert!(super::exception_matches_builtin_name(
                _py,
                error_bits,
                "TypeError"
            ));

            super::clear_exception(_py);
            dec_ref_bits(_py, error_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, exception_bits);
            dec_ref_bits(_py, encoding_bits);
            dec_ref_bits(_py, source_bits);
            dec_ref_bits(_py, reason_bits);
        });
    }

    #[test]
    fn uncaught_raise_remains_pending_until_host_boundary_reports_it() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            assert!(!super::exception_handler_active());
            let exception_ptr = super::alloc_exception(_py, "RuntimeError", "uncaught");
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let result = crate::molt_raise(exception_bits);
            assert!(obj_from_bits(result).is_none());
            assert!(super::exception_pending(_py));
            let pending_bits = crate::molt_exception_last();
            assert_eq!(pending_bits, exception_bits);

            super::clear_exception(_py);
            dec_ref_bits(_py, pending_bits);
            dec_ref_bits(_py, exception_bits);
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn system_exit_status_accepts_only_python_int_storage_and_projects_status_bits() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = super::exception_type_bits_from_name(_py, "SystemExit");
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("SystemExit class");

            let float_bits = MoltObject::from_float(2.0).bits();
            let float_exc_bits =
                unsafe { crate::call_class_init_with_args(_py, class_ptr, &[float_bits]) };
            let float_exc_ptr = obj_from_bits(float_exc_bits).as_ptr().unwrap_or_else(|| {
                let error_bits = crate::molt_exception_last();
                let detail = obj_from_bits(error_bits)
                    .as_ptr()
                    .map(|ptr| super::format_exception_message(_py, ptr))
                    .unwrap_or_else(|| "no pending exception".to_string());
                panic!("SystemExit float instance: {detail}");
            });
            assert_eq!(super::system_exit_code(_py, float_exc_ptr), 1);

            let huge_bits = crate::builtins::numbers::int_bits_from_bigint(
                _py,
                num_bigint::BigInt::from(1u8) << 100usize,
            );
            let huge_exc_bits =
                unsafe { crate::call_class_init_with_args(_py, class_ptr, &[huge_bits]) };
            let huge_exc_ptr = obj_from_bits(huge_exc_bits)
                .as_ptr()
                .expect("SystemExit bigint instance");
            assert_eq!(super::system_exit_code(_py, huge_exc_ptr), -1);

            let fitting_wide_bits = crate::builtins::numbers::int_bits_from_bigint(
                _py,
                num_bigint::BigInt::from(1u8) << 40usize,
            );
            let fitting_wide_exc_bits =
                unsafe { crate::call_class_init_with_args(_py, class_ptr, &[fitting_wide_bits]) };
            let fitting_wide_exc_ptr = obj_from_bits(fitting_wide_exc_bits)
                .as_ptr()
                .expect("SystemExit fitting wide integer instance");
            assert_eq!(super::system_exit_code(_py, fitting_wide_exc_ptr), 0);

            dec_ref_bits(_py, float_exc_bits);
            dec_ref_bits(_py, huge_exc_bits);
            dec_ref_bits(_py, huge_bits);
            dec_ref_bits(_py, fitting_wide_exc_bits);
            dec_ref_bits(_py, fitting_wide_bits);
        });
    }

    #[test]
    fn exception_init_descriptor_uses_layout_root_identity_without_descendant_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let import_bits = super::exception_type_bits_from_name(_py, "ImportError");
            let module_bits = super::exception_type_bits_from_name(_py, "ModuleNotFoundError");
            let import_init = super::exception_method_bits_for_owner(_py, import_bits, "__init__");
            assert!(
                import_init.is_some(),
                "the declaring layout root owns __init__"
            );
            assert_eq!(
                super::exception_method_bits_for_owner(_py, module_bits, "__init__"),
                None,
                "typed descendants inherit the root descriptor through MRO"
            );
        });
    }

    #[test]
    fn typed_exception_syntax_reinit_stages_state_like_cpython_312() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let old_msg_ptr = crate::alloc_string(_py, b"old");
            let old_file_ptr = crate::alloc_string(_py, b"old.py");
            let old_text_ptr = crate::alloc_string(_py, b"old\n");
            assert!(!old_msg_ptr.is_null() && !old_file_ptr.is_null() && !old_text_ptr.is_null());
            let old_msg = MoltObject::from_ptr(old_msg_ptr).bits();
            let old_file = MoltObject::from_ptr(old_file_ptr).bits();
            let old_text = MoltObject::from_ptr(old_text_ptr).bits();
            let old_location_ptr = crate::alloc_tuple(
                _py,
                &[
                    old_file,
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    old_text,
                    MoltObject::from_int(3).bits(),
                    MoltObject::from_int(4).bits(),
                ],
            );
            let old_location = MoltObject::from_ptr(old_location_ptr).bits();
            let initial_args_ptr = crate::alloc_tuple(_py, &[old_msg, old_location]);
            let initial_args = MoltObject::from_ptr(initial_args_ptr).bits();
            let syntax = super::exception_type_bits_from_name(_py, "SyntaxError");
            let exc_ptr = super::alloc_exception_from_class_bits(_py, syntax, initial_args);
            assert!(!exc_ptr.is_null());
            let raw = |field| unsafe {
                super::exception_typed_field_raw_bits(exc_ptr, field).expect("SyntaxError field")
            };
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxMessage),
                old_msg
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxFilename),
                old_file
            );

            let empty_ptr = crate::alloc_tuple(_py, &[]);
            let empty = MoltObject::from_ptr(empty_ptr).bits();
            unsafe { super::exception_reinitialize_from_args(_py, exc_ptr, empty) }
                .expect("zero-argument SyntaxError reinit");
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxMessage),
                old_msg
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxFilename),
                old_file
            );

            let new_msg_ptr = crate::alloc_string(_py, b"new");
            let new_msg = MoltObject::from_ptr(new_msg_ptr).bits();
            let one_ptr = crate::alloc_tuple(_py, &[new_msg]);
            let one = MoltObject::from_ptr(one_ptr).bits();
            unsafe { super::exception_reinitialize_from_args(_py, exc_ptr, one) }
                .expect("one-argument SyntaxError reinit");
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxMessage),
                new_msg
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxFilename),
                old_file
            );

            let short_file_ptr = crate::alloc_string(_py, b"short.py");
            let short_file = MoltObject::from_ptr(short_file_ptr).bits();
            let short_location_ptr =
                crate::alloc_tuple(_py, &[short_file, MoltObject::from_int(9).bits()]);
            let short_location = MoltObject::from_ptr(short_location_ptr).bits();
            let short_msg_ptr = crate::alloc_string(_py, b"short");
            let short_msg = MoltObject::from_ptr(short_msg_ptr).bits();
            let short_args_ptr = crate::alloc_tuple(_py, &[short_msg, short_location]);
            let short_args = MoltObject::from_ptr(short_args_ptr).bits();
            assert!(
                unsafe { super::exception_reinitialize_from_args(_py, exc_ptr, short_args) }
                    .is_err()
            );
            assert!(exception_pending(_py));
            assert_eq!(unsafe { super::exception_args_bits(exc_ptr) }, short_args);
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxMessage),
                short_msg
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxFilename),
                old_file
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxEndLineNumber),
                MoltObject::none().bits()
            );
            assert_eq!(
                raw(molt_obj_model::ExceptionTypedField::SyntaxEndOffset),
                MoltObject::none().bits()
            );
            clear_exception(_py);

            dec_ref_bits(_py, MoltObject::from_ptr(exc_ptr).bits());
            for bits in [
                short_args,
                short_msg,
                short_location,
                short_file,
                one,
                new_msg,
                empty,
                initial_args,
                old_location,
                old_text,
                old_file,
                old_msg,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn typed_exception_unicode_bad_arity_commits_cpython_312_partial_state() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let encoding_ptr = crate::alloc_string(_py, b"utf-8");
            let object_ptr = crate::alloc_string(_py, b"x");
            let reason_ptr = crate::alloc_string(_py, b"old");
            let encoding = MoltObject::from_ptr(encoding_ptr).bits();
            let object = MoltObject::from_ptr(object_ptr).bits();
            let reason = MoltObject::from_ptr(reason_ptr).bits();
            let initial_ptr = crate::alloc_tuple(
                _py,
                &[
                    encoding,
                    object,
                    MoltObject::from_int(0).bits(),
                    MoltObject::from_int(1).bits(),
                    reason,
                ],
            );
            let initial = MoltObject::from_ptr(initial_ptr).bits();
            let class_bits = super::exception_type_bits_from_name(_py, "UnicodeEncodeError");
            let exc_ptr = super::alloc_exception_from_class_bits(_py, class_bits, initial);
            assert!(!exc_ptr.is_null());

            let bad_ptr = crate::alloc_string(_py, b"bad");
            let bad = MoltObject::from_ptr(bad_ptr).bits();
            let bad_args_ptr = crate::alloc_tuple(_py, &[bad]);
            let bad_args = MoltObject::from_ptr(bad_args_ptr).bits();
            assert!(
                unsafe { super::exception_reinitialize_from_args(_py, exc_ptr, bad_args) }.is_err()
            );
            assert!(exception_pending(_py));
            assert_eq!(unsafe { super::exception_args_bits(exc_ptr) }, bad_args);
            for field in [
                molt_obj_model::ExceptionTypedField::UnicodeEncoding,
                molt_obj_model::ExceptionTypedField::UnicodeObject,
                molt_obj_model::ExceptionTypedField::UnicodeReason,
            ] {
                assert_eq!(
                    unsafe { super::exception_typed_field_raw_bits(exc_ptr, field) },
                    Some(MoltObject::none().bits())
                );
            }
            assert_eq!(
                unsafe {
                    super::exception_typed_field_raw_bits(
                        exc_ptr,
                        molt_obj_model::ExceptionTypedField::UnicodeStart,
                    )
                },
                Some(0)
            );
            assert_eq!(
                unsafe {
                    super::exception_typed_field_raw_bits(
                        exc_ptr,
                        molt_obj_model::ExceptionTypedField::UnicodeEnd,
                    )
                },
                Some(1)
            );
            clear_exception(_py);

            dec_ref_bits(_py, MoltObject::from_ptr(exc_ptr).bits());
            for bits in [bad_args, bad, initial, reason, object, encoding] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn exception_landing_identity_is_one_owned_class_edge_and_name_is_derived() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_ptr = super::alloc_class_obj_from_name(_py, "IdentityBefore");
            assert!(!class_ptr.is_null());
            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            let class_rc_before = unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
            let msg_ptr = crate::alloc_string(_py, b"identity");
            let args_ptr = crate::alloc_tuple(_py, &[]);
            assert!(!msg_ptr.is_null());
            assert!(!args_ptr.is_null());
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let args_bits = MoltObject::from_ptr(args_ptr).bits();
            let exc_ptr = super::alloc_exception_obj(
                _py,
                class_bits,
                msg_bits,
                args_bits,
                MoltObject::none().bits(),
            );
            assert!(!exc_ptr.is_null());
            assert_eq!(
                unsafe { (*header_from_obj_ptr(exc_ptr)).aux_kind },
                crate::object::HEADER_AUX_KIND_CLASS_INLINE,
                "exception constructors must reserve their class lane before publication"
            );
            dec_ref_bits(_py, msg_bits);
            dec_ref_bits(_py, args_bits);

            assert_eq!(unsafe { crate::object_class_bits(exc_ptr) }, class_bits);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                class_rc_before + 1
            );

            let renamed_ptr = crate::alloc_string(_py, b"IdentityAfter");
            assert!(!renamed_ptr.is_null());
            let renamed_bits = MoltObject::from_ptr(renamed_ptr).bits();
            unsafe { crate::class_set_name_bits(_py, class_ptr, renamed_bits) };
            dec_ref_bits(_py, renamed_bits);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(unsafe {
                    super::exception_kind_bits(exc_ptr)
                }))
                .as_deref(),
                Some("IdentityAfter")
            );

            let mut visited = Vec::new();
            unsafe { super::exception_visit_owned_edges(exc_ptr, |bits| visited.push(bits)) };
            let none = MoltObject::none().bits();
            assert_eq!(
                visited,
                [msg_bits, none, args_bits, none, none, none, none, none]
            );
            let detached = unsafe { super::exception_detach_owned_edges(exc_ptr) };
            let detached_again = unsafe { super::exception_detach_owned_edges(exc_ptr) };
            assert!(
                detached_again
                    .bits
                    .iter()
                    .all(|bits| obj_from_bits(*bits).is_none())
            );
            let mut sink = crate::object::heap_lifecycle::DetachedEdgeSink::try_with_capacities(
                2 * super::EXCEPTION_OWNED_EDGE_SLOTS.len(),
                0,
            )
            .expect("exception test sink allocation");
            super::exception_move_detached_edges(detached, &mut sink);
            super::exception_move_detached_edges(detached_again, &mut sink);
            sink.release_all(_py);

            dec_ref_bits(_py, MoltObject::from_ptr(exc_ptr).bits());
            assert_eq!(
                unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                class_rc_before
            );
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn exception_landing_invalid_class_rolls_back_every_payload_edge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let invalid_class_ptr = crate::alloc_string(_py, b"not a class");
            let msg_ptr = crate::alloc_string(_py, b"payload");
            let args_ptr = crate::alloc_tuple(_py, &[]);
            assert!(!invalid_class_ptr.is_null() && !msg_ptr.is_null() && !args_ptr.is_null());
            let invalid_class_bits = MoltObject::from_ptr(invalid_class_ptr).bits();
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let args_bits = MoltObject::from_ptr(args_ptr).bits();
            let refcount =
                |ptr: *mut u8| unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() };
            let invalid_class_baseline = refcount(invalid_class_ptr);
            let msg_baseline = refcount(msg_ptr);
            let args_baseline = refcount(args_ptr);

            let exc_ptr = super::alloc_exception_obj(
                _py,
                invalid_class_bits,
                msg_bits,
                args_bits,
                MoltObject::none().bits(),
            );
            assert!(exc_ptr.is_null());
            assert_eq!(refcount(invalid_class_ptr), invalid_class_baseline);
            assert_eq!(refcount(msg_ptr), msg_baseline);
            assert_eq!(refcount(args_ptr), args_baseline);

            dec_ref_bits(_py, invalid_class_bits);
            dec_ref_bits(_py, msg_bits);
            dec_ref_bits(_py, args_bits);
        });
    }

    #[test]
    fn exceptions_runtime_state_is_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            let value_error_bits =
                super::builtin_exception_class_for_tag(_py, 5).expect("ValueError class");
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
    fn generator_lifecycle_detach_drains_exception_stack() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let generator_bits = crate::molt_generator_new(0, crate::GEN_CONTROL_SIZE as u64);
            let ptr = obj_from_bits(generator_bits)
                .as_ptr()
                .expect("generator allocation");
            let value_ptr = crate::alloc_string(_py, b"generator-exception-stack");
            assert!(!value_ptr.is_null());
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            generator_exception_stack_store(ptr, vec![value_bits]);
            let (edge_capacity, resource_capacity) =
                unsafe { crate::object::heap_lifecycle::terminal_detach_capacity(_py, ptr) };
            let mut sink = crate::object::heap_lifecycle::DetachedEdgeSink::try_with_capacities(
                edge_capacity,
                resource_capacity,
            )
            .expect("generator lifecycle sink allocation");
            unsafe { crate::object::heap_lifecycle::detach_generator_owned_edges(ptr, &mut sink) };
            let after = generator_exception_stack_take(ptr);
            assert!(
                after.is_empty(),
                "generator lifecycle detachment must publish an empty exception stack"
            );
            sink.release_all(_py);
            dec_ref_bits(_py, generator_bits);
        });
    }

    #[test]
    fn task_exception_stack_drop_clears_entries() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
    fn task_exception_detach_publishes_all_side_registries_empty_before_release() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let task = crate::alloc_list(_py, &[]);
            let stacked = crate::alloc_list(_py, &[]);
            let last = crate::alloc_list(_py, &[]);
            let task_slot = crate::PtrSlot(task);
            let stacked_bits = MoltObject::from_ptr(stacked).bits();
            let last_bits = MoltObject::from_ptr(last).bits();

            crate::inc_ref_bits(_py, stacked_bits);
            task_exception_stack_store(_py, task, vec![stacked_bits]);
            crate::inc_ref_bits(_py, last_bits);
            super::task_last_exceptions(_py)
                .lock()
                .unwrap()
                .insert(task_slot, crate::PtrSlot(last));
            let state = runtime_state(_py);
            state
                .task_exception_handler_stacks
                .lock()
                .unwrap()
                .insert(task_slot, vec![7]);
            state
                .task_exception_depths
                .lock()
                .unwrap()
                .insert(task_slot, 3);
            state
                .task_exception_baselines
                .lock()
                .unwrap()
                .insert(task_slot, 2);
            state
                .task_last_exception_pending
                .store(true, Ordering::Release);

            let mut sink =
                crate::object::heap_lifecycle::DetachedEdgeSink::try_with_capacities(2, 0)
                    .expect("test detach sink");
            task_exception_detach_owned_edges(_py, task, &mut sink);

            assert!(
                !super::task_exception_stacks(_py)
                    .lock()
                    .unwrap()
                    .contains_key(&task_slot)
            );
            assert!(
                !super::task_last_exceptions(_py)
                    .lock()
                    .unwrap()
                    .contains_key(&task_slot)
            );
            assert!(
                !state
                    .task_exception_handler_stacks
                    .lock()
                    .unwrap()
                    .contains_key(&task_slot)
            );
            assert!(
                !state
                    .task_exception_depths
                    .lock()
                    .unwrap()
                    .contains_key(&task_slot)
            );
            assert!(
                !state
                    .task_exception_baselines
                    .lock()
                    .unwrap()
                    .contains_key(&task_slot)
            );
            assert!(!state.task_last_exception_pending.load(Ordering::Acquire));

            sink.release_all(_py);
            dec_ref_bits(_py, MoltObject::from_ptr(task).bits());
            dec_ref_bits(_py, stacked_bits);
            dec_ref_bits(_py, last_bits);
        });
    }

    #[test]
    fn exception_last_ignores_non_pending_slots_inside_handler() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
    fn active_exception_stack_owns_one_canonical_view_until_context_clear() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_ptr = alloc_exception(_py, "RuntimeError", "canonical-active");
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
            let view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exc_bits)
            };
            assert!(!view.is_null());

            exception_stack_push();
            exception_context_set(_py, exc_bits);
            dec_ref_bits(_py, exc_bits);

            assert_eq!(super::exception_context_active_bits(), Some(exc_bits));
            let same_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exc_bits)
            };
            assert_eq!(same_view, view);
            unsafe {
                molt_cpython_abi::api::refcount::Py_INCREF(view);
                molt_cpython_abi::api::refcount::Py_DECREF(view);
            }
            crate::inc_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, exc_bits);
            assert_eq!(
                unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exc_bits)
                },
                view
            );

            exception_context_set(_py, MoltObject::none().bits());
            assert!(super::exception_context_active_bits().is_none());
            assert!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(view)
                    .is_none(),
                "clearing the sole active-stack owner must retire the canonical view"
            );
            exception_stack_pop(_py);
        });
    }

    #[test]
    fn active_exception_stack_pop_preserves_direct_c_root_and_pointer_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_ptr = alloc_exception(_py, "ValueError", "canonical-pop");
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
            let view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exc_bits)
            };
            assert!(!view.is_null());
            unsafe { molt_cpython_abi::api::refcount::Py_INCREF(view) };

            exception_stack_push();
            exception_context_set(_py, exc_bits);
            dec_ref_bits(_py, exc_bits);
            exception_stack_pop(_py);

            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(view),
                Some(exc_bits),
                "the direct C root must retain the stack-released exception"
            );
            crate::inc_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, exc_bits);
            assert_eq!(
                unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exc_bits)
                },
                view,
                "temporary runtime custody must preserve canonical pointer identity"
            );

            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
            assert!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(view)
                    .is_none(),
                "C-last release must retire the canonical view after stack pop"
            );
        });
    }

    #[test]
    fn builtin_exception_one_arg_materializes_args_on_demand() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
                assert_eq!(
                    crate::object::seq_access::item(args_ptr, 0)
                        .expect("exception args tuple must contain its message"),
                    MoltObject::from_int(42).bits()
                );
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
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let exc_bits = molt_exception_new_builtin_one(8, MoltObject::from_int(99).bits());
            let exc_ptr = obj_from_bits(exc_bits).as_ptr().expect("exception object");
            unsafe {
                assert!(super::exception_args_is_lazy_single(
                    super::exception_args_bits(exc_ptr)
                ));
                assert_eq!(
                    super::exception_typed_field_raw_bits(
                        exc_ptr,
                        molt_obj_model::ExceptionTypedField::StopIterationValue,
                    )
                    .expect("StopIteration value slot"),
                    MoltObject::from_int(99).bits()
                );
            }

            let _args_bits = super::exception_materialized_args_bits(_py, exc_ptr);
            unsafe {
                assert!(!super::exception_args_is_lazy_single(
                    super::exception_args_bits(exc_ptr)
                ));
                assert_eq!(
                    super::exception_typed_field_raw_bits(
                        exc_ptr,
                        molt_obj_model::ExceptionTypedField::StopIterationValue,
                    )
                    .expect("StopIteration value slot"),
                    MoltObject::from_int(99).bits()
                );
            }
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    fn execution_contexts_isolate_pending_exceptions_and_fast_flag() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let task_a = Box::into_raw(Box::new(0_u8));
            let task_b = Box::into_raw(Box::new(0_u8));
            let thread_exc = alloc_exception(_py, "ValueError", "thread");
            let task_a_exc = alloc_exception(_py, "TypeError", "task-a");
            let task_b_exc = alloc_exception(_py, "RuntimeError", "task-b");
            let thread_bits = MoltObject::from_ptr(thread_exc).bits();
            let task_a_bits = MoltObject::from_ptr(task_a_exc).bits();
            let task_b_bits = MoltObject::from_ptr(task_b_exc).bits();
            let flag_ptr =
                super::exception_state_abi::molt_exception_pending_flag_ptr() as *const bool;
            assert!(!flag_ptr.is_null());

            record_exception(_py, thread_exc);
            assert!(exception_pending(_py));
            assert!(unsafe { *flag_ptr });

            let previous = crate::replace_current_task(_py, task_a);
            assert!(previous.is_null());
            assert!(
                !exception_pending(_py),
                "ambient thread error must be hidden"
            );
            assert!(!unsafe { *flag_ptr });
            record_exception(_py, task_a_exc);

            crate::replace_current_task(_py, task_b);
            assert!(
                !exception_pending(_py),
                "task A error must be hidden from task B"
            );
            record_exception(_py, task_b_exc);

            crate::replace_current_task(_py, task_a);
            assert_eq!(exception_last_pending_bits(_py), task_a_bits);
            dec_ref_bits(_py, task_a_bits);
            clear_exception(_py);
            assert!(!unsafe { *flag_ptr });

            crate::replace_current_task(_py, task_b);
            assert_eq!(exception_last_pending_bits(_py), task_b_bits);
            dec_ref_bits(_py, task_b_bits);
            clear_exception(_py);

            crate::replace_current_task(_py, std::ptr::null_mut());
            assert_eq!(exception_last_pending_bits(_py), thread_bits);
            dec_ref_bits(_py, thread_bits);
            clear_exception(_py);
            assert!(!unsafe { *flag_ptr });

            dec_ref_bits(_py, task_b_bits);
            dec_ref_bits(_py, task_a_bits);
            dec_ref_bits(_py, thread_bits);
            unsafe {
                drop(Box::from_raw(task_b));
                drop(Box::from_raw(task_a));
            }
        });
    }

    #[test]
    fn rerecording_same_exception_reuses_the_owned_slot_edge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let task = Box::into_raw(Box::new(0_u8));
            let exc_ptr = alloc_exception(_py, "RuntimeError", "same-owner");
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
            let refcount = || unsafe { (*header_from_obj_ptr(exc_ptr)).ref_count_snapshot() };
            let baseline = refcount();

            record_exception(_py, exc_ptr);
            assert_eq!(refcount(), baseline + 1);
            record_exception(_py, exc_ptr);
            assert_eq!(refcount(), baseline + 1);
            clear_exception(_py);
            assert_eq!(refcount(), baseline);

            crate::replace_current_task(_py, task);
            record_exception(_py, exc_ptr);
            assert_eq!(refcount(), baseline + 1);
            record_exception(_py, exc_ptr);
            assert_eq!(refcount(), baseline + 1);
            clear_exception(_py);
            assert_eq!(refcount(), baseline);
            crate::replace_current_task(_py, std::ptr::null_mut());

            dec_ref_bits(_py, exc_bits);
            unsafe { drop(Box::from_raw(task)) };
        });
    }

    #[test]
    fn worker_exit_releases_its_pending_exception_edge() {
        // This deliberately does not hold a runtime test transaction. The worker's TLS
        // destructor reacquires runtime custody before consuming its owned
        // exception edge, so holding an unrelated process-wide fixture lock
        // across join would create a hidden lock lifetime and can deadlock a
        // parallel test that is already exercising the GIL. The exception
        // object and both refcount observations are protected by the GIL; TLS
        // teardown is the authority being tested here.
        let (exc_bits, baseline) = crate::with_gil_entry_nopanic!(_py, {
            let exc_ptr = alloc_exception(_py, "RuntimeError", "worker-exit");
            let bits = MoltObject::from_ptr(exc_ptr).bits();
            let count = unsafe { (*header_from_obj_ptr(exc_ptr)).ref_count_snapshot() };
            (bits, count)
        });

        std::thread::spawn(move || {
            crate::with_gil_entry_nopanic!(_py, {
                let ptr = obj_from_bits(exc_bits).as_ptr().expect("live exception");
                record_exception(_py, ptr);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() },
                    baseline + 1
                );
            });
        })
        .join()
        .expect("worker cleanup must complete");

        crate::with_gil_entry_nopanic!(_py, {
            let ptr = obj_from_bits(exc_bits)
                .as_ptr()
                .expect("main owner remains live");
            assert_eq!(
                unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() },
                baseline,
                "TLS teardown must consume the worker's pending-exception edge"
            );
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    #[ignore = "permanently shuts down process-global runtime; run in isolation"]
    fn tls_exception_destructor_excludes_concurrent_shutdown() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let exc_bits = crate::with_gil_entry_nopanic!(_py, {
            MoltObject::from_ptr(alloc_exception(_py, "RuntimeError", "tls-shutdown")).bits()
        });
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let (thread_id_tx, thread_id_rx) = std::sync::mpsc::channel();
        let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
        let (recorded_tx, recorded_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let (destructor_done_tx, destructor_done_rx) = std::sync::mpsc::channel();
        let worker_exc_bits = exc_bits;
        let worker = std::thread::spawn(move || {
            thread_id_tx.send(std::thread::current().id()).unwrap();
            proceed_rx.recv().unwrap();
            crate::with_gil_entry_nopanic!(_py, {
                let ptr = obj_from_bits(worker_exc_bits)
                    .as_ptr()
                    .expect("live exception");
                record_exception(_py, ptr);
            });
            recorded_tx.send(()).unwrap();
            exit_rx.recv().unwrap();
        });
        let worker_id = thread_id_rx.recv().unwrap();
        *super::state::THREAD_EXCEPTION_DROP_TEST_GATE
            .lock()
            .unwrap() = Some(super::state::ThreadExceptionDropTestGate {
            owner: worker_id,
            entered: entered_tx,
            release: std::sync::Arc::clone(&release),
            completion: destructor_done_tx,
        });

        // Create the shutdown thread before the worker holds the GIL: Windows
        // thread startup can itself enter runtime initialization.
        let (shutdown_id_tx, shutdown_id_rx) = std::sync::mpsc::channel();
        let (shutdown_proceed_tx, shutdown_proceed_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (trace_tx, trace_rx) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            shutdown_id_tx.send(std::thread::current().id()).unwrap();
            shutdown_proceed_rx.recv().unwrap();
            done_tx
                .send(crate::state::runtime_state::molt_runtime_shutdown())
                .unwrap();
        });
        let shutdown_id = shutdown_id_rx.recv().unwrap();
        *crate::state::lifecycle::THREAD_LOCAL_DROP_TEST_TRACE
            .lock()
            .unwrap() = Some((shutdown_id, trace_tx));

        proceed_tx.send(()).unwrap();
        recorded_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("worker recorded its pending exception");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, exc_bits);
        });
        exit_tx.send(()).unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("worker entered ThreadExceptionState::drop");

        shutdown_proceed_tx.send(()).unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "shutdown must wait while TLS destructor validates and decrefs under GIL"
        );

        release.wait();
        destructor_done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("target exception TLS destructor completed its DECREF");
        assert_eq!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("shutdown completed after TLS destructor released the GIL"),
            1
        );
        let mut trace = Vec::new();
        while let Ok(stage) = trace_rx.recv_timeout(std::time::Duration::from_secs(2)) {
            trace.push(stage);
            if stage == "drained" || stage == "cleared" {
                break;
            }
        }
        assert_eq!(
            trace,
            vec![
                "enter",
                "gil_acquired",
                "runtime_absent",
                "gil_released",
                "drained"
            ],
            "shutdown thread TLS cleanup stages"
        );
        shutdown.join().unwrap();
        worker.join().unwrap();
        *crate::state::lifecycle::THREAD_LOCAL_DROP_TEST_TRACE
            .lock()
            .unwrap() = None;
        *super::state::THREAD_EXCEPTION_DROP_TEST_GATE
            .lock()
            .unwrap() = None;
    }
}
