use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;

use crate::state::{RuntimeState, cache::clear_atomic_slots};
use crate::{
    ClassInfoProtocol, HEADER_FLAG_SKIP_CLASS_DECREF, PyToken, RuntimeClassInfo, TYPE_ID_BYTES,
    TYPE_ID_COMPLEX, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_ELLIPSIS, TYPE_ID_GENERIC_ALIAS,
    TYPE_ID_LIST, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_PROPERTY, TYPE_ID_RANGE, TYPE_ID_STRING,
    TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_class_obj, alloc_classmethod_obj, alloc_dict_with_pairs,
    alloc_generic_alias, alloc_instance_for_class, alloc_list, alloc_property_obj,
    alloc_staticmethod_obj, alloc_string, alloc_super_obj, alloc_tuple, apply_class_slots_layout,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, builtin_classes, builtin_type_bits,
    call_callable0, call_callable1, call_callable2, class_bases_bits, class_bases_vec,
    class_bump_layout_version, class_dict_bits, class_layout_version_bits, class_mro_bits,
    class_mro_vec, class_name_for_error, class_set_bases_bits, class_set_layout_version_bits,
    class_set_mro_bits, class_set_qualname_bits, clear_exception, collect_runtime_classinfo,
    dataclass_set_class_raw, dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order,
    dict_set_in_place, dict_update_apply, dict_update_set_in_place, exception_pending,
    function_dict_bits, generic_alias_origin_bits, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, instance_dict_bits, intern_static_name, is_builtin_class_bits, is_truthy,
    isinstance_runtime, issubclass_bits, issubclass_runtime, maybe_ptr_from_bits, missing_bits,
    molt_alloc, molt_call_bind, molt_callargs_new, molt_callargs_push_kw, molt_callargs_push_pos,
    molt_contains, molt_dict_from_obj, molt_dict_get, molt_eq, molt_getattr_builtin,
    molt_hash_builtin, molt_index, molt_iter, molt_iter_next, molt_len, molt_object_setattr,
    molt_repr_from_obj, molt_setitem_method, molt_str_from_obj, molt_string_isidentifier, obj_eq,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, property_del_bits,
    property_get_bits, property_set_bits, raise_exception, raise_not_iterable,
    runtime_classinfo_protocol_match, runtime_state, seq_vec_ref, string_obj_to_owned, to_i64,
    tuple_from_iter_bits, type_name, type_of_bits,
};

pub(crate) mod class_construction;
pub(crate) mod class_model;
pub(crate) mod concrete_types;
pub(crate) mod dataclasses;
pub(crate) mod descriptor_objects;
pub(crate) mod dynamic_class_attr;
pub(crate) mod keyword_metadata;

pub use class_construction::*;
pub(crate) use class_construction::{call_vararg_args, call_vararg_kwargs, call_with_kwargs};
pub use class_model::*;
pub use concrete_types::*;
pub(crate) use concrete_types::{
    capsule_class, cell_class, mappingproxy_class, mappingproxy_class_bits, method_class,
    simplenamespace_class, types_drop_instance,
};
pub use dataclasses::*;
pub use descriptor_objects::*;
pub(crate) use dynamic_class_attr::dynamic_class_attribute_class;
pub use dynamic_class_attr::*;
pub use keyword_metadata::*;
pub(crate) use keyword_metadata::{HARD_KEYWORDS, keyword_contains};

