use crate::PyToken;
use crate::builtins::exceptions::{
    molt_exception_init, molt_exception_new_bound, molt_exceptiongroup_init,
};
use crate::call::type_policy::{
    InitArgPolicy, resolved_constructor_init_policy, resolved_new_is_default_object_new,
};
use crate::*;
use std::borrow::Cow;

fn str_codec_arg(_py: &PyToken<'_>, bits: u64, arg_name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("str() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<Option<String>>(_py, "TypeError", &msg);
    };
    Some(text)
}

unsafe fn max_slot_end_from_offsets_dict(_py: &PyToken<'_>, offsets_ptr: *mut u8) -> usize {
    unsafe {
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return 0;
        }
        let mut max_end = 0usize;
        let entries = dict_order(offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let end = (offset as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        max_end
    }
}

unsafe fn max_slot_end_from_mro_offsets(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    fields_name_bits: u64,
) -> usize {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(class_bits))
        };
        let mut max_end = 0usize;
        for mro_class_bits in mro.iter().copied() {
            let Some(mro_class_ptr) = obj_from_bits(mro_class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(mro_class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(mro_class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(offsets_bits) = dict_get_in_place(_py, dict_ptr, fields_name_bits) else {
                continue;
            };
            let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            max_end = max_end.max(max_slot_end_from_offsets_dict(_py, offsets_ptr));
        }
        max_end
    }
}

/// Compute the byte size of the payload for instances of the class at
/// `class_ptr`.  This involves MRO walks, dict probes and name interning so
/// it is expensive.  Callers in hot loops should cache the result (e.g. via
/// the call-bind IC `cached_alloc_size` field).
pub(crate) unsafe fn class_layout_size_cached(_py: &PyToken<'_>, class_ptr: *mut u8) -> usize {
    unsafe { class_layout_size(_py, class_ptr) }
}

unsafe fn class_layout_size(_py: &PyToken<'_>, class_ptr: *mut u8) -> usize {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let fields_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let size_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        let class_dict_ptr = obj_from_bits(class_dict_bits(class_ptr)).as_ptr();

        // Hot path: when the class dict already carries
        // `__molt_layout_size__` AND `__molt_field_offsets__`, the
        // cached size is the recomputation target the slow path
        // below would converge on (`size = max_end + reserved_tail`)
        // — both terms are determined by the same `__molt_field_offsets__`
        // dict that own_has_offsets verifies.  Subsequent calls in
        // tight allocation loops (`while …: Point(0,0)`) hit this
        // path and skip two MRO walks (`class_attr_lookup_raw_mro`
        // for `size_name_bits`, `max_slot_end_from_mro_offsets`
        // for `fields_name_bits`) plus the issubclass-bits MRO
        // walks for the int/dict min-size guards.
        //
        // Soundness rests on the cache invalidation contract:
        // anything that mutates `__molt_field_offsets__` MUST clear
        // or update `__molt_layout_size__` in the same atomic
        // operation, so a stale size never coexists with a fresh
        // offsets dict.  Class definition / inheritance assembly
        // already obey this (the slow path below writes
        // `__molt_layout_size__` last and bumps the layout version);
        // mutating `__molt_field_offsets__` after the class is
        // sealed is unsupported.
        if let Some(class_dict_ptr) = class_dict_ptr
            && object_type_id(class_dict_ptr) == TYPE_ID_DICT
            && let Some(size_bits) = dict_get_in_place(_py, class_dict_ptr, size_name_bits)
            && let Some(cached_size) = obj_from_bits(size_bits).as_int()
            && cached_size > 0
            && let Some(offsets_bits) = dict_get_in_place(_py, class_dict_ptr, fields_name_bits)
            && obj_from_bits(offsets_bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT)
        {
            return cached_size as usize;
        }

        // Slow path: cache miss — full recompute.
        let builtins = builtin_classes(_py);
        let reserved_tail = if issubclass_bits(class_bits, builtins.dict) {
            2 * std::mem::size_of::<u64>()
        } else {
            std::mem::size_of::<u64>()
        };
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
                size = val as usize;
            }
            if let Some(offsets_bits) = dict_get_in_place(_py, class_dict_ptr, fields_name_bits) {
                own_has_offsets = obj_from_bits(offsets_bits)
                    .as_ptr()
                    .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
            }
        }
        if let Some(size_bits) = class_attr_lookup_raw_mro(_py, class_ptr, size_name_bits)
            && let Some(val) = obj_from_bits(size_bits).as_int()
            && val > 0
        {
            size = size.max(val as usize);
        }
        let max_end = max_slot_end_from_mro_offsets(_py, class_ptr, fields_name_bits);
        let needs_recompute = !has_own_layout
            || size < reserved_tail
            || !own_has_offsets
            || size < max_end.saturating_add(reserved_tail);
        if needs_recompute && max_end != 0 {
            size = size.max(max_end.saturating_add(reserved_tail));
        }
        if size == 0 {
            size = reserved_tail.max(std::mem::size_of::<u64>());
        }
        if issubclass_bits(class_bits, builtins.int) && size < 16 {
            size = 16;
        }
        if issubclass_bits(class_bits, builtins.dict) && size < 16 {
            size = 16;
        }
        if needs_recompute
            && let Some(class_dict_ptr) = class_dict_ptr
            && object_type_id(class_dict_ptr) == TYPE_ID_DICT
        {
            let size_bits = MoltObject::from_int(size as i64).bits();
            dict_set_in_place(_py, class_dict_ptr, size_name_bits, size_bits);
            class_bump_layout_version(class_ptr);
        }
        size
    }
}

