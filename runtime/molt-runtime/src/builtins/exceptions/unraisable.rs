//! Canonical unraisable-exception transaction and reporting authority.

use super::*;
use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};
use crate::{
    alloc_property_obj, call_callable1, exception_materialize_traceback_bits,
    function_set_attr_bits, missing_bits, molt_get_attr_name, molt_is_callable, molt_module_import,
    molt_sys_stderr, object_class_bits, tuple_from_iter_bits,
};

const UNRAISABLE_FIELDS: [&str; 5] = [
    "exc_type",
    "exc_value",
    "exc_traceback",
    "err_msg",
    "object",
];

fn try_join_text(parts: &[&str]) -> Option<String> {
    let capacity = parts
        .iter()
        .try_fold(0_usize, |total, part| total.checked_add(part.len()))?;
    let mut out = String::new();
    out.try_reserve_exact(capacity).ok()?;
    for part in parts {
        out.push_str(part);
    }
    Some(out)
}

struct RaisedSnapshot {
    task: Option<(PtrSlot, u64)>,
    global: Option<u64>,
}

struct HandledSnapshot {
    active: Vec<u64>,
    fallback: Vec<u64>,
}

/// A transaction is required at every unraisable boundary. It preserves the
/// complete raised and handled channels while arbitrary reporting hooks run.
struct UnraisableTransaction {
    raised: RaisedSnapshot,
    handled: HandledSnapshot,
}

fn take_raised(_py: &PyToken<'_>) -> RaisedSnapshot {
    let state = runtime_state(_py);
    let task = current_task_key().and_then(|key| {
        if !state
            .task_last_exception_pending
            .load(AtomicOrdering::Relaxed)
        {
            return None;
        }
        let mut guard = task_last_exceptions(_py).lock().unwrap();
        let removed = guard.remove(&key);
        if guard.is_empty() {
            state
                .task_last_exception_pending
                .store(false, AtomicOrdering::Relaxed);
        }
        drop(guard);
        removed
            .filter(|slot| exception_slot_is_valid(*slot))
            .map(|slot| (key, MoltObject::from_ptr(slot.0).bits()))
    });
    let global = if state.last_exception_pending.load(AtomicOrdering::Acquire) {
        global_last_exception_take(_py)
            .filter(|slot| exception_slot_is_valid(*slot))
            .map(|slot| MoltObject::from_ptr(slot.0).bits())
    } else {
        None
    };
    RaisedSnapshot { task, global }
}

fn take_handled(_py: &PyToken<'_>) -> HandledSnapshot {
    let active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let fallback = ACTIVE_EXCEPTION_FALLBACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    HandledSnapshot { active, fallback }
}

fn release_stack(_py: &PyToken<'_>, stack: Vec<u64>) {
    for bits in stack {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

fn restore_handled(_py: &PyToken<'_>, saved: HandledSnapshot) {
    let old_active = ACTIVE_EXCEPTION_STACK
        .with(|stack| std::mem::replace(&mut *stack.borrow_mut(), saved.active));
    let old_fallback = ACTIVE_EXCEPTION_FALLBACK
        .with(|stack| std::mem::replace(&mut *stack.borrow_mut(), saved.fallback));
    release_stack(_py, old_active);
    release_stack(_py, old_fallback);
}

fn restore_raised(_py: &PyToken<'_>, saved: RaisedSnapshot) {
    if let Some(bits) = saved.global {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            let old = runtime_state(_py)
                .last_exception
                .swap(ptr, AtomicOrdering::AcqRel);
            runtime_state(_py)
                .last_exception_pending
                .store(true, AtomicOrdering::Release);
            if !old.is_null() && old != ptr {
                dec_ref_bits(_py, MoltObject::from_ptr(old).bits());
            }
        }
    }
    if let Some((key, bits)) = saved.task {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            let old = task_last_exceptions(_py)
                .lock()
                .unwrap()
                .insert(key, PtrSlot(ptr));
            runtime_state(_py)
                .task_last_exception_pending
                .store(true, AtomicOrdering::Relaxed);
            if let Some(old) = old
                && old.0 != ptr
            {
                dec_ref_bits(_py, MoltObject::from_ptr(old.0).bits());
            }
        }
    }
}

fn discard_current_raised(_py: &PyToken<'_>) {
    clear_exception(_py);
    clear_exception_state(_py);
}