macro_rules! define_types_runtime_state {
    (@unit $field:ident) => {
        ()
    };
    ($($field:ident),+ $(,)?) => {
        const TYPES_RUNTIME_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_types_runtime_state!(@unit $field)),+
        ]);

        pub(crate) struct TypesRuntimeState {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl TypesRuntimeState {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(TYPES_RUNTIME_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_types_runtime_state! {
    mappingproxy_class,
    simplenamespace_class,
    capsule_class,
    cell_class,
    dynamic_class_attribute_class,
    method_class,
    mappingproxy_new_fn,
    mappingproxy_init_fn,
    mappingproxy_getitem_fn,
    mappingproxy_iter_fn,
    mappingproxy_len_fn,
    mappingproxy_contains_fn,
    mappingproxy_get_fn,
    mappingproxy_keys_fn,
    mappingproxy_items_fn,
    mappingproxy_values_fn,
    mappingproxy_repr_fn,
    mappingproxy_setitem_fn,
    mappingproxy_delitem_fn,
    simplenamespace_init_fn,
    simplenamespace_repr_fn,
    simplenamespace_eq_fn,
    dynamic_class_attribute_init_fn,
    dynamic_class_attribute_get_fn,
    dynamic_class_attribute_set_fn,
    dynamic_class_attribute_delete_fn,
    dynamic_class_attribute_getter_fn,
    dynamic_class_attribute_setter_fn,
    dynamic_class_attribute_deleter_fn,
    capsule_new_fn,
    cell_new_fn,
    method_new_fn,
    method_init_fn,
    types_coroutine_fn,
    types_get_original_bases_fn,
    types_prepare_class_fn,
    types_resolve_bases_fn,
    types_new_class_fn,
}

fn types_state(_py: &PyToken<'_>) -> &'static TypesRuntimeState {
    &runtime_state(_py).types
}

pub(crate) fn types_clear_runtime_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.types.slots();
    clear_atomic_slots(_py, &slots);
}

fn builtin_func_bits(_py: &PyToken<'_>, slot: &AtomicU64, fn_ptr: u64, arity: u64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                let old_bits = object_class_bits(ptr);
                if old_bits != builtin_bits {
                    if old_bits != 0 {
                        dec_ref_bits(_py, old_bits);
                    }
                    object_set_class_bits(_py, ptr, builtin_bits);
                    inc_ref_bits(_py, builtin_bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn bootstrap_runtime_func_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn types_class(_py: &PyToken<'_>, slot: &AtomicU64, name: &str, layout_size: i64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let class_ptr = alloc_class_obj(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        if class_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                object_set_class_bits(_py, ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
        let _ = molt_class_set_base(class_bits, builtins.object);
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let layout_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
        }
        class_bits
    })
}

fn set_class_method(_py: &PyToken<'_>, class_bits: u64, name: &str, fn_bits: u64) {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return;
    };
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, fn_bits) };
    dec_ref_bits(_py, name_bits);
}

fn mark_vararg_method(_py: &PyToken<'_>, func_bits: u64, include_self: bool) {
    let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
        return;
    };
    let dict_bits = unsafe { function_dict_bits(func_ptr) };
    let dict_ptr = if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return;
        }
        unsafe { crate::function_set_dict_bits(func_ptr, MoltObject::from_ptr(dict_ptr).bits()) };
        dict_ptr
    } else {
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return;
            }
        }
        dict_ptr
    };
    let arg_names = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_arg_names,
        b"__molt_arg_names__",
    );
    if unsafe { dict_get_in_place(_py, dict_ptr, arg_names) }.is_none() {
        let mut names: Vec<u64> = Vec::new();
        if include_self {
            let name_ptr = alloc_string(_py, b"self");
            if !name_ptr.is_null() {
                names.push(MoltObject::from_ptr(name_ptr).bits());
            }
        }
        let names_ptr = alloc_tuple(_py, names.as_slice());
        for bits in names.iter() {
            dec_ref_bits(_py, *bits);
        }
        if !names_ptr.is_null() {
            let names_bits = MoltObject::from_ptr(names_ptr).bits();
            unsafe { dict_set_in_place(_py, dict_ptr, arg_names, names_bits) };
            dec_ref_bits(_py, names_bits);
        }
    }
    let vararg_name = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_vararg,
        b"__molt_vararg__",
    );
    let varkw_name = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_varkw,
        b"__molt_varkw__",
    );
    unsafe {
        dict_set_in_place(
            _py,
            dict_ptr,
            vararg_name,
            MoltObject::from_bool(true).bits(),
        );
        dict_set_in_place(
            _py,
            dict_ptr,
            varkw_name,
            MoltObject::from_bool(true).bits(),
        );
    }
}

