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
use crate::object::ops_encoding::DecodeFailure;
use crate::*;
use molt_obj_model::ExceptionTypedField;

fn str_codec_arg(_py: &PyToken<'_>, bits: u64, arg_name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("str() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<Option<String>>(_py, "TypeError", &msg);
    };
    Some(text)
}

// A missing capacity is an allocation failure, never absent layout metadata.
fn class_allocation_capacity<T>(_py: &PyToken<'_>, capacity: Option<T>) -> Option<T> {
    if capacity.is_none() {
        record_memory_error_without_allocation(_py);
    }
    capacity
}

unsafe fn max_slot_end_from_offsets_dict(_py: &PyToken<'_>, offsets_ptr: *mut u8) -> Option<usize> {
    unsafe {
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return Some(0);
        }
        let mut max_end = 0usize;
        for pair in dict_order(offsets_ptr).chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let offset = class_allocation_capacity(_py, usize::try_from(offset).ok())?;
                let end =
                    class_allocation_capacity(_py, offset.checked_add(std::mem::size_of::<u64>()))?;
                if end > max_end {
                    max_end = end;
                }
            }
        }
        Some(max_end)
    }
}

unsafe fn max_slot_end_from_mro_offsets(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    fields_name_bits: u64,
) -> Option<usize> {
    unsafe {
        let mro = class_mro_view(_py, class_ptr);
        if exception_pending(_py) {
            return None;
        }
        let mut max_end = 0usize;
        for mro_class_bits in mro.iter().copied() {
            let Some(mro_class_ptr) = obj_from_bits(mro_class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(mro_class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let offsets_bits = crate::builtins::attr::class_field_offsets_map_bits(
                _py,
                mro_class_ptr,
                Some(fields_name_bits),
            );
            if exception_pending(_py) {
                return None;
            }
            let Some(offsets_bits) = offsets_bits else {
                continue;
            };
            let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            max_end = max_end.max(max_slot_end_from_offsets_dict(_py, offsets_ptr)?);
        }
        Some(max_end)
    }
}

/// Compute the byte size of the payload for instances of the class at
/// `class_ptr`.  This involves MRO walks, dict probes and name interning so
/// it is expensive.  Callers in hot loops should cache the result (e.g. via
/// the call-bind IC `cached_alloc_size` field).
pub(crate) unsafe fn class_layout_size_cached(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> Option<usize> {
    unsafe {
        if let Some(size) = crate::object::layout::class_cached_layout_size(class_ptr) {
            return Some(size);
        }
        class_layout_size(_py, class_ptr)
    }
}

unsafe fn class_layout_size(_py: &PyToken<'_>, class_ptr: *mut u8) -> Option<usize> {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let fields_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        if fields_name_bits == 0 || exception_pending(_py) {
            return None;
        }
        let size_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        if size_name_bits == 0 || exception_pending(_py) {
            return None;
        }
        let class_dict_ptr = obj_from_bits(class_dict_bits(class_ptr)).as_ptr();

        // The Python-visible layout metadata is an input to validation, never
        // the hot-path cache authority. A forged smaller value therefore cannot
        // under-allocate an instance.
        let builtins = builtin_classes(_py);
        let reserved_prefix = crate::object::class_reserved_layout_prefix(class_ptr);
        let reserved_tail = crate::object::class_reserved_layout_tail(_py, class_ptr);
        if exception_pending(_py) {
            return None;
        }
        let mut size = 0usize;
        let mut has_own_layout = false;
        let mut own_has_offsets = false;
        if let Some(class_dict_ptr) = class_dict_ptr
            && object_type_id(class_dict_ptr) == TYPE_ID_DICT
        {
            if let Some(size_bits) = dict_get_in_place(_py, class_dict_ptr, size_name_bits)
                && let Some(val) = obj_from_bits(size_bits).as_int()
                && val > 0
            {
                has_own_layout = true;
                size = class_allocation_capacity(_py, usize::try_from(val).ok())?;
            }
            if exception_pending(_py) {
                return None;
            }
            if let Some(offsets_bits) = dict_get_in_place(_py, class_dict_ptr, fields_name_bits) {
                own_has_offsets = obj_from_bits(offsets_bits)
                    .as_ptr()
                    .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        // A sealed ancestor's private size cache is the only layout-size
        // authority. Namespace reads remain available solely for ancestors
        // that are themselves still under construction.
        let mro = class_mro_view(_py, class_ptr);
        if exception_pending(_py) {
            return None;
        }
        for ancestor_bits in mro.iter().copied().skip(1) {
            let Some(ancestor) = obj_from_bits(ancestor_bits).as_ptr() else {
                continue;
            };
            if object_type_id(ancestor) != TYPE_ID_TYPE {
                continue;
            }
            let inherited = if crate::object::class_definition_is_finished(ancestor) {
                Some(
                    crate::object::layout::class_cached_layout_size(ancestor)
                        .expect("sealed ancestor has no private layout size"),
                )
            } else {
                let Some(dict) = obj_from_bits(class_dict_bits(ancestor)).as_ptr() else {
                    continue;
                };
                if object_type_id(dict) != TYPE_ID_DICT {
                    continue;
                }
                let declared = dict_get_in_place(_py, dict, size_name_bits)
                    .and_then(|bits| obj_from_bits(bits).as_int())
                    .filter(|&value| value > 0);
                if exception_pending(_py) {
                    return None;
                }
                match declared {
                    Some(value) => {
                        Some(class_allocation_capacity(_py, usize::try_from(value).ok())?)
                    }
                    None => None,
                }
            };
            if let Some(inherited) = inherited {
                size = size.max(inherited);
            }
        }
        let max_end = max_slot_end_from_mro_offsets(_py, class_ptr, fields_name_bits)?;
        let required = class_allocation_capacity(
            _py,
            max_end.max(reserved_prefix).checked_add(reserved_tail),
        )?;
        if has_own_layout && own_has_offsets && size < required {
            raise_exception::<()>(
                _py,
                "ValueError",
                "class field offset exceeds the declared layout size",
            );
            return None;
        }
        let needs_recompute = !has_own_layout || size < required || !own_has_offsets;
        if needs_recompute {
            size = size.max(required);
        }
        if size == 0 {
            size = reserved_tail.max(std::mem::size_of::<u64>());
        }
        if issubclass_bits(class_bits, builtins.int) && size < 16 {
            size = 16;
        }
        if issubclass_bits(class_bits, builtins.float) && size < 16 {
            size = 16;
        }
        if issubclass_bits(class_bits, builtins.dict) && size < 16 {
            size = 16;
        }
        if needs_recompute
            && let Some(class_dict_ptr) = class_dict_ptr
            && object_type_id(class_dict_ptr) == TYPE_ID_DICT
        {
            let size_i64 = class_allocation_capacity(_py, i64::try_from(size).ok())?;
            let size_bits = MoltObject::from_int(size_i64).bits();
            dict_set_in_place(_py, class_dict_ptr, size_name_bits, size_bits);
            if exception_pending(_py) {
                return None;
            }
            class_bump_layout_version(class_ptr);
        }
        if exception_pending(_py) {
            return None;
        }
        if crate::object::class_definition_is_finished(class_ptr) {
            crate::object::layout::class_set_cached_layout_size(class_ptr, size);
        }
        Some(size)
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
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        if let Some(inst_bits) =
            crate::object::builders::alloc_dataclass_for_class_ptr(_py, class_ptr, class_bits)
        {
            return inst_bits;
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        alloc_instance_for_class(_py, class_ptr)
    }
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
        callable_matches_runtime_symbol(Some(init_bits), fn_addr!(molt_exception_init))
            || callable_matches_runtime_symbol(Some(init_bits), fn_addr!(molt_exception_init_owned))
    } {
        let _ = molt_exception_init(inst_bits, args_bits);
    } else {
        debug_assert!(unsafe {
            callable_matches_runtime_symbol(Some(init_bits), fn_addr!(molt_exceptiongroup_init))
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

        let call = |callable_bits: u64,
                    prefix: Option<u64>,
                    call_pos: &[u64],
                    include_keywords: bool|
         -> u64 {
            let keyword_count = if include_keywords { kw_names.len() } else { 0 };
            let position_count = call_pos.len().saturating_add(usize::from(prefix.is_some()));
            let builder_bits = molt_callargs_new(position_count as u64, keyword_count as u64);
            if builder_bits == 0 {
                return MoltObject::none().bits();
            }
            if let Some(prefix) = prefix {
                let _ = molt_callargs_push_pos(builder_bits, prefix);
                if exception_pending(_py) {
                    dec_ref_bits(_py, builder_bits);
                    return MoltObject::none().bits();
                }
            }
            for &arg in call_pos {
                let _ = molt_callargs_push_pos(builder_bits, arg);
                if exception_pending(_py) {
                    dec_ref_bits(_py, builder_bits);
                    return MoltObject::none().bits();
                }
            }
            if include_keywords {
                for (&name, &value) in kw_names.iter().zip(kw_values) {
                    let _ = molt_callargs_push_kw(builder_bits, name, value);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, builder_bits);
                        return MoltObject::none().bits();
                    }
                }
            }
            molt_call_bind(callable_bits, builder_bits)
        };

        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let (inst_bits, initialized_by_default_new) = if let Some(new_bits) =
            class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits)
        {
            let default_new =
                callable_matches_runtime_symbol(Some(new_bits), fn_addr!(molt_exception_new_bound));
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
                call(new_bits, Some(class_bits), pos, true)
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
            callable_matches_runtime_symbol(Some(init_bits), fn_addr!(molt_exception_init))
                || callable_matches_runtime_symbol(
                    Some(init_bits),
                    fn_addr!(molt_exception_init_owned),
                )
                || callable_matches_runtime_symbol(
                    Some(init_bits),
                    fn_addr!(molt_exceptiongroup_init),
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
            let init_result = call(init_bits, None, pos, true);
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
        let abstract_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.abstractmethods_name,
            b"__abstractmethods__",
        );
        if let Some(abstract_bits) = class_attr_lookup_raw_mro(_py, class_ptr, abstract_name_bits)
            && !obj_from_bits(abstract_bits).is_none()
            && is_truthy(_py, obj_from_bits(abstract_bits))
        {
            let class_name = class_name_for_error(class_bits);
            let msg = format!("Can't instantiate abstract class {class_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
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
        if class_bits == builtins.list {
            match args.len() {
                0 => {
                    let ptr = alloc_list(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                1 => {
                    let Some(bits) = list_from_iter_bits(_py, args[0]) else {
                        return MoltObject::none().bits();
                    };
                    return bits;
                }
                _ => {
                    let msg = format!("list expected at most 1 argument, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.tuple || issubclass_bits(class_bits, builtins.tuple) {
            match args.len() {
                0 => {
                    if class_bits != builtins.tuple {
                        return crate::object::builders::alloc_tuple_subclass(_py, class_bits, &[]);
                    }
                    let ptr = alloc_tuple(_py, &[]);
                    return if ptr.is_null() {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_ptr(ptr).bits()
                    };
                }
                1 => {
                    let Some(bits) = tuple_from_iter_bits(_py, args[0]) else {
                        return MoltObject::none().bits();
                    };
                    if class_bits == builtins.tuple {
                        return bits;
                    }
                    let out = if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                        let Some(items) = crate::object::seq_access::snapshot(
                            _py,
                            ptr,
                            "tuple subclass snapshot allocation failed",
                        ) else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        crate::object::builders::alloc_tuple_subclass(_py, class_bits, &items)
                    } else {
                        MoltObject::none().bits()
                    };
                    dec_ref_bits(_py, bits);
                    return out;
                }
                _ => {
                    let msg = format!("tuple expected at most 1 argument, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.dict {
            match args.len() {
                0 => return molt_dict_new(0),
                1 => return molt_dict_from_obj(args[0]),
                _ => {
                    let msg = format!("dict expected at most 1 argument, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.module {
            match args.len() {
                0 => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "module() missing required argument 'name' (pos 1)",
                    );
                }
                1 => return molt_module_new(args[0]),
                2 => {
                    let mod_bits = molt_module_new(args[0]);
                    if obj_from_bits(mod_bits).is_none() {
                        return mod_bits;
                    }
                    let Some(doc_name_bits) = attr_name_bits_from_bytes(_py, b"__doc__") else {
                        return mod_bits;
                    };
                    let _ = molt_module_set_attr(mod_bits, doc_name_bits, args[1]);
                    dec_ref_bits(_py, doc_name_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return mod_bits;
                }
                _ => {
                    let msg = format!("module expected at most 2 arguments, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.set {
            match args.len() {
                0 => return molt_set_new(0),
                1 => {
                    let set_bits = molt_set_new(0);
                    if obj_from_bits(set_bits).is_none() {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_set_update(set_bits, args[0]);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, set_bits);
                        return MoltObject::none().bits();
                    }
                    return set_bits;
                }
                _ => {
                    let msg = format!("set expected at most 1 argument, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.frozenset {
            match args.len() {
                0 => return molt_frozenset_new(0),
                1 => {
                    let Some(bits) = frozenset_from_iter_bits(_py, args[0]) else {
                        return MoltObject::none().bits();
                    };
                    return bits;
                }
                _ => {
                    let msg = format!("frozenset expected at most 1 argument, got {}", args.len());
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
        if class_bits == builtins.bytes {
            match args.len() {
                0 => {
                    let ptr = alloc_bytes(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                1 => return molt_bytes_from_obj(args[0]),
                2 => return molt_bytes_from_str(args[0], args[1], MoltObject::none().bits()),
                3 => return molt_bytes_from_str(args[0], args[1], args[2]),
                _ => {
                    let msg = format!("bytes() takes at most 3 arguments ({} given)", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.bytearray {
            match args.len() {
                0 => {
                    let ptr = alloc_bytearray(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                1 => return molt_bytearray_from_obj(args[0]),
                2 => return molt_bytearray_from_str(args[0], args[1], MoltObject::none().bits()),
                3 => return molt_bytearray_from_str(args[0], args[1], args[2]),
                _ => {
                    let msg = format!(
                        "bytearray() takes at most 3 arguments ({} given)",
                        args.len()
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if class_bits == builtins.str {
            match args.len() {
                0 => {
                    let ptr = alloc_string(_py, b"");
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                1 => return molt_str_from_obj(args[0]),
                2 | 3 => {
                    let obj = obj_from_bits(args[0]);
                    let Some(ptr) = obj.as_ptr() else {
                        let msg = format!(
                            "decoding to str: need a bytes-like object, {} found",
                            type_name(_py, obj)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    let type_id = object_type_id(ptr);
                    if type_id == TYPE_ID_STRING {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "decoding str is not supported",
                        );
                    }
                    if type_id != TYPE_ID_BYTES
                        && type_id != TYPE_ID_BYTEARRAY
                        && type_id != TYPE_ID_MEMORYVIEW
                    {
                        let msg = format!(
                            "decoding to str: need a bytes-like object, {} found",
                            type_name(_py, obj)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let encoding = match str_codec_arg(_py, args[1], "encoding") {
                        Some(val) => val,
                        None => return MoltObject::none().bits(),
                    };
                    let errors = if args.len() == 3 {
                        match str_codec_arg(_py, args[2], "errors") {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        }
                    } else {
                        "strict".to_string()
                    };
                    let bytes_bits = if type_id == TYPE_ID_BYTES {
                        inc_ref_bits(_py, args[0]);
                        args[0]
                    } else {
                        let bits = molt_bytes_from_obj(args[0]);
                        if obj_from_bits(bits).is_none() {
                            return MoltObject::none().bits();
                        }
                        bits
                    };
                    let bytes_obj = obj_from_bits(bytes_bits);
                    let out_bits = if let Some(bytes_ptr) = bytes_obj.as_ptr() {
                        let bytes = bytes_like_slice(bytes_ptr).unwrap_or(&[]);
                        match decode_bytes_text(&encoding, &errors, bytes) {
                            Ok((text_bytes, _label)) => {
                                let ptr = alloc_string(_py, &text_bytes);
                                if ptr.is_null() {
                                    MoltObject::none().bits()
                                } else {
                                    MoltObject::from_ptr(ptr).bits()
                                }
                            }
                            Err(DecodeTextError::UnknownEncoding(name)) => {
                                let msg = format!("unknown encoding: {name}");
                                raise_exception::<_>(_py, "LookupError", &msg)
                            }
                            Err(DecodeTextError::UnknownErrorHandler(name)) => {
                                let msg = format!("unknown error handler name '{name}'");
                                raise_exception::<_>(_py, "LookupError", &msg)
                            }
                            Err(DecodeTextError::Failure(
                                DecodeFailure::Byte { pos, message, .. },
                                label,
                            )) => raise_unicode_decode_error(
                                _py,
                                &label,
                                bytes_bits,
                                pos,
                                pos + 1,
                                message,
                            ),
                            Err(DecodeTextError::Failure(
                                DecodeFailure::Range {
                                    start,
                                    end,
                                    message,
                                },
                                label,
                            )) => {
                                let end_exclusive = end.saturating_add(1);
                                raise_unicode_decode_error(
                                    _py,
                                    &label,
                                    bytes_bits,
                                    start,
                                    end_exclusive,
                                    message,
                                )
                            }
                            Err(DecodeTextError::Failure(
                                DecodeFailure::UnknownErrorHandler(name),
                                _label,
                            )) => {
                                let msg = format!("unknown error handler name '{name}'");
                                raise_exception::<_>(_py, "LookupError", &msg)
                            }
                        }
                    } else {
                        MoltObject::none().bits()
                    };
                    dec_ref_bits(_py, bytes_bits);
                    return out_bits;
                }
                _ => {
                    let msg = format!("str expected at most 3 arguments, got {}", args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let mut resolved_new_bits = None;
        let inst_bits =
            if let Some(new_bits) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits) {
                resolved_new_bits = Some(new_bits);
                let default_new = resolved_new_is_default_object_new(resolved_new_bits);
                let inst_bits = if default_new {
                    let inst_bits = alloc_instance_for_default_object_new(_py, class_ptr);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                } else {
                    let builder_bits = molt_callargs_new(args.len() as u64 + 1, 0);
                    if builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(builder_bits, class_bits);
                    for &arg in args {
                        let _ = molt_callargs_push_pos(builder_bits, arg);
                    }
                    let inst_bits = molt_call_bind(new_bits, builder_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                inst_bits
            } else {
                alloc_instance_for_class(_py, class_ptr)
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
        match resolved_constructor_init_policy(resolved_new_bits, Some(init_bits)) {
            InitArgPolicy::RejectConstructorArgs if !args.is_empty() => {
                let class_name = class_name_for_error(class_bits);
                let msg = format!("{class_name}() takes no arguments");
                dec_ref_bits(_py, init_bits);
                dec_ref_bits(_py, inst_bits);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            InitArgPolicy::RejectConstructorArgs | InitArgPolicy::SkipObjectInit => {
                dec_ref_bits(_py, init_bits);
                return inst_bits;
            }
            InitArgPolicy::ForwardArgs => {}
        }
        // Every callable ABI borrows Python arguments, including a bound
        // `__init__` receiver. The freshly allocated instance's original owner
        // is the constructor result; the owned bound-method handle separately
        // pins `self` for the duration of this call. Adding a callable-category
        // retain here creates a hidden second result owner on backends whose
        // compiled parameters correctly follow the shared borrowed convention.
        let builder_bits = molt_callargs_new(args.len() as u64, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, init_bits);
            return inst_bits;
        }
        for &arg in args {
            let _ = molt_callargs_push_pos(builder_bits, arg);
        }
        let init_result = molt_call_bind(init_bits, builder_bits);
        dec_ref_bits(_py, init_bits);
        // Consume and validate the owned `__init__` result. A pending exception
        // or non-None return discards the partially-constructed instance.
        resolve_construct_after_init(_py, inst_bits, init_result)
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
        if is_builtin_class_bits(_py, call_bits) {
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
            if call_bits == builtins.float {
                if args.is_empty() {
                    return Some(MoltObject::from_float(0.0).bits());
                }
                if args.len() == 1 {
                    return Some(crate::molt_float_from_obj(args[0]));
                }
                let msg = format!("float expected at most 1 argument, got {}", args.len());
                return Some(raise_exception::<_>(_py, "TypeError", &msg));
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
            if call_bits == builtins.int {
                if args.is_empty() {
                    return Some(MoltObject::from_int(0).bits());
                }
                if args.len() == 1 {
                    let has_base = MoltObject::from_int(0).bits();
                    let base = MoltObject::from_int(10).bits();
                    return Some(crate::molt_int_from_obj(args[0], base, has_base));
                }
                if args.len() == 2 {
                    let has_base = MoltObject::from_int(1).bits();
                    return Some(crate::molt_int_from_obj(args[0], args[1], has_base));
                }
                let msg = format!("int() takes at most 2 arguments ({} given)", args.len());
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
            if let Some(bits) =
                crate::object::layout::function_code_signature_metadata_bits(func_ptr, name)
            {
                return Some(bits);
            }
        }
        let dict_bits = function_dict_bits(func_ptr);
        if dict_bits == 0 {
            return None;
        }
        let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        dict_get_in_place(_py, dict_ptr, attr_bits)
    }
}

/// Allocate the first metadata dictionary off-object. Its allocation reference
/// transfers to the function only after every initial entry is installed.
unsafe fn publish_function_dict(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    pairs: &[u64],
) -> Option<*mut u8> {
    unsafe {
        let dict_ptr =
            crate::object::builders::alloc_dict_with_capacity_and_pairs(py, pairs.len() / 2, &[]);
        if dict_ptr.is_null() || exception_pending(py) {
            if !dict_ptr.is_null() {
                dec_ref_bits(py, MoltObject::from_ptr(dict_ptr).bits());
            }
            if !exception_pending(py) {
                raise_exception::<u64>(py, "MemoryError", "function metadata allocation failed");
            }
            return None;
        }
        for pair in pairs.chunks_exact(2) {
            if crate::object::ops::dict_set_deferred(py, dict_ptr, pair[0], pair[1]).is_err() {
                dec_ref_bits(py, MoltObject::from_ptr(dict_ptr).bits());
                if !exception_pending(py) {
                    raise_exception::<u64>(py, "MemoryError", "function metadata insertion failed");
                }
                return None;
            }
        }
        function_set_dict_bits(func_ptr, MoltObject::from_ptr(dict_ptr).bits());
        Some(dict_ptr)
    }
}

/// Return the live metadata dictionary, creating it only for an explicit
/// __dict__ read. Attribute writes stage their first entry before publication.
pub(crate) unsafe fn function_ensure_dict(py: &PyToken<'_>, func_ptr: *mut u8) -> Option<*mut u8> {
    unsafe {
        if exception_pending(py) {
            return None;
        }
        let bits = function_dict_bits(func_ptr);
        if bits == 0 {
            return publish_function_dict(py, func_ptr, &[]);
        }
        if let Some(ptr) = obj_from_bits(bits).as_ptr()
            && object_type_id(ptr) == TYPE_ID_DICT
        {
            return Some(ptr);
        }
        raise_exception::<Option<*mut u8>>(
            py,
            "SystemError",
            "invalid function metadata dictionary",
        )
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

/// A committed dictionary update whose displaced owner has not been released.
/// Dependent call metadata must be published before this receipt is dropped.
#[must_use]
pub(crate) struct FunctionMetadataPublication<'a, 'py> {
    _displaced: Option<crate::object::ops::DetachedDictReferences<'a, 'py>>,
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
        if !obj_from_bits(attr_bits)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_STRING)
        {
            raise_exception::<u64>(_py, "TypeError", "function attribute name must be a string");
            return Err(());
        }
        if function_dict_bits(func_ptr) == 0 {
            return publish_function_dict(_py, func_ptr, &[attr_bits, val_bits])
                .map(|_| FunctionMetadataPublication { _displaced: None })
                .ok_or(());
        }
        let Some(dict_ptr) = function_ensure_dict(_py, func_ptr) else {
            return Err(());
        };
        let result = crate::object::ops::dict_set_deferred(_py, dict_ptr, attr_bits, val_bits);
        if result.is_err() && !exception_pending(_py) {
            raise_exception::<u64>(_py, "MemoryError", "function metadata insertion failed");
        }
        result.map(|displaced| FunctionMetadataPublication {
            _displaced: Some(displaced),
        })
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
                    MoltObject::from_ptr(super::function_ensure_dict(py, function).unwrap()).bits(),
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
                assert!(super::function_ensure_dict(py, function).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_dict_bits(function), corrupt);
                let _ = molt_exception_clear();
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