impl UnraisableTransaction {
    fn begin(_py: &PyToken<'_>) -> Self {
        let handled = take_handled(_py);
        let raised = take_raised(_py);
        Self { raised, handled }
    }

    fn finish_current(self, _py: &PyToken<'_>, context_bits: u64, err_msg: Option<&str>) {
        let raised = take_raised(_py);
        self.finish_raised(_py, raised, context_bits, err_msg);
    }

    fn finish_raised(
        self,
        _py: &PyToken<'_>,
        raised: RaisedSnapshot,
        context_bits: u64,
        err_msg: Option<&str>,
    ) {
        match (raised.task, raised.global) {
            (Some((_, task_bits)), Some(global_bits)) if task_bits == global_bits => {
                report_unraisable_exception(_py, context_bits, task_bits, err_msg);
                discard_current_raised(_py);
                // Both raised channels owned one reference to the same object.
                dec_ref_bits(_py, task_bits);
                dec_ref_bits(_py, global_bits);
            }
            (Some((_, task_bits)), Some(global_bits)) => {
                report_unraisable_exception(_py, context_bits, task_bits, err_msg);
                discard_current_raised(_py);
                report_unraisable_exception(
                    _py,
                    context_bits,
                    global_bits,
                    Some(
                        "Invariant breach: distinct task and global exceptions at unraisable boundary",
                    ),
                );
                discard_current_raised(_py);
                dec_ref_bits(_py, task_bits);
                dec_ref_bits(_py, global_bits);
            }
            (Some((_, bits)), None) | (None, Some(bits)) => {
                report_unraisable_exception(_py, context_bits, bits, err_msg);
                discard_current_raised(_py);
                dec_ref_bits(_py, bits);
            }
            (None, None) => {}
        }
        restore_handled(_py, self.handled);
        restore_raised(_py, self.raised);
    }

    fn report_captured(
        self,
        _py: &PyToken<'_>,
        context_bits: u64,
        exc_bits: u64,
        err_msg: Option<&str>,
    ) {
        discard_current_raised(_py);
        if !obj_from_bits(exc_bits).is_none() {
            report_unraisable_exception(_py, context_bits, exc_bits, err_msg);
            discard_current_raised(_py);
        }
        restore_handled(_py, self.handled);
        restore_raised(_py, self.raised);
    }
}

pub(crate) fn run_unraisable<R>(
    _py: &PyToken<'_>,
    context_bits: u64,
    err_msg: Option<&str>,
    run: impl FnOnce() -> R,
) -> R {
    let transaction = UnraisableTransaction::begin(_py);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    transaction.finish_current(_py, context_bits, err_msg);
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(crate) fn run_unraisable_with_policy<R>(
    _py: &PyToken<'_>,
    policy: impl FnOnce() -> (u64, Option<String>),
    run: impl FnOnce() -> R,
) -> R {
    let transaction = UnraisableTransaction::begin(_py);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    let raised = take_raised(_py);
    let has_raised = raised.task.is_some() || raised.global.is_some();
    let policy_result = if has_raised {
        Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            policy,
        )))
    } else {
        None
    };
    let (context_bits, err_msg) = policy_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|(context, message)| (*context, message.as_deref()))
        .unwrap_or((MoltObject::none().bits(), None));
    transaction.finish_raised(_py, raised, context_bits, err_msg);
    let output = match result {
        Ok(output) => output,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    if let Some(Err(payload)) = policy_result {
        std::panic::resume_unwind(payload);
    }
    output
}

pub(crate) fn report_captured_unraisable(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) {
    UnraisableTransaction::begin(_py).report_captured(_py, context_bits, exc_bits, err_msg);
}

fn sys_attr_bits(_py: &PyToken<'_>, name: &[u8]) -> u64 {
    let sys_name_ptr = alloc_string(_py, b"sys");
    if sys_name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let sys_name_bits = MoltObject::from_ptr(sys_name_ptr).bits();
    let sys_bits = molt_module_import(sys_name_bits);
    dec_ref_bits(_py, sys_name_bits);
    if exception_pending(_py) || obj_from_bits(sys_bits).is_none() {
        discard_current_raised(_py);
        if !obj_from_bits(sys_bits).is_none() {
            dec_ref_bits(_py, sys_bits);
        }
        return MoltObject::none().bits();
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        dec_ref_bits(_py, sys_bits);
        return MoltObject::none().bits();
    };
    let value_bits = molt_get_attr_name(sys_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, sys_bits);
    if exception_pending(_py) || obj_from_bits(value_bits).is_none() {
        discard_current_raised(_py);
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return MoltObject::none().bits();
    }
    value_bits
}

