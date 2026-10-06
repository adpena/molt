use crate::PyToken;
use crate::builtins::exceptions::{
    exception_layout_kind_for_class, exception_typed_fields_replace_internal, molt_exception_init,
    molt_exception_init_owned, molt_exception_new_bound, molt_exceptiongroup_init,
    record_memory_error_without_allocation,
};
use crate::call::type_policy::{
    InitArgPolicy, callable_matches_runtime_symbol, resolved_constructor_init_policy,
    resolved_new_is_default_object_new,
};
use crate::*;
use molt_obj_model::ExceptionTypedField;

/// Allocation reads the concrete size sealed with the physical row record.
/// Construction is the only fallible path; there is no ancestor sizing lane.
pub(crate) unsafe fn class_layout_size_cached(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> Option<usize> {
    unsafe {
        crate::object::class_finish_definition(_py, class_ptr).ok()?;
        crate::object::layout::class_cached_layout_size(class_ptr)
    }
}

pub(crate) unsafe fn alloc_published_instance_for_class_with_total_size(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    total_size: usize,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let Some(payload) = total_size.checked_sub(std::mem::size_of::<MoltHeader>()) else {
            record_memory_error_without_allocation(_py);
            return MoltObject::none().bits();
        };
        let bits = crate::object::builders::alloc_class_instance(_py, payload, class_bits);
        let Some(obj_ptr) = obj_from_bits(bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        crate::object::gc::gc_publish_initialized(_py, obj_ptr);
        bits
    }
}

pub(crate) unsafe fn alloc_instance_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> u64 {
    // Default type construction and explicit object.__new__ converge here.
    // A plain class allocation cannot supply the wrapper's native poll payload.
    if crate::builtins::classes::builtin_classes_if_initialized(_py)
        .is_some_and(|classes| MoltObject::from_ptr(class_ptr).bits() == classes.coroutine_wrapper)
    {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "cannot create 'coroutine_wrapper' instances",
        );
    }
    unsafe {
        if crate::object::class_finish_definition(_py, class_ptr).is_err() {
            return MoltObject::none().bits();
        }
        let Some(payload_size) = class_layout_size_cached(_py, class_ptr) else {
            return MoltObject::none().bits();
        };
        let Some(total_size) = payload_size.checked_add(std::mem::size_of::<MoltHeader>()) else {
            record_memory_error_without_allocation(_py);
            return MoltObject::none().bits();
        };
        alloc_published_instance_for_class_with_total_size(_py, class_ptr, total_size)
    }
}