fn iter_next_pair(_py: &PyToken<'_>, iter_bits: u64) -> Option<(u64, bool)> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let pair_ptr = pair_obj.as_ptr()?;
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Some((val_bits, done))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stdlib_probe() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_coroutine(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__molt_is_coroutine__") else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(func_bits, name_bits, MoltObject::from_bool(true).bits());
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, func_bits);
        func_bits
    })
}

fn build_types_bootstrap_dict(_py: &PyToken<'_>) -> u64 {
    let debug_bootstrap = std::env::var("MOLT_DEBUG_TYPES_BOOTSTRAP").as_deref() == Ok("1");
    let trace_stage = |stage: &str| {
        if debug_bootstrap {
            eprintln!("molt types bootstrap stage={stage}");
        }
    };
    trace_stage("start");
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return 0;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let builtins = builtin_classes(_py);
    trace_stage("builtins");
    let mappingproxy_bits = mappingproxy_class(_py);
    trace_stage("mappingproxy");
    let simplenamespace_bits = simplenamespace_class(_py);
    trace_stage("simplenamespace");
    let capsule_bits = capsule_class(_py);
    trace_stage("capsule");
    let cell_bits = cell_class(_py);
    trace_stage("cell");
    let dynamic_class_attr_bits = dynamic_class_attribute_class(_py);
    trace_stage("dynamic_class_attribute");

    let method_type_bits = method_class(_py);
    trace_stage("method_type_done");

    // Bootstrap-critical descriptor exports must come from stable runtime
    // type objects, not reflective attribute probing that can recurse back
    // into the still-initializing attribute/type machinery.
    let wrapper_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("wrapper_descriptor");
    let method_wrapper_bits = builtins.builtin_function_or_method;
    trace_stage("method_wrapper");
    let method_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("method_descriptor");
    let classmethod_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("classmethod_descriptor");
    let getset_descriptor_bits = builtins.property;
    trace_stage("getset_descriptor");
    let member_descriptor_bits = builtins.property;
    trace_stage("member_descriptor");

    let coroutine_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_coroutine_fn,
        crate::molt_types_coroutine as *const () as usize as u64,
        1,
    );
    if coroutine_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    trace_stage("coroutine_bits");

    let get_original_bases_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_get_original_bases_fn,
        crate::molt_types_get_original_bases as *const () as usize as u64,
        1,
    );
    if get_original_bases_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    trace_stage("get_original_bases");

    let prepare_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_prepare_class_fn,
        crate::molt_types_prepare_class as *const () as usize as u64,
        2,
    );
    if prepare_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, prepare_bits, false);
    trace_stage("prepare_bits");

    let resolve_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_resolve_bases_fn,
        crate::molt_types_resolve_bases as *const () as usize as u64,
        2,
    );
    if resolve_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, resolve_bits, false);
    trace_stage("resolve_bits");

    let new_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_new_class_fn,
        crate::molt_types_new_class as *const () as usize as u64,
        2,
    );
    if new_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, new_bits, false);
    trace_stage("new_bits");

    let names = [
        ("AsyncGeneratorType", builtins.async_generator),
        ("BuiltinFunctionType", builtins.builtin_function_or_method),
        ("BuiltinMethodType", builtins.builtin_function_or_method),
        ("CapsuleType", capsule_bits),
        ("CellType", cell_bits),
        ("ClassMethodDescriptorType", classmethod_descriptor_bits),
        ("CodeType", builtins.code),
        ("CoroutineType", builtins.coroutine),
        ("EllipsisType", builtins.ellipsis_type),
        ("FrameType", builtins.frame),
        ("FunctionType", builtins.function),
        ("GeneratorType", builtins.generator),
        ("MappingProxyType", mappingproxy_bits),
        ("MethodType", method_type_bits),
        ("MethodDescriptorType", method_descriptor_bits),
        ("MethodWrapperType", method_wrapper_bits),
        ("ModuleType", builtins.module),
        ("NoneType", builtins.none_type),
        ("NotImplementedType", builtins.not_implemented_type),
        ("GenericAlias", builtins.generic_alias),
        ("GetSetDescriptorType", getset_descriptor_bits),
        ("LambdaType", builtins.function),
        ("MemberDescriptorType", member_descriptor_bits),
        ("SimpleNamespace", simplenamespace_bits),
        ("TracebackType", builtins.traceback),
        ("UnionType", builtins.union_type),
        ("WrapperDescriptorType", wrapper_descriptor_bits),
        ("DynamicClassAttribute", dynamic_class_attr_bits),
        ("coroutine", coroutine_bits),
        ("get_original_bases", get_original_bases_bits),
        ("new_class", new_bits),
        ("prepare_class", prepare_bits),
        ("resolve_bases", resolve_bits),
    ];
    let release_failed_payload = || {
        dec_ref_bits(_py, dict_bits);
        0
    };
    for (name, value_bits) in names.iter() {
        let key_ptr = alloc_string(_py, name.as_bytes());
        if key_ptr.is_null() {
            return release_failed_payload();
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        unsafe {
            dict_set_in_place(_py, dict_ptr, key_bits, *value_bits);
        }
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return release_failed_payload();
        }
    }
    trace_stage("dict_populated");
    trace_stage("done");
    dict_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_bootstrap() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_bits = build_types_bootstrap_dict(_py);
        if dict_bits == 0 {
            return MoltObject::none().bits();
        }
        dict_bits
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MoltHeader, maybe_ptr_from_bits};
    use std::sync::Once;
    use std::sync::atomic::Ordering;

    static INIT: Once = Once::new();

    fn init_runtime() {
        INIT.call_once(|| {
            assert_ne!(crate::lifecycle::init(), 0);
        });
        let _ = crate::molt_exception_clear();
    }

    unsafe fn ref_count(bits: u64) -> u32 {
        let ptr = maybe_ptr_from_bits(bits).expect("expected heap object");
        let header = unsafe { ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader };
        unsafe { (*header).ref_count.load(Ordering::Acquire) }
    }

    #[test]
    fn type_new_borrows_kwargs_dict() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let builtins = builtin_classes(_py);
                let name_ptr = alloc_string(_py, b"KwargsBorrowedTypeNew");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let bases_ptr = alloc_tuple(_py, &[builtins.object]);
                assert!(!bases_ptr.is_null());
                let bases_bits = MoltObject::from_ptr(bases_ptr).bits();
                let ns_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!ns_ptr.is_null());
                let ns_bits = MoltObject::from_ptr(ns_ptr).bits();
                let kwargs_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!kwargs_ptr.is_null());
                let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();
                inc_ref_bits(_py, kwargs_bits);
                let before = ref_count(kwargs_bits);

                let cls_bits = molt_type_new(
                    builtins.type_obj,
                    name_bits,
                    bases_bits,
                    ns_bits,
                    kwargs_bits,
                );

                assert!(
                    !exception_pending(_py),
                    "type.__new__ with empty kwargs left an exception pending"
                );
                assert_eq!(
                    ref_count(kwargs_bits),
                    before,
                    "type.__new__ must borrow kwargs; caller owns argument cleanup"
                );

                dec_ref_bits(_py, cls_bits);
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, ns_bits);
                dec_ref_bits(_py, bases_bits);
                dec_ref_bits(_py, name_bits);
            }
        });
    }

    #[test]
    fn type_init_borrows_kwargs_dict() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let kwargs_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!kwargs_ptr.is_null());
                let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();
                inc_ref_bits(_py, kwargs_bits);
                let before = ref_count(kwargs_bits);

                let result = molt_type_init(
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    kwargs_bits,
                );

                assert!(obj_from_bits(result).is_none());
                assert_eq!(
                    ref_count(kwargs_bits),
                    before,
                    "type.__init__ must borrow kwargs; caller owns argument cleanup"
                );
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, kwargs_bits);
            }
        });
    }

    #[test]
    fn types_bootstrap_returns_fresh_dicts_with_cached_helpers() {
        init_runtime();

        let first_bits = molt_types_bootstrap();
        let second_bits = molt_types_bootstrap();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                assert!(
                    !exception_pending(_py),
                    "types bootstrap must not leave an exception pending"
                );
                assert_ne!(
                    first_bits, second_bits,
                    "types bootstrap must return independent module dicts"
                );

                let first_ptr = maybe_ptr_from_bits(first_bits).expect("first bootstrap dict");
                let second_ptr = maybe_ptr_from_bits(second_bits).expect("second bootstrap dict");
                assert_eq!(object_type_id(first_ptr), TYPE_ID_DICT);
                assert_eq!(object_type_id(second_ptr), TYPE_ID_DICT);

                let key_ptr = alloc_string(_py, b"new_class");
                assert!(!key_ptr.is_null());
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let first_new_class =
                    dict_get_in_place(_py, first_ptr, key_bits).expect("first new_class");
                let second_new_class =
                    dict_get_in_place(_py, second_ptr, key_bits).expect("second new_class");
                assert_eq!(
                    first_new_class, second_new_class,
                    "fresh bootstrap dicts should share cached runtime helper objects"
                );

                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, first_bits);
                dec_ref_bits(_py, second_bits);
            }
        });
    }

    #[test]
    fn types_runtime_state_is_owned_and_clearable() {
        init_runtime();

        let state = RuntimeState::new();
        for slot in state.types.slots() {
            slot.store(MoltObject::from_int(7).bits(), Ordering::Release);
        }

        crate::with_gil_entry_nopanic!(_py, {
            types_clear_runtime_state(_py, &state);
        });

        for slot in state.types.slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn vararg_marker_reuses_function_dict_and_preserves_empty_arg_names() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let func_bits = bootstrap_runtime_func_bits(
                    _py,
                    &types_state(_py).types_prepare_class_fn,
                    crate::molt_types_prepare_class as *const () as usize as u64,
                    2,
                );
                assert_ne!(func_bits, 0);
                let func_ptr = maybe_ptr_from_bits(func_bits).expect("prepare_class function");

                mark_vararg_method(_py, func_bits, false);
                let first_dict_bits = function_dict_bits(func_ptr);
                assert_ne!(
                    first_dict_bits, 0,
                    "vararg marker must install a function dict"
                );

                mark_vararg_method(_py, func_bits, false);
                let second_dict_bits = function_dict_bits(func_ptr);
                assert_eq!(
                    first_dict_bits, second_dict_bits,
                    "repeated vararg marking must not replace cached function metadata"
                );

                let dict_ptr = maybe_ptr_from_bits(second_dict_bits).expect("function dict");
                assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);
                let arg_names_key = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_arg_names,
                    b"__molt_arg_names__",
                );
                let arg_names_bits = dict_get_in_place(_py, dict_ptr, arg_names_key)
                    .expect("empty arg-name metadata");
                let arg_names_ptr = maybe_ptr_from_bits(arg_names_bits).expect("arg names tuple");
                assert_eq!(object_type_id(arg_names_ptr), TYPE_ID_TUPLE);
                assert_eq!(
                    seq_vec_ref(arg_names_ptr).len(),
                    0,
                    "non-self vararg helpers still need an explicit empty arg-name tuple"
                );
            }
        });
    }
}