pub(crate) fn context_repr(_py: &PyToken<'_>, context_bits: u64) -> String {
    if obj_from_bits(context_bits).is_none() {
        return "<callback>".to_string();
    }
    let repr_bits = crate::molt_repr_from_obj(context_bits);
    if exception_pending(_py) {
        discard_current_raised(_py);
        return "<callback>".to_string();
    }
    let rendered = crate::object::ops::string_obj_to_owned(obj_from_bits(repr_bits))
        .unwrap_or_else(|| "<callback>".to_string());
    if !obj_from_bits(repr_bits).is_none() {
        dec_ref_bits(_py, repr_bits);
    }
    rendered
}

fn set_class_attr(_py: &PyToken<'_>, class_ptr: *mut u8, name: &str, value_bits: u64) -> bool {
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return false;
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return false;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, value_bits) };
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn alloc_runtime_method(_py: &PyToken<'_>, fn_ptr: u64, arity: u64) -> u64 {
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
    if ptr.is_null() {
        return 0;
    }
    if !init_class_edge(_py, ptr, builtin_classes(_py).builtin_function_or_method) {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        return 0;
    }
    MoltObject::from_ptr(ptr).bits()
}

fn init_class_edge(_py: &PyToken<'_>, ptr: *mut u8, class_bits: u64) -> bool {
    unsafe { object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned) }
}

fn install_method(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    name: &str,
    fn_ptr: u64,
    arity: u64,
) -> bool {
    let func_bits = alloc_runtime_method(_py, fn_ptr, arity);
    if func_bits == 0 {
        return false;
    }
    let installed = set_class_attr(_py, class_ptr, name, func_bits);
    dec_ref_bits(_py, func_bits);
    installed
}

fn install_method_with_defaults(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    name: &str,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> bool {
    let func_bits = alloc_runtime_method(_py, fn_ptr, arity);
    let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
        return false;
    };
    let defaults_ptr = alloc_tuple(_py, defaults);
    if defaults_ptr.is_null() {
        dec_ref_bits(_py, func_bits);
        return false;
    }
    let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
    let defaults_name = intern_static_name(
        _py,
        &runtime_state(_py).interned.defaults_name,
        b"__defaults__",
    );
    unsafe { function_set_attr_bits(_py, func_ptr, defaults_name, defaults_bits) };
    dec_ref_bits(_py, defaults_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, func_bits);
        return false;
    }
    let installed = set_class_attr(_py, class_ptr, name, func_bits);
    dec_ref_bits(_py, func_bits);
    installed
}

fn install_readonly_field(_py: &PyToken<'_>, class_ptr: *mut u8, name: &str, getter: u64) -> bool {
    let getter_bits = alloc_runtime_method(_py, getter, 1);
    if getter_bits == 0 {
        return false;
    }
    let none = MoltObject::none().bits();
    let property_ptr = alloc_property_obj(_py, getter_bits, none, none);
    dec_ref_bits(_py, getter_bits);
    if property_ptr.is_null() {
        return false;
    }
    let property_bits = MoltObject::from_ptr(property_ptr).bits();
    let installed = set_class_attr(_py, class_ptr, name, property_bits);
    dec_ref_bits(_py, property_bits);
    installed
}

fn discard_unraisable_args_class(_py: &PyToken<'_>, class_bits: u64) -> u64 {
    if !obj_from_bits(class_bits).is_none() {
        class_break_cycles(_py, class_bits);
        dec_ref_bits(_py, class_bits);
    }
    0
}