pub(crate) unsafe fn alloc_instance_for_default_object_new(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> u64 {
    // Every default object allocation, including dataclasses, shares admission.
    crate::molt_object_new_bound(MoltObject::from_ptr(class_ptr).bits())
}

/// Consume and validate an owned `__init__` result.
///
/// Python's `__init__` contract is stricter than an ordinary statement-like
/// call: a successful result must be `None`.  This helper consumes the result
/// on every path and raises the canonical `TypeError` for any other value.
#[inline]
pub(crate) unsafe fn consume_init_result(_py: &PyToken<'_>, init_result_bits: u64) -> bool {
    let call_failed = exception_pending(_py);
    let invalid_type = if call_failed || obj_from_bits(init_result_bits).is_none() {
        None
    } else {
        Some(type_name(_py, obj_from_bits(init_result_bits)).into_owned())
    };
    crate::call::discard_owned_call_result(_py, init_result_bits);
    if call_failed {
        return false;
    }
    let Some(type_name) = invalid_type else {
        return true;
    };
    let message = format!("__init__() should return None, not '{type_name}'");
    let _ = raise_exception::<u64>(_py, "TypeError", &message);
    false
}

/// Resolve a constructor's return value after `__init__` has run.
///
/// `inst_bits` carries the single owning reference that the constructor path
/// would otherwise hand back to the caller (the freshly constructed instance).
/// `init_result_bits` is the owned result returned by `__init__`.  If the call
/// raised or returned a non-`None` value, CPython propagates the exception out
/// of the `ClassName(...)` construct expression and discards the instance.
/// Returning `none` is load-bearing: every downstream propagation guard keys off
/// `result.is_none() && exception_pending(_py)` (the IC dispatch guards) and the
/// frontend's post-construct `check_exception` only fires on the `none` result.
/// Returning a live instance with a pending exception silently swallows the
/// raise (task #60).
///
/// This is the single authority for the "transfer the instance XOR drop it and
/// surface the pending exception" decision. EVERY constructor path that invokes
/// a user `__init__` and would `return inst_bits` MUST route through this helper
/// so the fast path and the full-binding path can never re-diverge.
///
/// # Safety
/// `inst_bits` must be the sole owning reference produced by the constructor at
/// the point of the call (exactly the reference the caller's `return inst_bits`
/// would have transferred). `init_result_bits` must be the owning reference
/// returned by the call. On either failure path both references are consumed.
#[inline]
pub(crate) unsafe fn resolve_construct_after_init(
    _py: &PyToken<'_>,
    inst_bits: u64,
    init_result_bits: u64,
) -> u64 {
    if !unsafe { consume_init_result(_py, init_result_bits) } {
        dec_ref_bits(_py, inst_bits);
        return MoltObject::none().bits();
    }
    inst_bits
}

#[inline]
fn reject_builtin_exception_keywords(_py: &PyToken<'_>, class_bits: u64, kw_names: &[u64]) -> bool {
    if kw_names.is_empty() {
        return false;
    }
    let class_name = class_name_for_error(class_bits);
    let msg = format!("{class_name}() takes no keyword arguments");
    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
    true
}

/// Validate and publish the three builtin-exception keyword families that
/// CPython exposes.  The accepted names and physical fields come from the
/// canonical exception-layout authority; this function owns only the
/// constructor parser's exact diagnostics and transactional hand-off.
pub(crate) fn apply_builtin_exception_keywords(
    _py: &PyToken<'_>,
    constructor_class_bits: u64,
    inst_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> bool {
    if kw_names.is_empty() {
        return false;
    }
    if obj_from_bits(inst_bits).as_ptr().is_none() {
        let _ = raise_exception::<u64>(_py, "SystemError", "expected exception object");
        return true;
    }
    let layout = exception_layout_kind_for_class(_py, constructor_class_bits);
    let keyword_policies = layout.constructor_keyword_policies();
    let max_keywords = keyword_policies.clone().count();
    if max_keywords == 0 {
        return reject_builtin_exception_keywords(_py, constructor_class_bits, kw_names);
    }
    let constructor_name = class_name_for_error(constructor_class_bits);
    if kw_names.len() > max_keywords {
        let msg = format!(
            "{constructor_name}() takes at most {max_keywords} keyword argument{} ({} given)",
            if max_keywords == 1 { "" } else { "s" },
            kw_names.len(),
        );
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return true;
    }

    let none = MoltObject::none().bits();
    let mut updates = [(ExceptionTypedField::ImportName, none); 3];
    for (slot, policy) in updates.iter_mut().zip(keyword_policies) {
        *slot = (policy.field, none);
    }
    for (&name_bits, &value_bits) in kw_names.iter().zip(kw_values) {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            let _ = raise_exception::<u64>(_py, "SystemError", "keyword name must be str");
            return true;
        };
        let field = layout
            .constructor_keyword_policy(&name)
            .map(|policy| policy.field);
        let Some(field) = field else {
            let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
                format!("{constructor_name}() got an unexpected keyword argument '{name}'")
            } else {
                format!("'{name}' is an invalid keyword argument for {constructor_name}()")
            };
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return true;
        };
        let slot = updates[..max_keywords]
            .iter_mut()
            .find(|(candidate, _)| *candidate == field)
            .expect("canonical exception keyword field");
        slot.1 = value_bits;
    }

    if let Err(message) =
        exception_typed_fields_replace_internal(_py, inst_bits, &updates[..max_keywords])
    {
        if !exception_pending(_py) {
            let _ = raise_exception::<u64>(_py, "SystemError", message);
        }
        return true;
    }
    false
}

