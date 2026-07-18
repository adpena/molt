use crate::PyToken;
use crate::builtins::exceptions::{
    molt_exception_init, molt_exception_new_bound, molt_exceptiongroup_init,
};
use crate::call::type_policy::{
    InitArgPolicy, callable_matches_runtime_symbol, resolved_constructor_init_policy,
    resolved_new_is_default_object_new,
};
use crate::object::ops_encoding::DecodeFailure;
use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};
use crate::*;

fn str_codec_arg(_py: &PyToken<'_>, bits: u64, arg_name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("str() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<Option<String>>(_py, "TypeError", &msg);
    };
    Some(text)
}

unsafe fn max_slot_end_from_offsets_dict(_py: &PyToken<'_>, offsets_ptr: *mut u8) -> Option<usize> {
    unsafe {
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return Some(0);
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
                let offset = usize::try_from(offset).ok()?;
                let end = offset.checked_add(std::mem::size_of::<u64>())?;
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
        let size_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        let class_dict_ptr = obj_from_bits(class_dict_bits(class_ptr)).as_ptr();

        // The Python-visible layout metadata is an input to validation, never
        // the hot-path cache authority. A forged smaller value therefore cannot
        // under-allocate an instance.
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
                size = usize::try_from(val).ok()?;
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
            size = size.max(usize::try_from(val).ok()?);
        }
        let max_end = max_slot_end_from_mro_offsets(_py, class_ptr, fields_name_bits)?;
        let required = max_end.checked_add(reserved_tail)?;
        let needs_recompute =
            !has_own_layout || size < reserved_tail || !own_has_offsets || size < required;
        if needs_recompute && max_end != 0 {
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
            && let Ok(size_i64) = i64::try_from(size)
        {
            let size_bits = MoltObject::from_int(size_i64).bits();
            dict_set_in_place(_py, class_dict_ptr, size_name_bits, size_bits);
            class_bump_layout_version(class_ptr);
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
        let type_id = crate::object::class_instance_type_id(class_ptr);
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let obj_ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
            _py,
            total_size,
            type_id,
            ObjectAuxPreselection::ClassInline,
        );
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if !object_init_class_edge_unpublished(_py, obj_ptr, class_bits, ClassEdgeOwnership::Owned)
        {
            dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            return MoltObject::none().bits();
        }
        crate::object::gc::gc_publish_initialized(_py, obj_ptr);
        MoltObject::from_ptr(obj_ptr).bits()
    }
}

pub(crate) unsafe fn alloc_instance_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> u64 {
    unsafe {
        let Some(payload_size) = class_layout_size_cached(_py, class_ptr) else {
            return MoltObject::none().bits();
        };
        let Some(total_size) = payload_size.checked_add(std::mem::size_of::<MoltHeader>()) else {
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
        alloc_instance_for_class(_py, class_ptr)
    }
}

pub(crate) unsafe fn alloc_instance_for_class_no_pool(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> u64 {
    unsafe {
        let Some(payload_size) = class_layout_size_cached(_py, class_ptr) else {
            return MoltObject::none().bits();
        };
        let Some(total_size) = payload_size.checked_add(std::mem::size_of::<MoltHeader>()) else {
            return MoltObject::none().bits();
        };
        alloc_published_instance_for_class_with_total_size(_py, class_ptr, total_size)
    }
}

/// Resolve a constructor's return value after `__init__` has run.
///
/// `inst_bits` carries the single owning reference that the constructor path
/// would otherwise hand back to the caller (the freshly constructed instance).
/// If `__init__` raised, CPython propagates that exception out of the
/// `ClassName(...)` construct expression and the instance is discarded; mirror
/// that here by dropping the owning reference and returning the `none` sentinel.
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
/// would have transferred). On the exception path that one reference is dropped.
#[inline]
pub(crate) unsafe fn resolve_construct_after_init(_py: &PyToken<'_>, inst_bits: u64) -> u64 {
    if exception_pending(_py) {
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
    if unsafe { callable_matches_runtime_symbol(Some(init_bits), fn_addr!(molt_exception_init)) } {
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
                    fn_addr!(molt_exceptiongroup_init),
                );
        if tuple_init && initialized_by_default_new {
            if reject_builtin_exception_keywords(_py, class_bits, kw_names) {
                dec_ref_bits(_py, inst_bits);
                return MoltObject::none().bits();
            }
            return inst_bits;
        }
        if tuple_init {
            if reject_builtin_exception_keywords(_py, class_bits, kw_names) {
                dec_ref_bits(_py, inst_bits);
                return MoltObject::none().bits();
            }
            initialize_builtin_exception_from_positional(_py, init_bits, inst_bits, pos);
        } else {
            let _ = call(init_bits, None, pos, true);
        }
        resolve_construct_after_init(_py, inst_bits)
    }
}

/// Allocate a fresh tuple payload for a tuple subclass and attach its class
/// before the value crosses the constructor boundary. Tuple subclass
/// construction must never reuse and retag an exact tuple input.
unsafe fn alloc_tuple_subclass_from_items(
    _py: &PyToken<'_>,
    class_bits: u64,
    items: &[u64],
) -> u64 {
    let ptr = alloc_tuple(_py, items);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    if !unsafe {
        object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned)
    } {
        dec_ref_bits(_py, bits);
        return MoltObject::none().bits();
    }
    bits
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
                        return alloc_tuple_subclass_from_items(_py, class_bits, &[]);
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
                        alloc_tuple_subclass_from_items(_py, class_bits, &items)
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
        // If `__init__` (including a full-binding `*args`/`**kwargs`/keyword-only
        // signature) raised, propagate it instead of returning the
        // partially-constructed instance (task #60, args-slice construct lane).
        resolve_construct_after_init(_py, inst_bits)
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

#[cfg(test)]
mod tests {
    use super::{
        alloc_instance_for_class, call_class_init_with_args, construct_exception_from_args,
    };
    use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};
    use crate::*;

    #[test]
    fn class_instance_allocation_publishes_only_after_class_edge_initialization() {
        let _guard = crate::test_mutex_guard();
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
    fn tuple_subclass_constructor_copies_exact_tuple_without_retagging_source() {
        let _guard = crate::test_mutex_guard();
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
        let _guard = crate::test_mutex_guard();
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
}