fn build_unraisable_args_class(_py: &PyToken<'_>) -> u64 {
    let name_ptr = alloc_string(_py, b"UnraisableHookArgs");
    if name_ptr.is_null() {
        return 0;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    if class_ptr.is_null() {
        return 0;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    unsafe {
        if !crate::object::class_set_not_base(_py, class_ptr)
            || !crate::object::class_set_immutable(_py, class_ptr)
        {
            return discard_unraisable_args_class(_py, class_bits);
        }
    }
    let builtins = builtin_classes(_py);
    if !init_class_edge(_py, class_ptr, builtins.type_obj) {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let _ = molt_class_set_base(class_bits, builtins.tuple);
    if exception_pending(_py) {
        return discard_unraisable_args_class(_py, class_bits);
    }

    let module_ptr = alloc_string(_py, b"builtins");
    if module_ptr.is_null() {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();
    let module_set = set_class_attr(_py, class_ptr, "__module__", module_bits);
    dec_ref_bits(_py, module_bits);
    if !module_set
        || !set_class_attr(
            _py,
            class_ptr,
            "n_fields",
            MoltObject::from_int(UNRAISABLE_FIELDS.len() as i64).bits(),
        )
        || !set_class_attr(
            _py,
            class_ptr,
            "n_sequence_fields",
            MoltObject::from_int(UNRAISABLE_FIELDS.len() as i64).bits(),
        )
        || !set_class_attr(
            _py,
            class_ptr,
            "n_unnamed_fields",
            MoltObject::from_int(0).bits(),
        )
    {
        return discard_unraisable_args_class(_py, class_bits);
    }

    let none = MoltObject::none().bits();
    let mut match_args = [none; UNRAISABLE_FIELDS.len()];
    let mut initialized = 0;
    for (index, field) in UNRAISABLE_FIELDS.iter().enumerate() {
        let ptr = alloc_string(_py, field.as_bytes());
        if ptr.is_null() {
            for bits in &match_args[..initialized] {
                dec_ref_bits(_py, *bits);
            }
            return discard_unraisable_args_class(_py, class_bits);
        }
        match_args[index] = MoltObject::from_ptr(ptr).bits();
        initialized += 1;
    }
    let match_args_ptr = alloc_tuple(_py, &match_args);
    for bits in match_args {
        dec_ref_bits(_py, bits);
    }
    if match_args_ptr.is_null() {
        return discard_unraisable_args_class(_py, class_bits);
    }
    let match_args_bits = MoltObject::from_ptr(match_args_ptr).bits();
    let match_args_set = set_class_attr(_py, class_ptr, "__match_args__", match_args_bits);
    dec_ref_bits(_py, match_args_bits);
    if !match_args_set
        || !install_method_with_defaults(
            _py,
            class_ptr,
            "__new__",
            molt_unraisable_hook_args_new as *const () as usize as u64,
            3,
            &[missing_bits(_py)],
        )
        || !install_method(
            _py,
            class_ptr,
            "__repr__",
            molt_unraisable_hook_args_repr as *const () as usize as u64,
            1,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_type",
            molt_unraisable_hook_args_exc_type as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_value",
            molt_unraisable_hook_args_exc_value as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "exc_traceback",
            molt_unraisable_hook_args_exc_traceback as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "err_msg",
            molt_unraisable_hook_args_err_msg as *const () as usize as u64,
        )
        || !install_readonly_field(
            _py,
            class_ptr,
            "object",
            molt_unraisable_hook_args_object as *const () as usize as u64,
        )
    {
        return discard_unraisable_args_class(_py, class_bits);
    }
    class_bits
}

fn unraisable_args_class(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &exceptions_state(_py).unraisable_hook_args_class,
        || build_unraisable_args_class(_py),
    )
}

fn unraisable_args_is_exact(_py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    // Validation must not initialize a hidden type merely because user code
    // passed a forged object to sys.__unraisablehook__.
    let class_bits = exceptions_state(_py)
        .unraisable_hook_args_class
        .load(AtomicOrdering::Acquire);
    class_bits != 0
        && unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE
        && unsafe { object_class_bits(ptr) } == class_bits
}

/// Exact, unforgeable validator used by the Python-level default hook.
///
/// The hidden struct-sequence type is runtime-owned and intentionally absent
/// from `sys`; name/module/shape checks would let user classes spoof it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_unraisable_hook_args_is_exact(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(unraisable_args_is_exact(_py, bits)).bits()
    })
}