unsafe fn initialize_builtin_exception_from_positional(
    _py: &PyToken<'_>,
    init_bits: u64,
    inst_bits: u64,
    pos: &[u64],
) {
    let args_ptr = alloc_tuple(_py, pos);
    if args_ptr.is_null() {
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    if unsafe {
        callable_matches_runtime_symbol(Some(init_bits), fn_key!(molt_exception_init))
            || callable_matches_runtime_symbol(Some(init_bits), fn_key!(molt_exception_init_owned))
    } {
        let _ = molt_exception_init(inst_bits, args_bits);
    } else {
        debug_assert!(unsafe {
            callable_matches_runtime_symbol(Some(init_bits), fn_key!(molt_exceptiongroup_init))
        });
        let _ = molt_exceptiongroup_init(inst_bits, args_bits);
    }
}

/// Construct an exception subclass through one canonical `__new__`/`__init__`
/// transaction. Both vector/builder calls and fixed-arity runtime calls route
/// here so the runtime-only `(self, args_tuple)` ABI of the builtin exception
/// methods cannot leak into ordinary Python argument forwarding.
pub(crate) unsafe fn construct_exception_from_args(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    pos: &[u64],
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        if kw_names.len() != kw_values.len() {
            return raise_exception::<_>(_py, "SystemError", "malformed constructor keywords");
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.base_exception) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        }

        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let (inst_bits, initialized_by_default_new) = if let Some(new_bits) =
            class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits)
        {
            let default_new =
                callable_matches_runtime_symbol(Some(new_bits), fn_key!(molt_exception_new_bound));
            let result = if default_new {
                let args_ptr = alloc_tuple(_py, pos);
                if args_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let args_bits = MoltObject::from_ptr(args_ptr).bits();
                let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
                dec_ref_bits(_py, args_bits);
                if exc_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(exc_ptr).bits()
            } else {
                // `type.__call__` lends its arguments to each constructor
                // phase; the phase retains its own argument vector.
                crate::call::bind::call_bind_borrowed(
                    _py,
                    new_bits,
                    Some(class_bits),
                    pos,
                    kw_names,
                    kw_values,
                )
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !isinstance_bits(_py, result, class_bits) {
                return result;
            }
            (result, default_new)
        } else {
            let args_ptr = alloc_tuple(_py, pos);
            if args_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let args_bits = MoltObject::from_ptr(args_ptr).bits();
            let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
            dec_ref_bits(_py, args_bits);
            if exc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            (MoltObject::from_ptr(exc_ptr).bits(), true)
        };

        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return inst_bits;
        };
        let init_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
        let Some(init_bits) =
            class_attr_lookup(_py, class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
        else {
            return inst_bits;
        };
        let tuple_init =
            callable_matches_runtime_symbol(Some(init_bits), fn_key!(molt_exception_init))
                || callable_matches_runtime_symbol(
                    Some(init_bits),
                    fn_key!(molt_exception_init_owned),
                )
                || callable_matches_runtime_symbol(
                    Some(init_bits),
                    fn_key!(molt_exceptiongroup_init),
                );
        if tuple_init && initialized_by_default_new {
            let failed =
                apply_builtin_exception_keywords(_py, class_bits, inst_bits, kw_names, kw_values);
            // `class_attr_lookup` returns an owned descriptor result.  For
            // builtin exception roots this is a bound method that owns `self`;
            // retaining it keeps every constructed exception (and its args
            // graph) alive after local rebinding.
            dec_ref_bits(_py, init_bits);
            if failed {
                dec_ref_bits(_py, inst_bits);
                return MoltObject::none().bits();
            }
            return inst_bits;
        }
        if tuple_init {
            initialize_builtin_exception_from_positional(_py, init_bits, inst_bits, pos);
            let failed = exception_pending(_py)
                || apply_builtin_exception_keywords(
                    _py, class_bits, inst_bits, kw_names, kw_values,
                );
            dec_ref_bits(_py, init_bits);
            if failed {
                dec_ref_bits(_py, inst_bits);
                return MoltObject::none().bits();
            }
        } else {
            let init_result = crate::call::bind::call_bind_borrowed(
                _py, init_bits, None, pos, kw_names, kw_values,
            );
            dec_ref_bits(_py, init_bits);
            return resolve_construct_after_init(_py, inst_bits, init_result);
        }
        inst_bits
    }
}

pub(crate) unsafe fn call_class_init_with_args(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    args: &[u64],
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        if class_bits == builtins.none_type {
            if !args.is_empty() {
                return raise_exception::<_>(_py, "TypeError", "NoneType takes no arguments");
            }
            return MoltObject::none().bits();
        }
        if class_bits == builtins.not_implemented_type {
            if !args.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "NotImplementedType takes no arguments",
                );
            }
            return not_implemented_bits(_py);
        }
        if class_bits == builtins.ellipsis_type {
            if !args.is_empty() {
                return raise_exception::<_>(_py, "TypeError", "ellipsis takes no arguments");
            }
            return ellipsis_bits(_py);
        }
        if class_bits == builtins.function {
            return crate::builtins::functions::function_type_new_from_args(_py, args);
        }
        if issubclass_bits(class_bits, builtins.base_exception) {
            return construct_exception_from_args(_py, class_ptr, args, &[], &[]);
        }
        if class_bits == builtins.slice {
            match args.len() {
                0 => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "slice expected at least 1 argument, got 0",
                    );
                }
                1 => {
                    return molt_slice_new(
                        MoltObject::none().bits(),
                        args[0],
                        MoltObject::none().bits(),
                    );
                }
                2 => {
                    return molt_slice_new(args[0], args[1], MoltObject::none().bits());
                }
                3 => {
                    return molt_slice_new(args[0], args[1], args[2]);
                }
                _ => {
                    let msg = format!("slice expected at most 3 arguments, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.range {
            match args.len() {
                0 => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "range expected at least 1 argument, got 0",
                    );
                }
                1 => {
                    let start_bits = MoltObject::from_int(0).bits();
                    let step_bits = MoltObject::from_int(1).bits();
                    return molt_range_new(start_bits, args[0], step_bits);
                }
                2 => {
                    let step_bits = MoltObject::from_int(1).bits();
                    return molt_range_new(args[0], args[1], step_bits);
                }
                3 => {
                    return molt_range_new(args[0], args[1], args[2]);
                }
                _ => {
                    let msg = format!("range expected at most 3 arguments, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if let Some(result) = crate::builtins::types::wrappers::try_construct_exact_wrapper(
            _py,
            class_bits,
            args,
            &[],
            &[],
        ) {
            return result;
        }
        construct_regular_class(_py, class_ptr, args, &[], &[])
    }
}