pub(crate) unsafe fn alloc_instance_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let size = class_layout_size(_py, class_ptr);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        object_set_class_bits(_py, obj_ptr, class_bits);
        inc_ref_bits(_py, class_bits);
        MoltObject::from_ptr(obj_ptr).bits()
    }
}

/// Variant of [`alloc_instance_for_class`] that takes the payload
/// size in bytes as a parameter, skipping the in-runtime
/// `class_layout_size` lookup entirely.  Used by the native
/// codegen for the `object_new_bound` heap path when the frontend
/// already carries the static class size on the SimpleIR op (set
/// by the class-instantiation fold from `class_info["size"]`).
///
/// **Soundness contract**: `payload_size_bytes` MUST equal what
/// `class_layout_size(class_ptr)` would return.  The frontend
/// derives this from the class definition's static field layout
/// (`len(field_order) * 8 + reserved_tail`); the runtime's
/// `class_layout_size` slow path computes the same thing from the
/// dict offsets.  Per the CLAUDE.md no-runtime-monkeypatching
/// contract, class definitions are immutable post-compile, so the
/// two values must agree.
///
/// In debug builds we assert the equality to catch any divergence
/// during testing; in release the assertion is compiled out and
/// the caller pays nothing for the safety check.
pub(crate) unsafe fn alloc_instance_for_class_sized(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    payload_size_bytes: usize,
) -> u64 {
    unsafe {
        debug_assert_eq!(
            payload_size_bytes,
            class_layout_size(_py, class_ptr),
            "alloc_instance_for_class_sized: caller-supplied size must match \
             class_layout_size — frontend layout drift detected"
        );
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let total_size = payload_size_bytes + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        object_set_class_bits(_py, obj_ptr, class_bits);
        inc_ref_bits(_py, class_bits);
        MoltObject::from_ptr(obj_ptr).bits()
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
        alloc_instance_for_class(_py, class_ptr)
    }
}