fn unraisable_args_field(_py: &PyToken<'_>, self_bits: u64, index: usize) -> u64 {
    let Some(ptr) = obj_from_bits(self_bits).as_ptr() else {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "UnraisableHookArgs descriptor requires an instance",
        );
    };
    if !unraisable_args_is_exact(_py, self_bits) {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "UnraisableHookArgs descriptor requires an instance",
        );
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    let Some(bits) = fields.get(index).copied() else {
        return raise_exception::<_>(_py, "RuntimeError", "invalid UnraisableHookArgs payload");
    };
    inc_ref_bits(_py, bits);
    bits
}

macro_rules! unraisable_field_getter {
    ($name:ident, $index:expr) => {
        extern "C" fn $name(self_bits: u64) -> u64 {
            crate::with_gil_entry_nopanic!(_py, { unraisable_args_field(_py, self_bits, $index) })
        }
    };
}

unraisable_field_getter!(molt_unraisable_hook_args_exc_type, 0);
unraisable_field_getter!(molt_unraisable_hook_args_exc_value, 1);
unraisable_field_getter!(molt_unraisable_hook_args_exc_traceback, 2);
unraisable_field_getter!(molt_unraisable_hook_args_err_msg, 3);
unraisable_field_getter!(molt_unraisable_hook_args_object, 4);

extern "C" fn molt_unraisable_hook_args_new(
    cls_bits: u64,
    sequence_bits: u64,
    fields_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if cls_bits != unraisable_args_class(_py) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "UnraisableHookArgs cannot be subclassed",
            );
        }
        if fields_bits != missing_bits(_py) {
            let Some(fields_ptr) = obj_from_bits(fields_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() takes a dict as second arg, if any",
                );
            };
            if unsafe { object_type_id(fields_ptr) } != TYPE_ID_DICT {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() takes a dict as second arg, if any",
                );
            }
            if !unsafe { dict_order(fields_ptr) }.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "UnraisableHookArgs() got duplicate or unexpected field name(s)",
                );
            }
        }
        let Some(source_bits) = (unsafe { tuple_from_iter_bits(_py, sequence_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let fields = unsafe { seq_vec_ref(source_ptr) };
        if fields.len() != UNRAISABLE_FIELDS.len() {
            let actual = fields.len();
            dec_ref_bits(_py, source_bits);
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!(
                    "UnraisableHookArgs() takes a {}-sequence ({}-sequence given)",
                    UNRAISABLE_FIELDS.len(),
                    actual
                ),
            );
        }
        let fields = [fields[0], fields[1], fields[2], fields[3], fields[4]];
        // The source tuple may be the sole owner of one or more heap fields.
        // Retain them into the new tuple before releasing the source edge.
        let ptr = alloc_tuple(_py, &fields);
        dec_ref_bits(_py, source_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if !init_class_edge(_py, ptr, cls_bits) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

extern "C" fn molt_unraisable_hook_args_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if !unraisable_args_is_exact(_py, self_bits) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "UnraisableHookArgs.__repr__ requires an instance",
            );
        }
        let fields = unsafe { seq_vec_ref(ptr) };
        if fields.len() != UNRAISABLE_FIELDS.len() {
            return raise_exception::<_>(_py, "RuntimeError", "invalid UnraisableHookArgs payload");
        }
        let mut rendered = String::new();
        if rendered.try_reserve_exact(96).is_err() {
            return raise_exception::<_>(
                _py,
                "MemoryError",
                "UnraisableHookArgs repr allocation failed",
            );
        }
        rendered.push_str("UnraisableHookArgs(");
        for (index, (name, bits)) in UNRAISABLE_FIELDS.iter().zip(fields.iter()).enumerate() {
            let punctuation = if index == 0 { 1 } else { 3 };
            let reserve = name.len().checked_add(punctuation);
            if reserve.is_none_or(|reserve| rendered.try_reserve(reserve).is_err()) {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "UnraisableHookArgs repr allocation failed",
                );
            }
            if index != 0 {
                rendered.push_str(", ");
            }
            rendered.push_str(name);
            rendered.push('=');
            let repr_bits = crate::molt_repr_from_obj(*bits);
            if exception_pending(_py) || obj_from_bits(repr_bits).is_none() {
                if !obj_from_bits(repr_bits).is_none() {
                    dec_ref_bits(_py, repr_bits);
                }
                return MoltObject::none().bits();
            }
            let Some(item) = string_obj_to_owned(obj_from_bits(repr_bits)) else {
                dec_ref_bits(_py, repr_bits);
                return raise_exception::<_>(_py, "TypeError", "__repr__ returned non-string");
            };
            dec_ref_bits(_py, repr_bits);
            if rendered.try_reserve(item.len()).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "UnraisableHookArgs repr allocation failed",
                );
            }
            rendered.push_str(&item);
        }
        rendered.push(')');
        let out = alloc_string(_py, rendered.as_bytes());
        if out.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out).bits()
        }
    })
}

