macro_rules! fn_addr {
    ($func:path) => {
        $func as *const () as usize as u64
    };
}

use molt_obj_model::MoltObject;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use crate::PyToken;

use crate::{
    alloc_class_obj, alloc_dict_with_pairs, alloc_object, alloc_string, alloc_tuple,
    alloc_instance_for_class, builtin_classes, builtin_func_bits, call_class_init_with_args,
    class_break_cycles, class_name_bits, code_filename_bits, code_firstlineno, code_name_bits,
    context_stack_unwind, current_task_key, current_token_id, dec_ref_bits, dict_get_in_place,
    dict_set_in_place, format_obj, format_obj_str, inc_ref_bits, instance_dict_bits,
    instance_set_dict_bits, intern_static_name, is_truthy, issubclass_bits, module_dict_bits,
    molt_class_set_base, molt_dec_ref, molt_iter_checked, molt_iter_next, molt_repr_from_obj,
    molt_str_from_obj, obj_from_bits, object_mark_has_ptrs, object_type_id, runtime_state,
    seq_vec_ref, string_obj_to_owned, task_exception_depths, task_exception_handler_stacks,
    task_exception_stacks, task_last_exceptions, to_i64, token_is_cancelled, type_name,
    MoltHeader, PtrSlot, RuntimeState, FRAME_STACK, TYPE_ID_CODE, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE,
};
use crate::builtins::frames::FrameEntry;

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
    pub(crate) static EXCEPTION_STACK: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    pub(crate) static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static GENERATOR_EXCEPTION_STACKS: RefCell<HashMap<usize, Vec<u64>>> =
        RefCell::new(HashMap::new());
    pub(crate) static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
    pub(crate) static TASK_RAISE_ACTIVE: Cell<bool> = const { Cell::new(false) };
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
}

use internals::{
    exception_type_cache, module_cache, CHARACTERS_WRITTEN_ATTR_NAME, ERRNO_ATTR_NAME,
    FILENAME_ATTR_NAME, STRERROR_ATTR_NAME,
};

pub(crate) fn exception_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__init__" => Some(builtin_func_bits(_py,
            &runtime_state(_py).method_cache.exception_init,
            fn_addr!(molt_exception_init),
            2,
        )),
        "__new__" => Some(builtin_func_bits(_py,
            &runtime_state(_py).method_cache.exception_new,
            fn_addr!(molt_exception_new_bound),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn raise_exception<T: ExceptionSentinel>(_py: &PyToken<'_>, kind: &str, message: &str) -> T {
    let ptr = alloc_exception(_py, kind, message);
    if !ptr.is_null() {
        record_exception(_py, ptr);
    }
    if !exception_handler_active() && !generator_raise_active() && !task_raise_active() {
        if kind == "SystemExit" && !ptr.is_null() {
            handle_system_exit(_py, ptr);
        }
        let exc_bits = if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        };
        context_stack_unwind(_py, exc_bits);
        eprintln!("{kind}: {message}");
        std::process::exit(1);
    }
    T::exception_sentinel()
}

pub(crate) fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken<'_>, bits: u64) -> T {
    let msg = format!(
        "'{}' object is not iterable",
        type_name(_py, obj_from_bits(bits))
    );
    raise_exception::<T>(_py, "TypeError", &msg)
}

pub(crate) fn raise_key_error_with_key<T: ExceptionSentinel>(_py: &PyToken<'_>, key_bits: u64) -> T {
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
        }
    }
    dec_ref_bits(_py, kind_bits);
    dec_ref_bits(_py, msg_bits);
    dec_ref_bits(_py, args_bits);
    ptr
}

pub(crate) fn alloc_exception_obj(_py: &PyToken<'_>,
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
    if let Some(task_key) = current_task_key() {
        let guard = task_last_exceptions(_py).lock().unwrap();
        if guard.contains_key(&task_key) {
            return true;
        }
        drop(guard);
        return runtime_state(_py).last_exception.lock().unwrap().is_some();
    }
    runtime_state(_py).last_exception.lock().unwrap().is_some()
}

pub(crate) fn clear_exception_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    let ptr = {
        let mut guard = runtime_state(_py).last_exception.lock().unwrap();
        guard.take()
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
    EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(0);
    });
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(MoltObject::none().bits());
    });
}

pub(crate) fn exception_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
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
            stack.push(1);
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

pub(crate) fn task_exception_handler_stack_take(
    _py: &PyToken<'_>,
    ptr: *mut u8,
) -> Vec<u8> {
    task_exception_handler_stacks(_py)
        .lock()
        .unwrap()
        .remove(&PtrSlot(ptr))
        .unwrap_or_default()
}