/// One type.__call__ lifecycle for positional and fully-bound callers. __new__
/// lends its arguments; only a result with real subtype ancestry receives init,
/// and init is selected from the actual result type (CPython type_call).
pub(crate) unsafe fn construct_regular_class(
    py: &PyToken<'_>,
    class: *mut u8,
    positional: &[u64],
    names: &[u64],
    values: &[u64],
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class).bits();
        let new_name = intern_static_name(py, &runtime_state(py).interned.new_name, b"__new__");
        let init_name = intern_static_name(py, &runtime_state(py).interned.init_name, b"__init__");
        let new = class_attr_lookup_raw_mro(py, class, new_name);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let _new_owner = new.and_then(|bits| {
            obj_from_bits(bits).as_ptr().map(|ptr| {
                inc_ref_bits(py, bits);
                crate::PtrDropGuard::new(ptr)
            })
        });
        let instance = match new {
            Some(new) if !resolved_new_is_default_object_new(Some(new)) => {
                crate::call::bind::call_bind_borrowed(
                    py,
                    new,
                    Some(class_bits),
                    positional,
                    names,
                    values,
                )
            }
            _ => {
                let init = class_attr_lookup_raw_mro(py, class, init_name);
                if resolved_constructor_init_policy(new, init)
                    == InitArgPolicy::RejectConstructorArgs
                    && (!positional.is_empty() || !names.is_empty())
                {
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        &format!("{}() takes no arguments", class_name_for_error(class_bits)),
                    );
                }
                alloc_instance_for_default_object_new(py, class)
            }
        };
        if exception_pending(py) {
            crate::call::discard_owned_call_result(py, instance);
            return MoltObject::none().bits();
        }
        let actual_bits = type_of_bits(py, instance);
        if !crate::object::class_layout::is_real_subtype(py, actual_bits, class_bits) {
            return instance;
        }
        let actual = obj_from_bits(actual_bits)
            .as_ptr()
            .expect("instance type is a class");
        let raw_init = class_attr_lookup_raw_mro(py, actual, init_name);
        if exception_pending(py) {
            dec_ref_bits(py, instance);
            return MoltObject::none().bits();
        }
        let Some(raw_init) = raw_init else {
            return instance;
        };
        let actual_new = class_attr_lookup_raw_mro(py, actual, new_name);
        match resolved_constructor_init_policy(actual_new, Some(raw_init)) {
            InitArgPolicy::RejectConstructorArgs if !positional.is_empty() || !names.is_empty() => {
                dec_ref_bits(py, instance);
                return raise_exception::<_>(
                    py,
                    "TypeError",
                    &format!("{}() takes no arguments", class_name_for_error(actual_bits)),
                );
            }
            InitArgPolicy::RejectConstructorArgs | InitArgPolicy::SkipObjectInit => {
                return instance;
            }
            InitArgPolicy::ForwardArgs => {}
        }
        let Some(instance_ptr) = obj_from_bits(instance).as_ptr() else {
            return instance;
        };
        let Some(init) = class_attr_lookup(py, actual, actual, Some(instance_ptr), init_name)
        else {
            if exception_pending(py) {
                dec_ref_bits(py, instance);
                return MoltObject::none().bits();
            }
            return instance;
        };
        let result =
            crate::call::bind::call_bind_borrowed(py, init, None, positional, names, values);
        dec_ref_bits(py, init);
        resolve_construct_after_init(py, instance, result)
    }
}

pub(crate) fn raise_not_callable(_py: &PyToken<'_>, obj: MoltObject) -> u64 {
    let trace_not_callable = matches!(
        std::env::var("MOLT_TRACE_NOT_CALLABLE").ok().as_deref(),
        Some("1")
    );
    if trace_not_callable {
        if let Some(frame) =
            crate::state::tls::FRAME_STACK.with(|stack| stack.borrow().last().copied())
            && let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits)
        {
            let (name_bits, file_bits) =
                unsafe { (code_name_bits(code_ptr), code_filename_bits(code_ptr)) };
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<code>".to_string());
            let file = string_obj_to_owned(obj_from_bits(file_bits))
                .unwrap_or_else(|| "<file>".to_string());
            eprintln!(
                "molt not_callable frame name={} file={} line={}",
                name, file, frame.line
            );
        }
        eprintln!(
            "molt not_callable bits=0x{:x} type={} ptr={} none={} bool={:?} int={:?} float={:?}",
            obj.bits(),
            type_name(_py, obj),
            obj.as_ptr().is_some(),
            obj.is_none(),
            obj.as_bool(),
            obj.as_int(),
            obj.as_float(),
        );
    }
    let msg = format!("'{}' object is not callable", type_name(_py, obj));
    raise_exception::<_>(_py, "TypeError", &msg)
}