fn alloc_unraisable_hook_args(_py: &PyToken<'_>, fields: &[u64; 5]) -> u64 {
    let class_bits = unraisable_args_class(_py);
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return MoltObject::none().bits();
    }
    let ptr = alloc_tuple(_py, fields);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    if !init_class_edge(_py, ptr, class_bits) {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn build_hook_args(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) -> u64 {
    let msg_bits = if let Some(err_msg) = err_msg {
        let msg_ptr = alloc_string(_py, err_msg.as_bytes());
        if msg_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(msg_ptr).bits()
    } else {
        MoltObject::none().bits()
    };
    let mut exc_type_bits = MoltObject::none().bits();
    let mut trace_bits = MoltObject::none().bits();
    if let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr()
        && unsafe { object_type_id(exc_ptr) } == TYPE_ID_EXCEPTION
    {
        let class = unsafe { object_class_bits(exc_ptr) };
        if !obj_from_bits(class).is_none() {
            exc_type_bits = class;
        }
        let trace = exception_materialize_traceback_bits(_py, exc_ptr);
        if !obj_from_bits(trace).is_none() {
            // Borrowed from the exception payload; the arguments tuple retains it.
            trace_bits = trace;
        }
    }
    let out = alloc_unraisable_hook_args(
        _py,
        &[exc_type_bits, exc_bits, trace_bits, msg_bits, context_bits],
    );
    if !obj_from_bits(msg_bits).is_none() {
        dec_ref_bits(_py, msg_bits);
    }
    if exception_pending(_py) {
        discard_current_raised(_py);
        if !obj_from_bits(out).is_none() {
            dec_ref_bits(_py, out);
        }
        return MoltObject::none().bits();
    }
    out
}

enum HookOutcome {
    Handled,
    Unavailable,
    Raised,
}

fn try_hook(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) -> HookOutcome {
    let hook_bits = sys_attr_bits(_py, b"unraisablehook");
    if obj_from_bits(hook_bits).is_none()
        || !is_truthy(_py, obj_from_bits(molt_is_callable(hook_bits)))
    {
        if !obj_from_bits(hook_bits).is_none() {
            dec_ref_bits(_py, hook_bits);
        }
        return HookOutcome::Unavailable;
    }
    let args_bits = build_hook_args(_py, context_bits, exc_bits, err_msg);
    if obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, hook_bits);
        return HookOutcome::Unavailable;
    }
    let out = unsafe { call_callable1(_py, hook_bits, args_bits) };
    let hook_exc_bits = exception_last_bits_noinc(_py)
        .filter(|bits| exception_pending(_py) && !obj_from_bits(*bits).is_none());
    if let Some(bits) = hook_exc_bits {
        inc_ref_bits(_py, bits);
    }
    discard_current_raised(_py);
    if !obj_from_bits(out).is_none() {
        dec_ref_bits(_py, out);
    }
    dec_ref_bits(_py, args_bits);
    if let Some(bits) = hook_exc_bits {
        let hook_text = context_repr(_py, hook_bits);
        let formatted = obj_from_bits(bits)
            .as_ptr()
            .map(|ptr| format_exception_with_traceback(_py, ptr))
            .unwrap_or_default();
        if let Some(line) =
            try_join_text(&["Exception ignored in sys.unraisablehook: ", &hook_text])
        {
            write_stderr_line(_py, &line);
        }
        if !formatted.is_empty() {
            write_stderr_line(_py, &formatted);
        }
        dec_ref_bits(_py, bits);
        dec_ref_bits(_py, hook_bits);
        HookOutcome::Raised
    } else {
        dec_ref_bits(_py, hook_bits);
        HookOutcome::Handled
    }
}