pub(crate) fn task_exception_handler_stack_store(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    stack: Vec<u8>,
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

pub(crate) fn task_last_exception_drop(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if let Some(old_ptr) = task_last_exceptions(_py).lock().unwrap().remove(&PtrSlot(ptr)) {
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn record_exception(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    let task_key = current_task_key();
    let mut prior_ptr = None;
    let mut context_bits: Option<u64> = None;
    let mut same_ptr = false;
    if let Some(task_key) = task_key {
        if let Some(old_ptr) = task_last_exceptions(_py).lock().unwrap().remove(&task_key) {
            prior_ptr = Some(old_ptr.0);
        }
    } else {
        let mut guard = runtime_state(_py).last_exception.lock().unwrap();
        if let Some(old_ptr) = guard.take() {
            prior_ptr = Some(old_ptr.0);
        }
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
    if let Some(new_bits) = frame_stack_trace_bits(_py) {
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
    if let Some(task_key) = task_key {
        task_last_exceptions(_py)
            .lock()
            .unwrap()
            .insert(task_key, PtrSlot(ptr));
    } else {
        let mut guard = runtime_state(_py).last_exception.lock().unwrap();
        *guard = Some(PtrSlot(ptr));
    }
    let new_bits = MoltObject::from_ptr(ptr).bits();
    if !same_ptr {
        inc_ref_bits(_py, new_bits);
    }
}

pub(crate) fn clear_exception(_py: &PyToken<'_>) {
    crate::gil_assert();
    if let Some(task_key) = current_task_key() {
        if let Some(old_ptr) = task_last_exceptions(_py).lock().unwrap().remove(&task_key) {
            let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
            dec_ref_bits(_py, old_bits);
        }
        return;
    }
    let mut guard = runtime_state(_py).last_exception.lock().unwrap();
    if let Some(old_ptr) = guard.take() {
        let old_bits = MoltObject::from_ptr(old_ptr.0).bits();
        dec_ref_bits(_py, old_bits);
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
        let value_bits = dict_get_in_place(_py, dict_ptr, name_bits);
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

pub(crate) fn exception_type_bits_from_name(_py: &PyToken<'_>, name: &str) -> u64 {
    let builtins = builtin_classes(_py);
    match name {
        "Exception" => return builtins.exception,
        "BaseException" => return builtins.base_exception,
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
    class_bits
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
            if is_truthy(obj_from_bits(pair[1])) {
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

pub(crate) unsafe fn exception_store_args_and_message(_py: &PyToken<'_>, ptr: *mut u8, args_bits: u64, msg_bits: u64) {
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

pub(crate) unsafe fn exception_set_stop_iteration_value(_py: &PyToken<'_>, ptr: *mut u8, args_bits: u64) {
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
    if errno == libc::EPIPE as i64 || errno == libc::ESHUTDOWN as i64 {
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn raise_os_error_errno<T: ExceptionSentinel>(_py: &PyToken<'_>, errno: i64, message: &str) -> T {
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
        record_exception(_py, ptr);
    }
    T::exception_sentinel()
}

#[cfg(not(target_arch = "wasm32"))]
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

unsafe fn oserror_attr_dict(_py: &PyToken<'_>, errno_val: Option<i64>, strerror_bits: u64, filename_bits: u64) -> u64 {
    let errno_name = intern_static_name(_py, &ERRNO_ATTR_NAME, b"errno");
    let strerror_name = intern_static_name(_py, &STRERROR_ATTR_NAME, b"strerror");
    let filename_name = intern_static_name(_py, &FILENAME_ATTR_NAME, b"filename");
    let errno_bits = match errno_val {
        Some(val) => MoltObject::from_int(val).bits(),
        None => MoltObject::none().bits(),
    };
    let dict_ptr = alloc_dict_with_pairs(_py, &[
        errno_name,
        errno_bits,
        strerror_name,
        strerror_bits,
        filename_name,
        filename_bits,
    ]);
    if dict_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(dict_ptr).bits()
}

pub(crate) fn alloc_exception_from_class_bits(_py: &PyToken<'_>, class_bits: u64, args_bits: u64) -> *mut u8 {
    // TODO(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:partial): parse subclass-specific args (UnicodeError fields, ExceptionGroup tree) into dedicated attributes.
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
                    let name_bits =
                        intern_static_name(_py, &CHARACTERS_WRITTEN_ATTR_NAME, b"characters_written");
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            dict_set_in_place(_py, dict_ptr, name_bits, chars_bits);
                        }
                    }
                }
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
    let args = exception_args_vec(ptr);
    if args.is_empty() {
        return String::new();
    }
    let kind = exception_class_name(ptr);
    if kind == "KeyError" && args.len() == 1 {
        return format_obj(_py, obj_from_bits(args[0]));
    }
    if args.len() == 1 {
        return format_obj_str(_py, obj_from_bits(args[0]));
    }
    format_obj_str(_py, obj_from_bits(unsafe { exception_args_bits(ptr) }))
}

fn format_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let trace_bits = unsafe { exception_trace_bits(ptr) };
    if obj_from_bits(trace_bits).is_none() {
        return None;
    }
    let mut out = String::from("Traceback (most recent call last):\n");
    let tb_frame_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_lineno_name, b"tb_lineno");
    let tb_next_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits = intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
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

unsafe fn alloc_frame_obj(_py: &PyToken<'_>, code_bits: u64, line: i64) -> Option<u64> {
    // TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): add full frame fields (f_back, f_globals, f_locals) and keep f_lasti/f_lineno live-updated.
    let builtins = builtin_classes(_py);
    let class_obj = obj_from_bits(builtins.frame);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let frame_bits = alloc_instance_for_class(_py, class_ptr);
    let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits = intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let f_lasti_bits = intern_static_name(_py, &runtime_state(_py).interned.f_lasti_name, b"f_lasti");
    let line_bits = MoltObject::from_int(line).bits();
    let lasti_bits = MoltObject::from_int(-1).bits();
    let dict_ptr = alloc_dict_with_pairs(_py, &[
        f_code_bits,
        code_bits,
        f_lineno_bits,
        line_bits,
        f_lasti_bits,
        lasti_bits,
    ]);
    if dict_ptr.is_null() {
        dec_ref_bits(_py, frame_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(_py, frame_ptr, dict_bits);
    object_mark_has_ptrs(_py, frame_ptr);
    Some(frame_bits)
}

unsafe fn alloc_traceback_obj(_py: &PyToken<'_>, frame_bits: u64, line: i64, next_bits: u64) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let class_obj = obj_from_bits(builtins.traceback);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let tb_bits = alloc_instance_for_class(_py, class_ptr);
    let tb_ptr = obj_from_bits(tb_bits).as_ptr()?;
    let tb_frame_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_lineno_name, b"tb_lineno");
    let tb_next_bits = intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let line_bits = MoltObject::from_int(line).bits();
    let dict_ptr = alloc_dict_with_pairs(_py, &[
        tb_frame_bits,
        frame_bits,
        tb_lineno_bits,
        line_bits,
        tb_next_bits,
        next_bits,
    ]);
    if dict_ptr.is_null() {
        dec_ref_bits(_py, tb_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(_py, tb_ptr, dict_bits);
    object_mark_has_ptrs(_py, tb_ptr);
    Some(tb_bits)
}

pub(crate) fn frame_stack_trace_bits(_py: &PyToken<'_>) -> Option<u64> {
    let entries = FRAME_STACK.with(|stack| stack.borrow().clone());
    if entries.is_empty() {
        return None;
    }
    let mut next_bits = MoltObject::none().bits();
    let mut built_any = false;
    for entry in entries.into_iter().rev() {
        if entry.code_bits == 0 {
            continue;
        }
        let Some(code_ptr) = obj_from_bits(entry.code_bits).as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                continue;
            }
            let mut line = entry.line;
            if line <= 0 {
                line = code_firstlineno(code_ptr);
            }
            let Some(frame_bits) = alloc_frame_obj(_py, entry.code_bits, line) else {
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(_py, next_bits);
                }
                return None;
            };
            let Some(tb_bits) = alloc_traceback_obj(_py, frame_bits, line, next_bits) else {
                dec_ref_bits(_py, frame_bits);
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(_py, next_bits);
                }
                return None;
            };
            dec_ref_bits(_py, frame_bits);
            if !obj_from_bits(next_bits).is_none() {
                dec_ref_bits(_py, next_bits);
            }
            next_bits = tb_bits;
            built_any = true;
        }
    }
    if !built_any || obj_from_bits(next_bits).is_none() {
        if !obj_from_bits(next_bits).is_none() {
            dec_ref_bits(_py, next_bits);
        }
        return None;
    }
    Some(next_bits)
}

#[no_mangle]
pub extern "C" fn molt_exception_new(kind_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    let kind_obj = obj_from_bits(kind_bits);
    if let Some(ptr) = kind_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                return raise_exception::<u64>(_py, "TypeError", "exception kind must be a str");
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
        return raise_exception::<u64>(_py, "TypeError", "exceptions must derive from BaseException");
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
        return raise_exception::<u64>(_py, "TypeError", "exception init expects exception instance");
    };
    unsafe {
        if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
            return raise_exception::<u64>(_py,
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
        unsafe {
            inc_ref_bits(_py, norm_bits);
            inc_ref_bits(_py, msg_bits);
            exception_store_args_and_message(_py, self_ptr, norm_bits, msg_bits);
            exception_set_stop_iteration_value(_py, self_ptr, norm_bits);
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
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
                        let errno_name = intern_static_name(_py, &internals::ERRNO_ATTR_NAME, b"errno");
                        let strerror_name =
                            intern_static_name(_py, &internals::STRERROR_ATTR_NAME, b"strerror");
                        let filename_name =
                            intern_static_name(_py, &internals::FILENAME_ATTR_NAME, b"filename");
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
    }
    dec_ref_bits(_py, norm_bits);
    dec_ref_bits(_py, msg_bits);
    if !obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, args_bits);
    }
    MoltObject::none().bits()

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
            let ptr = 0x1 as *mut u8;
            let bits = vec![MoltObject::none().bits(), MoltObject::none().bits()];
            generator_exception_stack_store(ptr, bits);
            generator_exception_stack_drop(_py, ptr);
            let after = generator_exception_stack_take(ptr);
            assert!(
                after.is_empty(),
                "generator exception stack should be cleared on drop"
            );
        });
    }

    #[test]
    fn task_exception_stack_drop_clears_entries() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let ptr = 0x2 as *mut u8;
            let bits = vec![MoltObject::none().bits()];
            task_exception_stack_store(_py, ptr, bits);
            task_exception_stack_drop(_py, ptr);
            let after = task_exception_stack_take(_py, ptr);
            assert!(after.is_empty(), "task exception stack should be cleared");
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
            return raise_exception::<u64>(_py,
                "TypeError",
                "exception cause must be an exception or None",
            );
        };
        unsafe {
            if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py,
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
    record_exception(_py, ptr);
    MoltObject::none().bits()

    })
}

#[no_mangle]
pub extern "C" fn molt_exception_last() -> u64 {
    crate::with_gil_entry!(_py, {
    if let Some(task_key) = current_task_key() {
        let guard = task_last_exceptions(_py).lock().unwrap();
        if let Some(ptr) = guard.get(&task_key).copied() {
            let bits = MoltObject::from_ptr(ptr.0).bits();
            inc_ref_bits(_py, bits);
            return bits;
        }
        drop(guard);
        let guard = runtime_state(_py).last_exception.lock().unwrap();
        if let Some(ptr) = *guard {
            let bits = MoltObject::from_ptr(ptr.0).bits();
            inc_ref_bits(_py, bits);
            return bits;
        }
        return MoltObject::none().bits();
    }
    let guard = runtime_state(_py).last_exception.lock().unwrap();
    if let Some(ptr) = *guard {
        let bits = MoltObject::from_ptr(ptr.0).bits();
        inc_ref_bits(_py, bits);
        return bits;
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
    clear_exception(_py);
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
pub extern "C" fn molt_raise(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        return raise_exception::<u64>(_py, "TypeError", "exceptions must derive from BaseException");
    };
    let mut exc_ptr = ptr;
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_EXCEPTION => {}
            TYPE_ID_TYPE => {
                let class_bits = MoltObject::from_ptr(ptr).bits();
                if !issubclass_bits(class_bits, builtin_classes(_py).base_exception) {
                    return raise_exception::<u64>(_py,
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
                    return raise_exception::<u64>(_py,
                        "TypeError",
                        "exceptions must derive from BaseException",
                    );
                }
                exc_ptr = inst_ptr;
            }
            _ => {
                return raise_exception::<u64>(_py,
                    "TypeError",
                    "exceptions must derive from BaseException",
                );
            }
        }
    }
    record_exception(_py, exc_ptr);
    if !exception_handler_active() && !generator_raise_active() && !task_raise_active() {
        let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
        if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("SystemExit") {
            handle_system_exit(_py, exc_ptr);
        }
        context_stack_unwind(_py, MoltObject::from_ptr(exc_ptr).bits());
        eprintln!("{}", format_exception_with_traceback(_py, exc_ptr));
        std::process::exit(1);
    }
    MoltObject::none().bits()

    })
}