pub(crate) unsafe fn alloc_instance_for_class_no_pool(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let size = class_layout_size(_py, class_ptr);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        object_set_class_bits(_py, obj_ptr, class_bits);
        inc_ref_bits(_py, class_bits);
        MoltObject::from_ptr(obj_ptr).bits()
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
            let new_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
            let inst_bits =
                if let Some(new_bits) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits) {
                    let mut tuple_new = false;
                    if let Some(new_ptr) = obj_from_bits(new_bits).as_ptr()
                        && object_type_id(new_ptr) == TYPE_ID_FUNCTION
                        && function_fn_ptr(new_ptr) == fn_addr!(molt_exception_new_bound)
                    {
                        tuple_new = true;
                    }
                    let inst_bits = if tuple_new {
                        let args_ptr = alloc_tuple(_py, args);
                        if args_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        let args_bits = MoltObject::from_ptr(args_ptr).bits();
                        let builder_bits = molt_callargs_new(2, 0);
                        if builder_bits == 0 {
                            dec_ref_bits(_py, args_bits);
                            return MoltObject::none().bits();
                        }
                        let _ = molt_callargs_push_pos(builder_bits, class_bits);
                        let _ = molt_callargs_push_pos(builder_bits, args_bits);
                        let inst_bits = molt_call_bind(new_bits, builder_bits);
                        dec_ref_bits(_py, args_bits);
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
                        molt_call_bind(new_bits, builder_bits)
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                } else {
                    let args_ptr = alloc_tuple(_py, args);
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
            let mut tuple_init = false;
            if let Some(init_ptr) = obj_from_bits(init_bits).as_ptr()
                && object_type_id(init_ptr) == TYPE_ID_FUNCTION
            {
                let fn_ptr = function_fn_ptr(init_ptr);
                if fn_ptr == fn_addr!(molt_exception_init)
                    || fn_ptr == fn_addr!(molt_exceptiongroup_init)
                {
                    tuple_init = true;
                }
            }
            if tuple_init {
                let args_ptr = alloc_tuple(_py, args);
                if args_ptr.is_null() {
                    return inst_bits;
                }
                let args_bits = MoltObject::from_ptr(args_ptr).bits();
                let builder_bits = molt_callargs_new(2, 0);
                if builder_bits == 0 {
                    dec_ref_bits(_py, args_bits);
                    return inst_bits;
                }
                let _ = molt_callargs_push_pos(builder_bits, inst_bits);
                let _ = molt_callargs_push_pos(builder_bits, args_bits);
                let _ = molt_call_bind(init_bits, builder_bits);
                dec_ref_bits(_py, args_bits);
            } else {
                let pos_capacity = args.len() as u64;
                let builder_bits = molt_callargs_new(pos_capacity, 0);
                if builder_bits == 0 {
                    return inst_bits;
                }
                for &arg in args {
                    let _ = molt_callargs_push_pos(builder_bits, arg);
                }
                let _ = molt_call_bind(init_bits, builder_bits);
            }
            return inst_bits;
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
                    let ptr = alloc_tuple(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let out_bits = MoltObject::from_ptr(ptr).bits();
                    if class_bits != builtins.tuple {
                        let old_class_bits = object_class_bits(ptr);
                        if old_class_bits != class_bits {
                            if old_class_bits != 0 {
                                dec_ref_bits(_py, old_class_bits);
                            }
                            object_set_class_bits(_py, ptr, class_bits);
                            inc_ref_bits(_py, class_bits);
                        }
                    }
                    return out_bits;
                }
                1 => {
                    let Some(bits) = tuple_from_iter_bits(_py, args[0]) else {
                        return MoltObject::none().bits();
                    };
                    if class_bits != builtins.tuple
                        && let Some(ptr) = obj_from_bits(bits).as_ptr()
                    {
                        let old_class_bits = object_class_bits(ptr);
                        if old_class_bits != class_bits {
                            if old_class_bits != 0 {
                                dec_ref_bits(_py, old_class_bits);
                            }
                            object_set_class_bits(_py, ptr, class_bits);
                            inc_ref_bits(_py, class_bits);
                        }
                    }
                    return bits;
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
        if class_bits == builtins.classmethod {
            if args.len() != 1 {
                let msg = format!("classmethod expected 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            return molt_classmethod_new(args[0]);
        }
        if class_bits == builtins.staticmethod {
            if args.len() != 1 {
                let msg = format!("staticmethod expected 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            return molt_staticmethod_new(args[0]);
        }
        if class_bits == builtins.property {
            if args.len() > 4 {
                let msg = format!(
                    "property() takes at most 4 arguments ({} given)",
                    args.len()
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let none_bits = MoltObject::none().bits();
            let get_bits = args.first().copied().unwrap_or(none_bits);
            let set_bits = args.get(1).copied().unwrap_or(none_bits);
            let del_bits = args.get(2).copied().unwrap_or(none_bits);
            return molt_property_new(get_bits, set_bits, del_bits);
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
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            InitArgPolicy::RejectConstructorArgs | InitArgPolicy::SkipObjectInit => {
                return inst_bits;
            }
            InitArgPolicy::ForwardArgs => {}
        }
        // Inc-ref the instance before passing to __init__. The compiled
        // __init__ receives `self` as a block param and the function
        // epilogue dec-refs all tracked locals (including self). Without
        // this extra inc-ref, the dec-ref drops the instance to refcount 0,
        // freeing it — and the caller's inst_bits becomes a dangling pointer.
        inc_ref_bits(_py, inst_bits);
        let builder_bits = molt_callargs_new(args.len() as u64, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, inst_bits);
            return inst_bits;
        }
        for &arg in args {
            let _ = molt_callargs_push_pos(builder_bits, arg);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        inst_bits
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
            // `super` is a builtin type (CPython parity). We must handle it here so that
            // indirect calls like `alias = builtins.super; alias()` produce CPython-shaped
            // errors (RuntimeError when `__class__` cell is missing) instead of falling
            // through to the generic type-call path.
            let builtins = builtin_classes(_py);
            if call_bits == builtins.super_type {
                if args.is_empty() {
                    // CPython distinguishes between calling from module scope (no args at all)
                    // and calling from a function/method frame without a `__class__` cell.
                    let has_pos_args = crate::state::tls::FRAME_STACK.with(|stack| {
                        let frame = stack.borrow().last().copied();
                        let Some(frame) = frame else {
                            return false;
                        };
                        let Some(code_ptr) = obj_from_bits(frame.code_bits).as_ptr() else {
                            return false;
                        };
                        if object_type_id(code_ptr) != TYPE_ID_CODE {
                            return false;
                        }
                        code_argcount(code_ptr) > 0
                    });
                    let msg = if has_pos_args {
                        "super(): __class__ cell not found"
                    } else {
                        "super(): no arguments"
                    };
                    return Some(raise_exception::<_>(_py, "RuntimeError", msg));
                }
                if args.len() == 1 {
                    return Some(molt_super_new(args[0], MoltObject::none().bits()));
                }
                if args.len() == 2 {
                    return Some(molt_super_new(args[0], args[1]));
                }
                let msg = format!("super() expected at most 2 arguments, got {}", args.len());
                return Some(raise_exception::<_>(_py, "TypeError", &msg));
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

pub(crate) unsafe fn try_call_generator(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> Option<u64> {
    unsafe {
        let func_obj = obj_from_bits(func_bits);
        let func_ptr = func_obj.as_ptr()?;
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return None;
        }
        let is_gen = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_is_generator,
                b"__molt_is_generator__",
            ),
        )
        .is_some_and(|bits| is_truthy(_py, obj_from_bits(bits)));
        if !is_gen {
            return None;
        }
        let size_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_closure_size,
                b"__molt_closure_size__",
            ),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let Some(size_val) = obj_from_bits(size_bits).as_int() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if size_val < 0 {
            return raise_exception::<_>(_py, "TypeError", "closure size must be non-negative");
        }
        let closure_size = size_val as usize;
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        let mut payload: Vec<u64> =
            Vec::with_capacity(args.len() + if closure_bits != 0 { 1 } else { 0 });
        if closure_bits != 0 {
            payload.push(closure_bits);
        }
        payload.extend(args.iter().copied());
        let base = GEN_CONTROL_SIZE;
        let needed = base + payload.len() * std::mem::size_of::<u64>();
        if closure_size < needed {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let obj_bits = molt_generator_new(fn_ptr, closure_size as u64);
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return Some(MoltObject::none().bits());
        };
        let mut offset = base;
        for val_bits in payload {
            let slot = obj_ptr.add(offset) as *mut u64;
            *slot = val_bits;
            inc_ref_bits(_py, val_bits);
            offset += std::mem::size_of::<u64>();
        }
        Some(obj_bits)
    }
}

pub(crate) unsafe fn function_attr_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
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

/// Set an attribute on a function object's __dict__.
/// If the function has no dict or the dict slot holds a non-dict value (e.g. a
/// bare int from the legacy FUNC_DEFAULT_* system), a fresh dict is allocated
/// and installed before inserting the key-value pair.
pub(crate) unsafe fn function_set_attr_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    attr_bits: u64,
    val_bits: u64,
) {
    unsafe {
        let dict_bits = function_dict_bits(func_ptr);
        let dict_ptr = if dict_bits != 0 {
            if let Some(p) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(p) == TYPE_ID_DICT {
                    p
                } else {
                    // Dict slot holds a non-dict (legacy default_kind int).
                    // Replace it with a real dict.
                    let new_dict = alloc_dict_with_pairs(_py, &[]);
                    function_set_dict_bits(func_ptr, MoltObject::from_ptr(new_dict).bits());
                    new_dict
                }
            } else {
                let new_dict = alloc_dict_with_pairs(_py, &[]);
                function_set_dict_bits(func_ptr, MoltObject::from_ptr(new_dict).bits());
                new_dict
            }
        } else {
            let new_dict = alloc_dict_with_pairs(_py, &[]);
            function_set_dict_bits(func_ptr, MoltObject::from_ptr(new_dict).bits());
            new_dict
        };
        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
    }
}