fn write_stderr_line(_py: &PyToken<'_>, text: &str) {
    let text_ptr = alloc_string(_py, text.as_bytes());
    if text_ptr.is_null() {
        return;
    }
    let text_bits = MoltObject::from_ptr(text_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[text_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, text_bits);
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let stderr_bits = sys_attr_bits(_py, b"stderr");
    let stderr_bits = if obj_from_bits(stderr_bits).is_none() {
        molt_sys_stderr()
    } else {
        stderr_bits
    };
    if !obj_from_bits(stderr_bits).is_none() {
        let none = MoltObject::none().bits();
        let out = crate::molt_print_builtin(
            args_bits,
            none,
            none,
            stderr_bits,
            MoltObject::from_bool(true).bits(),
        );
        if !obj_from_bits(out).is_none() {
            dec_ref_bits(_py, out);
        }
        discard_current_raised(_py);
        dec_ref_bits(_py, stderr_bits);
    }
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, text_bits);
}

fn default_unraisable_prefix(
    context_is_none: bool,
    context_text: &str,
    err_msg: Option<&str>,
) -> Option<String> {
    match (err_msg, context_is_none) {
        (Some(message), true) => try_join_text(&[message, ":"]),
        (Some(message), false) => try_join_text(&[message, ": ", context_text]),
        (None, true) => None,
        (None, false) => try_join_text(&["Exception ignored in: ", context_text]),
    }
}