pub(crate) unsafe fn call_builtin_type_if_needed(
    _py: &PyToken<'_>,
    call_bits: u64,
    call_ptr: *mut u8,
    args: &[u64],
) -> Option<u64> {
    unsafe {
        if is_builtin_class_bits(_py, call_bits) && crate::object::class_is_immutable(_py, call_ptr)
        {
            let builtins = builtin_classes(_py);
            if call_bits == builtins.super_type {
                return Some(crate::builtins::types::descriptor_objects::super_call(
                    _py, args, false,
                ));
            }
            // `type(...)` needs the builder-aware path in `call_type_via_bind`
            // for CPython-compatible 1-arg and 3-arg semantics.
            if call_bits == builtins.type_obj {
                return None;
            }
            if call_bits == builtins.bool {
                if args.is_empty() {
                    return Some(MoltObject::from_bool(false).bits());
                }
                if args.len() == 1 {
                    return Some(crate::molt_bool_builtin(args[0]));
                }
                let msg = format!("bool expected at most 1 argument, got {}", args.len());
                return Some(raise_exception::<_>(_py, "TypeError", &msg));
            }
            return Some(call_class_init_with_args(_py, call_ptr, args));
        }
        None
    }
}

pub(crate) unsafe fn function_attr_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        if let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr()
            && object_type_id(attr_ptr) == TYPE_ID_STRING
        {
            let name = std::slice::from_raw_parts(string_bytes(attr_ptr), string_len(attr_ptr));
            if crate::object::function_metadata::FunctionMetadataField::from_name(name).is_some() {
                return crate::object::function_metadata::metadata_bits(func_ptr, name);
            }
        }
        let dictionary = crate::object::field_storage::current_dictionary(_py, func_ptr).ok()??;
        dict_get_in_place(_py, obj_from_bits(dictionary).as_ptr().unwrap(), attr_bits)
    }
}

/// Set a borrowed string-keyed attribute. False always leaves an exception;
/// failed first insertion never installs an empty or partially built dictionary.
#[must_use]
pub(crate) unsafe fn function_set_attr_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    attr_bits: u64,
    val_bits: u64,
) -> bool {
    unsafe {
        match function_set_attr_bits_deferred(_py, func_ptr, attr_bits, val_bits) {
            Ok(publication) => {
                let name = obj_from_bits(attr_bits).as_ptr().unwrap();
                crate::call::function::commit_function_metadata_change(
                    _py,
                    func_ptr,
                    std::slice::from_raw_parts(crate::string_bytes(name), crate::string_len(name)),
                    false,
                );
                drop(publication);
                !exception_pending(_py)
            }
            Err(()) => false,
        }
    }
}

/// A committed typed-field or ordinary dictionary update awaiting retirement.
/// Dependent call metadata must be published before this receipt is dropped.
#[must_use]
pub(crate) struct FunctionMetadataPublication<'a, 'py> {
    displaced: Option<crate::object::ops::DetachedDictReferences<'a, 'py>>,
    _typed: Option<crate::object::function_metadata::MetadataRetirement<'a, 'py>>,
}

impl Drop for FunctionMetadataPublication<'_, '_> {
    fn drop(&mut self) {
        let displaced = self.displaced.take();
        molt_cpython_abi::api::errors::with_preserved_error(|| drop(displaced));
    }
}

pub(crate) unsafe fn function_set_attr_bits_deferred<'a, 'py>(
    _py: &'a PyToken<'py>,
    func_ptr: *mut u8,
    attr_bits: u64,
    val_bits: u64,
) -> Result<FunctionMetadataPublication<'a, 'py>, ()> {
    unsafe {
        if exception_pending(_py) {
            return Err(());
        }
        if obj_from_bits(attr_bits)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_STRING)
        {
            raise_exception::<u64>(_py, "TypeError", "function attribute name must be a string");
            return Err(());
        }
        let name = obj_from_bits(attr_bits).as_ptr().unwrap();
        let name = std::slice::from_raw_parts(string_bytes(name), string_len(name));
        if let Some(field) =
            crate::object::function_metadata::FunctionMetadataField::from_name(name)
        {
            return Ok(FunctionMetadataPublication {
                displaced: None,
                _typed: Some(field.replace_deferred(_py, func_ptr, val_bits)),
            });
        }
        crate::object::field_storage::set_item_deferred(_py, func_ptr, attr_bits, val_bits).map(
            |displaced| FunctionMetadataPublication {
                displaced,
                _typed: None,
            },
        )
    }
}