fn report_unraisable_exception(
    _py: &PyToken<'_>,
    context_bits: u64,
    exc_bits: u64,
    err_msg: Option<&str>,
) {
    match try_hook(_py, context_bits, exc_bits, err_msg) {
        HookOutcome::Handled | HookOutcome::Raised => return,
        HookOutcome::Unavailable => {}
    }
    let context_text = context_repr(_py, context_bits);
    let formatted = obj_from_bits(exc_bits)
        .as_ptr()
        .map(|ptr| format_exception_with_traceback(_py, ptr))
        .unwrap_or_default();
    if let Some(prefix) = default_unraisable_prefix(
        obj_from_bits(context_bits).is_none(),
        &context_text,
        err_msg,
    ) {
        write_stderr_line(_py, &prefix);
    }
    if !formatted.is_empty() {
        write_stderr_line(_py, &formatted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_dict_value(_py: &PyToken<'_>, class_bits: u64, name: &str) -> u64 {
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class pointer");
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        let dict_ptr = obj_from_bits(dict_bits).as_ptr().expect("class dictionary");
        let name_ptr = alloc_string(_py, name.as_bytes());
        assert!(!name_ptr.is_null(), "attribute name allocation");
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value = unsafe { dict_get_in_place(_py, dict_ptr, name_bits) }
            .unwrap_or_else(|| panic!("missing class attribute {name}"));
        dec_ref_bits(_py, name_bits);
        value
    }

    #[test]
    fn unraisable_args_are_an_internal_immutable_structured_tuple() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let none = MoltObject::none().bits();
            let fields = [
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                none,
                MoltObject::from_int(4).bits(),
                MoltObject::from_bool(true).bits(),
            ];
            let args_bits = alloc_unraisable_hook_args(_py, &fields);
            assert!(unraisable_args_is_exact(_py, args_bits));
            assert!(!unraisable_args_is_exact(_py, none));
            assert_eq!(
                obj_from_bits(molt_unraisable_hook_args_is_exact(args_bits)).as_bool(),
                Some(true)
            );
            assert_eq!(
                obj_from_bits(molt_unraisable_hook_args_is_exact(none)).as_bool(),
                Some(false)
            );

            let args_ptr = obj_from_bits(args_bits).as_ptr().expect("args pointer");
            assert_eq!(unsafe { object_type_id(args_ptr) }, TYPE_ID_TUPLE);
            assert_eq!(unsafe { seq_vec_ref(args_ptr) }, &fields);
            assert!(isinstance_bits(_py, args_bits, builtin_classes(_py).tuple));
            assert_eq!(unraisable_args_field(_py, args_bits, 1), fields[1]);

            let field_name_ptr = alloc_string(_py, b"exc_value");
            assert!(!field_name_ptr.is_null());
            let field_name_bits = MoltObject::from_ptr(field_name_ptr).bits();
            let field_bits = molt_get_attr_name(args_bits, field_name_bits);
            assert_eq!(field_bits, fields[1]);
            dec_ref_bits(_py, field_bits);
            dec_ref_bits(_py, field_name_bits);

            let class_bits = unsafe { object_class_bits(args_ptr) };
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class pointer");
            assert!(
                unsafe { crate::object::class_is_not_base(_py, class_ptr) },
                "the hidden structured type must not be an acceptable base"
            );
            assert!(
                unsafe { crate::object::class_is_immutable(_py, class_ptr) },
                "the hidden structured type must use generic immutable-type metadata"
            );
            let class_refs_before = unsafe {
                (*header_from_obj_ptr(class_ptr))
                    .ref_count
                    .load(AtomicOrdering::Acquire)
            };
            let second_args_bits = alloc_unraisable_hook_args(_py, &fields);
            let class_refs_with_second = unsafe {
                (*header_from_obj_ptr(class_ptr))
                    .ref_count
                    .load(AtomicOrdering::Acquire)
            };
            assert_eq!(class_refs_with_second, class_refs_before + 1);
            dec_ref_bits(_py, second_args_bits);
            assert_eq!(
                unsafe {
                    (*header_from_obj_ptr(class_ptr))
                        .ref_count
                        .load(AtomicOrdering::Acquire)
                },
                class_refs_before,
                "each structured tuple owns and releases exactly one class edge"
            );
            assert_eq!(
                string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) })),
                Some("UnraisableHookArgs".to_string())
            );
            assert_eq!(
                string_obj_to_owned(obj_from_bits(class_dict_value(
                    _py,
                    class_bits,
                    "__module__"
                ))),
                Some("builtins".to_string())
            );
            assert_eq!(
                obj_from_bits(class_dict_value(_py, class_bits, "n_fields")).as_int(),
                Some(5)
            );

            let repr_name_ptr = alloc_string(_py, b"__repr__");
            assert!(!repr_name_ptr.is_null());
            let repr_name_bits = MoltObject::from_ptr(repr_name_ptr).bits();
            let class_mutation =
                crate::molt_set_attr_name(class_bits, repr_name_bits, MoltObject::none().bits());
            assert_eq!(class_mutation, 0, "setattr errors use the zero sentinel");
            assert!(
                exception_pending(_py),
                "the hidden runtime type itself must be immutable"
            );
            clear_exception(_py);
            clear_exception_state(_py);
            dec_ref_bits(_py, repr_name_bits);

            let source_ptr = alloc_tuple(_py, &fields);
            assert!(!source_ptr.is_null());
            let source_bits = MoltObject::from_ptr(source_ptr).bits();
            let constructed_bits =
                molt_unraisable_hook_args_new(class_bits, source_bits, missing_bits(_py));
            assert!(unraisable_args_is_exact(_py, constructed_bits));
            assert_ne!(constructed_bits, source_bits);
            dec_ref_bits(_py, constructed_bits);
            dec_ref_bits(_py, source_bits);

            let repr_bits = molt_unraisable_hook_args_repr(args_bits);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(repr_bits)),
                Some(
                    "UnraisableHookArgs(exc_type=1, exc_value=2, exc_traceback=None, err_msg=4, object=True)"
                        .to_string()
                )
            );
            dec_ref_bits(_py, repr_bits);

            let err_name_ptr = alloc_string(_py, b"err_msg");
            assert!(!err_name_ptr.is_null());
            let err_name_bits = MoltObject::from_ptr(err_name_ptr).bits();
            let result = crate::molt_set_attr_name(
                args_bits,
                err_name_bits,
                MoltObject::from_int(99).bits(),
            );
            assert_eq!(result, 0, "setattr errors use the zero sentinel");
            assert!(
                exception_pending(_py),
                "structured tuple fields are readonly"
            );
            clear_exception(_py);
            clear_exception_state(_py);
            dec_ref_bits(_py, err_name_bits);

            dec_ref_bits(_py, args_bits);
        });
    }

    #[test]
    fn default_hook_prefix_matches_cpython_punctuation() {
        assert_eq!(default_unraisable_prefix(true, "ignored", None), None);
        assert_eq!(
            default_unraisable_prefix(false, "callback", None),
            Some("Exception ignored in: callback".to_string())
        );
        assert_eq!(
            default_unraisable_prefix(true, "ignored", Some("message")),
            Some("message:".to_string())
        );
        assert_eq!(
            default_unraisable_prefix(false, "callback", Some("message")),
            Some("message: callback".to_string())
        );
    }
}