/// Resolve an owned attribute name and release it on either result path.
#[must_use]
pub(crate) unsafe fn function_set_attr_name(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    name: &[u8],
    value: u64,
) -> bool {
    unsafe {
        if exception_pending(py) {
            return false;
        }
        let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
            if !exception_pending(py) {
                raise_exception::<u64>(
                    py,
                    "MemoryError",
                    "function attribute name allocation failed",
                );
            }
            return false;
        };
        let result = function_set_attr_bits(py, func_ptr, name_bits, value);
        dec_ref_bits(py, name_bits);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        alloc_instance_for_class, call_class_init_with_args, construct_exception_from_args,
    };
    use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};
    use crate::*;

    #[test]
    fn function_metadata_failure_never_publishes_or_replaces_a_dictionary() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _ = builtin_classes(py);
            let function = alloc_function_obj(py, 1, 0);
            let name = alloc_string(py, b"metadata_key");
            let value = alloc_list(py, &[]);
            assert!(!function.is_null() && !name.is_null() && !value.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let name_bits = MoltObject::from_ptr(name).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            unsafe {
                assert!(!super::function_set_attr_bits(
                    py,
                    function,
                    MoltObject::none().bits(),
                    value_bits
                ));
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), 0);
                let _ = molt_exception_clear();

                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    ..Default::default()
                })));
                let reset = RestoreTracker;
                assert!(!super::function_set_attr_bits(
                    py, function, name_bits, value_bits
                ));
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), 0);
                let _ = molt_exception_clear();
                assert!(crate::object::field_storage::materialize(py, function).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), 0);
                drop(reset);
                let _ = molt_exception_clear();
                assert_eq!((*header_from_obj_ptr(value)).ref_count_snapshot(), 1);

                assert!(super::function_set_attr_bits(
                    py, function, name_bits, value_bits
                ));
                let dictionary = function_dict_bits(function);
                assert_ne!(dictionary, 0);
                assert_eq!((*header_from_obj_ptr(value)).ref_count_snapshot(), 2);
                assert!(super::function_set_attr_bits(
                    py,
                    function,
                    name_bits,
                    MoltObject::none().bits()
                ));
                assert_eq!(function_dict_bits(function), dictionary);
                assert_eq!((*header_from_obj_ptr(value)).ref_count_snapshot(), 1);
                assert_eq!(
                    crate::object::field_storage::materialize(py, function).unwrap(),
                    dictionary
                );
                assert!(!exception_pending(py));
            }
            for bits in [function_bits, name_bits, value_bits] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn function_metadata_corruption_is_reported_without_legacy_replacement() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let function = alloc_function_obj(py, 1, 0);
            assert!(!function.is_null());
            unsafe {
                let corrupt = MoltObject::from_int(7).bits();
                function_set_dict_bits(function, corrupt);
                assert!(crate::object::field_storage::materialize(py, function).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), corrupt);
                let _ = molt_exception_clear();
                let key = attr_name_bits_from_bytes(py, b"corrupt_read").unwrap();
                assert!(crate::builtins::attributes::attr_lookup_ptr(py, function, key).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), corrupt);
                let _ = molt_exception_clear();
                dec_ref_bits(py, key);
            }
            dec_ref_bits(py, MoltObject::from_ptr(function).bits());
        });
    }

    extern "C" fn compiled_init_borrows_self(self_bits: u64) -> i64 {
        crate::with_gil_entry_nopanic!(_py, {
            assert!(!obj_from_bits(self_bits).is_none());
            MoltObject::none().bits()
        }) as i64
    }

    #[test]
    fn class_instance_allocation_publishes_only_after_class_edge_initialization() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = builtin_classes(_py).object;
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("builtin object class");
            let inst_bits = unsafe { alloc_instance_for_class(_py, class_ptr) };
            let inst_ptr = obj_from_bits(inst_bits)
                .as_ptr()
                .expect("published object instance");
            let header = unsafe { &*header_from_obj_ptr(inst_ptr) };
            assert!(header.gc_is_published());
            assert_eq!(unsafe { object_class_bits(inst_ptr) }, class_bits);
            dec_ref_bits(_py, inst_bits);
        });
    }

    #[test]
    fn class_instance_allocation_capacity_failures_record_memory_error() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class_ptr = obj_from_bits(builtin_classes(py).object)
                .as_ptr()
                .expect("builtin object class");
            for total_size in [0, usize::MAX] {
                let result = unsafe {
                    super::alloc_published_instance_for_class_with_total_size(
                        py, class_ptr, total_size,
                    )
                };
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(py));
                // Emergency MemoryError is a non-allocating pending marker,
                // not a heap exception object exposed by last_pending.
                let error = molt_exception_last_pending();
                assert!(obj_from_bits(error).is_none());
                let _ = molt_exception_clear();
                dec_ref_bits(py, error);
            }
        });
    }

    #[test]
    fn class_instance_allocation_failure_preserves_class_owner_and_can_retry() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class_ptr = obj_from_bits(builtin_classes(py).object)
                .as_ptr()
                .expect("builtin object class");
            unsafe {
                let warm = alloc_instance_for_class(py, class_ptr);
                assert!(obj_from_bits(warm).as_ptr().is_some());
                dec_ref_bits(py, warm);
                let owners = (*header_from_obj_ptr(class_ptr)).ref_count_snapshot();
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    ..Default::default()
                })));
                let reset = RestoreTracker;
                let result = alloc_instance_for_class(py, class_ptr);
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(py));
                assert_eq!(
                    (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
                    owners
                );
                drop(reset);
                let _ = molt_exception_clear();
                let result = alloc_instance_for_class(py, class_ptr);
                let instance = obj_from_bits(result).as_ptr().expect("retry succeeds");
                assert!((*header_from_obj_ptr(instance)).gc_is_published());
                dec_ref_bits(py, result);
                assert_eq!(
                    (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
                    owners
                );
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn class_instance_capacity_failure_preserves_pending_error_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class_ptr = obj_from_bits(builtin_classes(py).object)
                .as_ptr()
                .expect("builtin object class");
            raise_exception::<()>(py, "ValueError", "original allocation caller error");
            let original = molt_exception_last_pending();
            assert!(obj_from_bits(original).as_ptr().is_some());
            unsafe {
                for total_size in [0, usize::MAX] {
                    let result = super::alloc_published_instance_for_class_with_total_size(
                        py, class_ptr, total_size,
                    );
                    assert!(obj_from_bits(result).is_none());
                }
            }
            let observed = molt_exception_last_pending();
            assert_eq!(observed, original);
            let _ = molt_exception_clear();
            dec_ref_bits(py, observed);
            dec_ref_bits(py, original);
        });
    }

    #[test]
    fn generic_class_init_returns_only_the_constructor_result_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let init_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_init_borrows_self as *const (),
                ),
                1,
            );
            assert!(!init_ptr.is_null());
            let init_bits = MoltObject::from_ptr(init_ptr).bits();
            let name_ptr = alloc_string(_py, b"GenericCtor");
            let init_name_ptr = alloc_string(_py, b"__init__");
            assert!(!name_ptr.is_null());
            assert!(!init_name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let init_name_bits = MoltObject::from_ptr(init_name_ptr).bits();
            let attrs = [init_name_bits, init_bits];
            let bases = [builtin_classes(_py).object];
            let class_bits = unsafe {
                crate::object::ops::molt_guarded_class_def(
                    name_bits,
                    crate::provenance::abi::expose_address(bases.as_ptr()),
                    bases.len() as u64,
                    crate::provenance::abi::expose_address(attrs.as_ptr()),
                    1,
                    std::mem::size_of::<u64>() as i64,
                    0,
                    1, // Install the supplied bases before inherited-hook dispatch.
                )
            };
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
            let result_bits = unsafe { call_class_init_with_args(_py, class_ptr, &[]) };
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live instance");
            assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_OBJECT);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(result_ptr)).ref_count_snapshot() },
                1,
                "generic class construction must preserve exactly its result owner while __init__ borrows self"
            );
            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, init_name_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, init_bits);
        });
    }

    #[test]
    fn memoryview_constructor_shares_positional_keyword_and_explicit_new_protocol() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil(|py| unsafe {
            let py = &py;
            let source = crate::alloc_bytearray(py, b"abcdefgh");
            assert!(!source.is_null());
            let source_bits = MoltObject::from_ptr(source).bits();
            let _source = crate::PtrDropGuard::new(source);
            let class = builtin_classes(py).memoryview;
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let object_name = crate::alloc_string(py, b"object");
            assert!(!object_name.is_null());
            let _object_name = crate::PtrDropGuard::new(object_name);
            let object_name = MoltObject::from_ptr(object_name).bits();
            let new_name = crate::attr_name_bits_from_bytes(py, b"__new__").unwrap();
            let new = crate::molt_get_attr_name(class, new_name);
            dec_ref_bits(py, new_name);
            assert!(!exception_pending(py));
            let _new = crate::PtrDropGuard::new(obj_from_bits(new).as_ptr().unwrap());
            let positional = call_class_init_with_args(py, class_ptr, &[source_bits]);
            let keyword = crate::call::bind::call_bind_borrowed(
                py,
                class,
                None,
                &[],
                &[object_name],
                &[source_bits],
            );
            let explicit = crate::call::bind::call_bind_borrowed(
                py,
                new,
                None,
                &[class, source_bits],
                &[],
                &[],
            );
            for view in [positional, keyword, explicit] {
                assert!(!exception_pending(py));
                let ptr = obj_from_bits(view).as_ptr().expect("native memoryview");
                assert_eq!(object_type_id(ptr), crate::TYPE_ID_MEMORYVIEW);
                let bytes = crate::molt_memoryview_tobytes(view);
                assert!(!exception_pending(py));
                let bytes_ptr = obj_from_bits(bytes).as_ptr().unwrap();
                assert_eq!(
                    std::slice::from_raw_parts(
                        crate::bytes_data(bytes_ptr),
                        crate::bytes_len(bytes_ptr)
                    ),
                    b"abcdefgh",
                );
                dec_ref_bits(py, bytes);
                crate::molt_memoryview_release(view);
                dec_ref_bits(py, view);
            }
            let missing = call_class_init_with_args(py, class_ptr, &[]);
            assert!(obj_from_bits(missing).is_none());
            assert!(exception_pending(py));
            let error = crate::molt_exception_last();
            let error_ptr = obj_from_bits(error).as_ptr().unwrap();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "TypeError"
            ));
            assert_eq!(
                crate::format_exception_message(py, error_ptr),
                "memoryview() missing required argument 'object' (pos 1)"
            );
            crate::clear_exception(py);
        });
    }

    #[test]
    fn tuple_subclass_constructor_copies_exact_tuple_without_retagging_source() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtins = builtin_classes(_py);
            let name_ptr = alloc_string(_py, b"AuxTupleSubclass");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let class_ptr = alloc_class_obj(_py, name_bits);
            dec_ref_bits(_py, name_bits);
            assert!(!class_ptr.is_null());
            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    class_ptr,
                    builtins.type_obj,
                    ClassEdgeOwnership::Owned,
                )
            });
            let set_base = molt_class_set_base(class_bits, builtins.tuple);
            assert!(obj_from_bits(set_base).is_none());
            assert!(!exception_pending(_py));

            let items = [
                MoltObject::from_int(11).bits(),
                MoltObject::from_int(29).bits(),
            ];
            let source_ptr = alloc_tuple(_py, &items);
            assert!(!source_ptr.is_null());
            let source_bits = MoltObject::from_ptr(source_ptr).bits();

            let result_bits = unsafe { call_class_init_with_args(_py, class_ptr, &[source_bits]) };
            let result_ptr = obj_from_bits(result_bits)
                .as_ptr()
                .expect("tuple subclass construction must succeed");
            assert_ne!(
                result_ptr, source_ptr,
                "tuple subclass must own a distinct payload"
            );
            assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_TUPLE);
            assert_eq!(unsafe { object_class_bits(result_ptr) }, class_bits);
            assert_eq!(unsafe { object_class_bits(source_ptr) }, 0);
            assert_eq!(
                unsafe {
                    crate::object::seq_access::with_borrowed(result_ptr, |items| items.to_vec())
                }
                .as_slice(),
                items.as_slice()
            );
            assert_eq!(
                unsafe {
                    crate::object::seq_access::with_borrowed(source_ptr, |items| items.to_vec())
                }
                .as_slice(),
                items.as_slice()
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, source_bits);
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn builtin_exception_constructor_rejects_keywords_before_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = builtin_classes(_py).exception;
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("Exception class must be initialized");
            let name_ptr = alloc_string(_py, b"unexpected");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_int(1).bits();

            let result = unsafe {
                construct_exception_from_args(_py, class_ptr, &[], &[name_bits], &[value_bits])
            };
            assert!(obj_from_bits(result).is_none());
            assert!(exception_pending(_py));

            clear_exception(_py);
            dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn builtin_exception_constructor_releases_owned_init_descriptor() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = crate::exception_type_bits_from_name(_py, "AttributeError");
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("AttributeError class");
            let message_ptr = alloc_string(_py, b"missing");
            let keyword_ptr = alloc_string(_py, b"name");
            let field_ptr = alloc_string(_py, b"field");
            assert!(!message_ptr.is_null() && !keyword_ptr.is_null() && !field_ptr.is_null());
            let message_bits = MoltObject::from_ptr(message_ptr).bits();
            let keyword_bits = MoltObject::from_ptr(keyword_ptr).bits();
            let field_bits = MoltObject::from_ptr(field_ptr).bits();

            let result = unsafe {
                construct_exception_from_args(
                    _py,
                    class_ptr,
                    &[message_bits],
                    &[keyword_bits],
                    &[field_bits],
                )
            };
            let result_ptr = obj_from_bits(result)
                .as_ptr()
                .expect("AttributeError construction");
            assert_eq!(
                unsafe { (*header_from_obj_ptr(result_ptr)).ref_count_snapshot() },
                1,
                "the returned exception must carry only the caller-owned reference"
            );

            dec_ref_bits(_py, result);
            dec_ref_bits(_py, message_bits);
            dec_ref_bits(_py, keyword_bits);
            dec_ref_bits(_py, field_bits);
        });
    }

    #[test]
    fn oserror_keyword_rejection_names_requested_constructor_before_errno_selection() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = crate::exception_type_bits_from_name(_py, "OSError");
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("OSError class");
            let message_ptr = alloc_string(_py, b"missing");
            let filename_ptr = alloc_string(_py, b"input.txt");
            let keyword_ptr = alloc_string(_py, b"bad");
            assert!(!message_ptr.is_null() && !filename_ptr.is_null() && !keyword_ptr.is_null());
            let message_bits = MoltObject::from_ptr(message_ptr).bits();
            let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
            let keyword_bits = MoltObject::from_ptr(keyword_ptr).bits();

            let result = unsafe {
                construct_exception_from_args(
                    _py,
                    class_ptr,
                    &[MoltObject::from_int(2).bits(), message_bits, filename_bits],
                    &[keyword_bits],
                    &[MoltObject::from_int(1).bits()],
                )
            };
            assert!(obj_from_bits(result).is_none());
            let error_bits = crate::molt_exception_last();
            let error_ptr = obj_from_bits(error_bits)
                .as_ptr()
                .expect("pending TypeError");
            assert_eq!(
                crate::format_exception_message(_py, error_ptr),
                "OSError() takes no keyword arguments"
            );

            clear_exception(_py);
            dec_ref_bits(_py, error_bits);
            dec_ref_bits(_py, message_bits);
            dec_ref_bits(_py, filename_bits);
            dec_ref_bits(_py, keyword_bits);
        });
    }
}
