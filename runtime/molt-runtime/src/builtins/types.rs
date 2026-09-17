use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use molt_obj_model::MoltObject;

use crate::object::seq_access::{locked_len, pin_item};
use crate::state::{RuntimeState, cache::clear_atomic_slots};
use crate::{
    ClassEdgeOwnership, ClassInfoProtocol, PyToken, RuntimeClassInfo, TYPE_ID_BYTES,
    TYPE_ID_COMPLEX, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_ELLIPSIS, TYPE_ID_GENERIC_ALIAS,
    TYPE_ID_LIST, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_RANGE, TYPE_ID_STRING, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, alloc_class_obj, alloc_dict_with_pairs, alloc_generic_alias,
    alloc_instance_for_class, alloc_list, alloc_property_obj, alloc_string, alloc_super_obj,
    alloc_tuple, apply_class_slots_layout, attr_name_bits_from_bytes, builtin_classes,
    builtin_type_bits, call_callable0, call_callable1, call_callable2, class_bases_bits,
    class_bases_vec, class_bump_layout_version, class_dict_bits, class_layout_version_bits,
    class_mro_vec, class_name_for_error, class_set_layout_version_bits, class_set_qualname_bits,
    clear_exception, collect_runtime_classinfo, dec_ref_bits, dict_del_in_place, dict_get_in_place,
    dict_order, dict_set_in_place, dict_update_apply, dict_update_set_in_place, exception_pending,
    function_dict_bits, generic_alias_origin_bits, inc_ref_bits, init_atomic_bits,
    instance_dict_bits, intern_static_name, is_truthy, isinstance_runtime, issubclass_bits,
    issubclass_runtime, missing_bits, molt_call_bind, molt_callargs_new, molt_callargs_push_kw,
    molt_callargs_push_pos, molt_contains, molt_dict_from_obj, molt_dict_get, molt_eq,
    molt_getattr_builtin, molt_hash_builtin, molt_index, molt_iter, molt_iter_next, molt_len,
    molt_object_setattr, molt_repr_from_obj, molt_setitem_method, molt_str_from_obj,
    molt_string_isidentifier, obj_eq, obj_from_bits, object_class_bits, object_type_id,
    property_del_bits, property_get_bits, property_set_bits, raise_exception, raise_not_iterable,
    runtime_classinfo_protocol_match, runtime_state, string_obj_to_owned, to_i64,
    tuple_from_iter_bits, type_name, type_of_bits,
};

pub(crate) mod class_construction;
pub(crate) mod class_model;
pub(crate) mod concrete_types;
pub(crate) mod dataclasses;
pub(crate) mod descriptor_objects;
pub(crate) mod dynamic_class_attr;
pub(crate) mod keyword_metadata;
pub(crate) mod native_descriptors;
pub(crate) mod wrappers;

pub use class_construction::*;
pub(crate) use class_construction::{call_vararg_args, call_vararg_kwargs, call_with_kwargs};
pub use class_model::*;
pub use concrete_types::*;
pub(crate) use concrete_types::{
    capsule_class, cell_class, mappingproxy_class, mappingproxy_class_bits, method_class,
    simplenamespace_class,
};
pub use dataclasses::*;
pub use descriptor_objects::*;
pub(crate) use dynamic_class_attr::dynamic_class_attribute_class;
pub use dynamic_class_attr::*;
pub use keyword_metadata::*;
pub(crate) use keyword_metadata::{HARD_KEYWORDS, keyword_contains};
pub use native_descriptors::*;
pub use wrappers::*;

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
    member_descriptor_class,
    getset_descriptor_class,
    native_descriptor_new_fn,
    native_descriptor_get_fn,
    native_descriptor_set_fn,
    native_descriptor_delete_fn,
    native_descriptor_repr_fn,
    native_descriptor_reduce_fn,
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
    cell_eq_fn,
    cell_ne_fn,
    cell_lt_fn,
    cell_le_fn,
    cell_gt_fn,
    cell_ge_fn,
    cell_contents_get_fn,
    cell_contents_set_fn,
    cell_contents_delete_fn,
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

pub(crate) fn types_runtime_class_roots(py: &PyToken<'_>, state: &RuntimeState) -> Vec<u64> {
    state
        .types
        .slots()
        .into_iter()
        .map(|slot| slot.load(AtomicOrdering::Acquire))
        .filter(|bits| crate::object::class_storage::is_canonical_runtime_class(py, *bits))
        .collect()
}

/// Clear callback-bearing cache owners, retaining canonical class identities
/// until the shared runtime-class retirement transaction reaches its tail.
pub(crate) fn types_clear_runtime_callbacks(py: &PyToken<'_>, state: &RuntimeState) -> bool {
    let mut detached = Vec::new();
    for slot in state.types.slots() {
        let bits = slot.load(AtomicOrdering::Acquire);
        if bits != 0 && !crate::object::class_storage::is_canonical_runtime_class(py, bits) {
            detached.push(slot.swap(0, AtomicOrdering::AcqRel));
        }
    }
    let changed = !detached.is_empty();
    for bits in detached {
        dec_ref_bits(py, bits);
    }
    changed
}

pub(crate) use crate::builtins::methods::builtin_func_bits;

fn bootstrap_runtime_func_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    signature: Option<(&[&[u8]], bool, bool)>,
) -> u64 {
    if exception_pending(_py) {
        return 0;
    }
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            if !exception_pending(_py) {
                let _ = raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "types bootstrap callable allocation failed",
                );
            }
            return 0;
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        if let Some((arg_names, has_vararg, has_varkw)) = signature
            && !crate::builtins::methods::configure_builtin_signature(
                _py, bits, arg_names, has_vararg, has_varkw,
            )
        {
            dec_ref_bits(_py, bits);
            return 0;
        }
        bits
    })
}

fn runtime_class_init_failed(_py: &PyToken<'_>, class_bits: u64, name: &str) -> u64 {
    if class_bits != 0 {
        dec_ref_bits(_py, class_bits);
    }
    if !exception_pending(_py) {
        let _ = raise_exception::<u64>(
            _py,
            "SystemError",
            &format!("{name} class initialization failed"),
        );
    }
    0
}

/// Build a runtime-owned class completely before publishing its cache slot.
///
/// All runtime-created classes in the `types`, `functools`, and `operator`
/// families use this authority so base, layout, namespace, callable, and
/// definition-finalization failures cannot cache a partially initialized
/// class.
fn init_cached_runtime_class_configured(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    layout_size: i64,
    instance_shape: Option<crate::object::ObjectShapeId>,
    configure: impl FnOnce(u64, *mut u8) -> bool,
) -> u64 {
    if exception_pending(_py) {
        return 0;
    }
    init_atomic_bits(_py, slot, || {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            if !exception_pending(_py) {
                let _ = raise_exception::<u64>(_py, "MemoryError", "class name allocation failed");
            }
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let class_ptr = alloc_class_obj(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        if class_ptr.is_null() {
            if !exception_pending(_py) {
                let _ = raise_exception::<u64>(_py, "MemoryError", "class allocation failed");
            }
            return 0;
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        if let Some(shape) = instance_shape
            && !unsafe { crate::object::class_set_instance_shape_id(class_ptr, shape) }
        {
            return runtime_class_init_failed(_py, class_bits, name);
        }
        let builtins = builtin_classes(_py);
        if !unsafe {
            crate::object::object_init_class_edge_unpublished(
                _py,
                class_ptr,
                builtins.type_obj,
                ClassEdgeOwnership::Owned,
            )
        } {
            return runtime_class_init_failed(_py, class_bits, name);
        }

        let base_result = molt_class_set_base(class_bits, builtins.object);
        dec_ref_bits(_py, base_result);
        let bases = class_bases_vec(unsafe { class_bases_bits(class_ptr) });
        if exception_pending(_py) || bases.as_slice() != [builtins.object] {
            return runtime_class_init_failed(_py, class_bits, name);
        }

        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return runtime_class_init_failed(_py, class_bits, name);
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return runtime_class_init_failed(_py, class_bits, name);
        }
        let layout_name = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        if layout_name == 0 || exception_pending(_py) {
            return runtime_class_init_failed(_py, class_bits, name);
        }
        let layout_bits = MoltObject::from_int(layout_size).bits();
        unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
        if exception_pending(_py)
            || unsafe { dict_get_in_place(_py, dict_ptr, layout_name) } != Some(layout_bits)
            || !configure(class_bits, dict_ptr)
        {
            return runtime_class_init_failed(_py, class_bits, name);
        }
        if unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err() {
            return runtime_class_init_failed(_py, class_bits, name);
        }
        class_bits
    })
}

#[derive(Clone, Copy)]
pub(crate) struct RuntimeMethodSignature<'a> {
    arg_names: &'a [&'a [u8]],
    has_vararg: bool,
    has_varkw: bool,
}

pub(crate) const NO_RUNTIME_ARGUMENT_NAMES: &[&[u8]] = &[];
pub(crate) const SELF_RUNTIME_ARGUMENT_NAMES: &[&[u8]] = &[b"self"];
pub(crate) const SELF_NAME_RUNTIME_ARGUMENT_NAMES: &[&[u8]] = &[b"self", b"name"];

impl<'a> RuntimeMethodSignature<'a> {
    pub(crate) const fn new(arg_names: &'a [&'a [u8]], has_vararg: bool, has_varkw: bool) -> Self {
        Self {
            arg_names,
            has_vararg,
            has_varkw,
        }
    }
}

pub(crate) struct RuntimeClassMethodSpec<'a> {
    name: &'static str,
    slot: &'a AtomicU64,
    fn_ptr: u64,
    arity: u64,
    signature: Option<RuntimeMethodSignature<'a>>,
}

impl<'a> RuntimeClassMethodSpec<'a> {
    pub(crate) const fn fixed(
        name: &'static str,
        slot: &'a AtomicU64,
        fn_ptr: u64,
        arity: u64,
    ) -> Self {
        Self {
            name,
            slot,
            fn_ptr,
            arity,
            signature: None,
        }
    }

    pub(crate) const fn with_signature(
        name: &'static str,
        slot: &'a AtomicU64,
        fn_ptr: u64,
        arity: u64,
        signature: RuntimeMethodSignature<'a>,
    ) -> Self {
        Self {
            name,
            slot,
            fn_ptr,
            arity,
            signature: Some(signature),
        }
    }
}

pub(crate) fn init_cached_runtime_class(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    layout_size: i64,
    instance_shape: Option<crate::object::ObjectShapeId>,
    methods: &[RuntimeClassMethodSpec<'_>],
) -> u64 {
    init_cached_runtime_class_configured(
        _py,
        slot,
        name,
        layout_size,
        instance_shape,
        |_class_bits, dict_ptr| configure_runtime_class_methods(_py, dict_ptr, methods),
    )
}

pub(crate) fn configure_runtime_class_methods(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    methods: &[RuntimeClassMethodSpec<'_>],
) -> bool {
    for method in methods {
        let bits = if let Some(signature) = method.signature {
            crate::builtins::methods::builtin_func_bits_with_signature(
                _py,
                method.slot,
                method.fn_ptr,
                method.arity,
                signature.arg_names,
                signature.has_vararg,
                signature.has_varkw,
            )
        } else {
            crate::builtins::methods::builtin_func_bits(
                _py,
                method.slot,
                method.fn_ptr,
                method.arity,
            )
        };
        if !set_class_method(_py, dict_ptr, method.name, bits) {
            return false;
        }
    }
    true
}

#[must_use]
fn set_class_method(_py: &PyToken<'_>, dict_ptr: *mut u8, name: &str, fn_bits: u64) -> bool {
    if fn_bits == 0 || exception_pending(_py) {
        return false;
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        if !exception_pending(_py) {
            let _ =
                raise_exception::<u64>(_py, "MemoryError", "class method name allocation failed");
        }
        return false;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, fn_bits) };
    let published = unsafe { dict_get_in_place(_py, dict_ptr, name_bits) } == Some(fn_bits);
    dec_ref_bits(_py, name_bits);
    published && !exception_pending(_py)
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
        if locked_len(pair_ptr) < 2 {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let Some(val) = pin_item(_py, pair_ptr, 0) else {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        };
        let Some(done_value) = pin_item(_py, pair_ptr, 1) else {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        };
        let val_bits = val.bits();
        let done = is_truthy(_py, obj_from_bits(done_value.bits()));
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
        if !crate::builtins::callable::is_callable_impl(_py, func_bits) {
            return raise_exception::<_>(_py, "TypeError", "types.coroutine() expects a callable");
        }
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "types.coroutine() expects a function");
        };
        unsafe {
            if object_type_id(func_ptr) != crate::TYPE_ID_FUNCTION {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "types.coroutine() runtime path expects a function",
                );
            }
            let code_bits = crate::function_code_bits(func_ptr);
            let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "types.coroutine() function has no code object",
                );
            };
            if object_type_id(code_ptr) != crate::TYPE_ID_CODE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "types.coroutine() function has invalid code object",
                );
            }
            match crate::object::layout::code_execution_kind(code_ptr) {
                crate::object::layout::CodeExecutionKind::Coroutine => {
                    inc_ref_bits(_py, func_bits);
                    func_bits
                }
                crate::object::layout::CodeExecutionKind::Generator => {
                    if crate::object::layout::code_protocol_flags(code_ptr)
                        & crate::object::layout::CO_ITERABLE_COROUTINE
                        != 0
                    {
                        inc_ref_bits(_py, func_bits);
                        return func_bits;
                    }
                    let clone = crate::object::builders::clone_code_obj_with_protocol_flags(
                        _py,
                        code_ptr,
                        crate::object::layout::CO_ITERABLE_COROUTINE,
                    );
                    if clone.is_null() {
                        return MoltObject::none().bits();
                    }
                    let clone_bits = MoltObject::from_ptr(clone).bits();
                    let attached = crate::function_set_code_bits(_py, func_ptr, clone_bits);
                    dec_ref_bits(_py, clone_bits);
                    if !attached {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, func_bits);
                    func_bits
                }
                crate::object::layout::CodeExecutionKind::Direct
                | crate::object::layout::CodeExecutionKind::AsyncGenerator => raise_exception::<_>(
                    _py,
                    "TypeError",
                    "types.coroutine() runtime path expects a generator or coroutine function",
                ),
            }
        }
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
        if !exception_pending(_py) {
            let _ = raise_exception::<u64>(_py, "MemoryError", "types module allocation failed");
        }
        return 0;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let release_failed_payload = || {
        dec_ref_bits(_py, dict_bits);
        if !exception_pending(_py) {
            let _ =
                raise_exception::<u64>(_py, "MemoryError", "types module initialization failed");
        }
        0
    };
    let builtins = builtin_classes(_py);
    trace_stage("builtins");
    let mappingproxy_bits = mappingproxy_class(_py);
    trace_stage("mappingproxy");
    if mappingproxy_bits == 0 {
        return release_failed_payload();
    }
    let simplenamespace_bits = simplenamespace_class(_py);
    trace_stage("simplenamespace");
    if simplenamespace_bits == 0 {
        return release_failed_payload();
    }
    let capsule_bits = capsule_class(_py);
    trace_stage("capsule");
    if capsule_bits == 0 {
        return release_failed_payload();
    }
    let cell_bits = cell_class(_py);
    trace_stage("cell");
    if cell_bits == 0 {
        return release_failed_payload();
    }
    let dynamic_class_attr_bits = dynamic_class_attribute_class(_py);
    trace_stage("dynamic_class_attribute");
    if dynamic_class_attr_bits == 0 {
        return release_failed_payload();
    }

    let method_type_bits = method_class(_py);
    trace_stage("method_type_done");
    if method_type_bits == 0 {
        return release_failed_payload();
    }

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
    let getset_descriptor_bits = getset_descriptor_class(_py);
    trace_stage("getset_descriptor");
    if getset_descriptor_bits == 0 {
        return release_failed_payload();
    }
    let member_descriptor_bits = member_descriptor_class(_py);
    trace_stage("member_descriptor");
    if member_descriptor_bits == 0 {
        return release_failed_payload();
    }

    let coroutine_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_coroutine_fn,
        crate::molt_types_coroutine as *const () as usize as u64,
        1,
        None,
    );
    if coroutine_bits == 0 {
        return release_failed_payload();
    }
    trace_stage("coroutine_bits");

    let get_original_bases_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_get_original_bases_fn,
        crate::molt_types_get_original_bases as *const () as usize as u64,
        1,
        None,
    );
    if get_original_bases_bits == 0 {
        return release_failed_payload();
    }
    trace_stage("get_original_bases");

    let prepare_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_prepare_class_fn,
        crate::molt_types_prepare_class as *const () as usize as u64,
        2,
        Some((NO_RUNTIME_ARGUMENT_NAMES, true, true)),
    );
    if prepare_bits == 0 {
        return release_failed_payload();
    }
    trace_stage("prepare_bits");

    let resolve_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_resolve_bases_fn,
        crate::molt_types_resolve_bases as *const () as usize as u64,
        2,
        Some((NO_RUNTIME_ARGUMENT_NAMES, true, true)),
    );
    if resolve_bits == 0 {
        return release_failed_payload();
    }
    trace_stage("resolve_bits");

    let new_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_new_class_fn,
        crate::molt_types_new_class as *const () as usize as u64,
        2,
        Some((NO_RUNTIME_ARGUMENT_NAMES, true, true)),
    );
    if new_bits == 0 {
        return release_failed_payload();
    }
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
    use crate::MoltHeader;
    use crate::object::maybe_ptr_from_bits;
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
        unsafe { (*header).ref_count_snapshot() }
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
    fn cached_runtime_class_is_not_published_before_configuration_succeeds() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            let slot = AtomicU64::new(0);
            let class_bits = init_cached_runtime_class_configured(
                _py,
                &slot,
                "FailureAtomicClass",
                0,
                None,
                |_class, _dict| false,
            );
            assert_eq!(class_bits, 0);
            assert_eq!(slot.load(Ordering::Acquire), 0);
            assert!(exception_pending(_py));
            clear_exception(_py);
        });
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
                    Some((NO_RUNTIME_ARGUMENT_NAMES, true, true)),
                );
                assert_ne!(func_bits, 0);
                let func_ptr = maybe_ptr_from_bits(func_bits).expect("prepare_class function");

                let first_dict_bits = function_dict_bits(func_ptr);
                assert_ne!(
                    first_dict_bits, 0,
                    "vararg marker must install a function dict"
                );

                assert!(crate::builtins::methods::configure_builtin_signature(
                    _py,
                    func_bits,
                    &[],
                    true,
                    true,
                ));
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
                    locked_len(arg_names_ptr),
                    0,
                    "non-self vararg helpers still need an explicit empty arg-name tuple"
                );
            }
        });
    }

    #[test]
    fn types_coroutine_clones_generator_code_without_mutating_shared_authority() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                use crate::object::layout::{
                    CO_GENERATOR, CO_ITERABLE_COROUTINE, CodeExecutionKind, code_arg_names_bits,
                    code_argcount, code_callable_arity, code_callable_fn_ptr,
                    code_callable_identity, code_callable_trampoline_ptr, code_execution_kind,
                    code_filename_bits, code_firstlineno, code_flags, code_frame_slot_id,
                    code_kwonly_names_bits, code_kwonlyargcount, code_linetable_bits,
                    code_name_bits, code_names_bits, code_posonlyargcount,
                    code_publish_execution_kind, code_set_frame_slot_id, code_set_signature_bits,
                    code_signature_posonly_bits, code_vararg_bits, code_varkw_bits,
                    code_varnames_bits, function_code_bits, function_set_code_bits,
                    function_set_trampoline_ptr,
                };

                let filename = alloc_string(_py, b"coroutine_clone.py");
                let name = alloc_string(_py, b"shared_generator");
                let arg_name = alloc_string(_py, b"arg");
                let vararg = alloc_string(_py, b"args");
                let varkw = alloc_string(_py, b"kwargs");
                let empty = alloc_tuple(_py, &[]);
                assert!(
                    !filename.is_null()
                        && !name.is_null()
                        && !arg_name.is_null()
                        && !vararg.is_null()
                        && !varkw.is_null()
                        && !empty.is_null()
                );
                let filename_bits = MoltObject::from_ptr(filename).bits();
                let name_bits = MoltObject::from_ptr(name).bits();
                let arg_name_bits = MoltObject::from_ptr(arg_name).bits();
                let vararg_bits = MoltObject::from_ptr(vararg).bits();
                let varkw_bits = MoltObject::from_ptr(varkw).bits();
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let arg_names = alloc_tuple(_py, &[arg_name_bits]);
                assert!(!arg_names.is_null());
                let arg_names_bits = MoltObject::from_ptr(arg_names).bits();
                let posonly_bits = MoltObject::from_int(1).bits();

                let original = crate::alloc_code_obj(
                    _py,
                    filename_bits,
                    name_bits,
                    37,
                    MoltObject::none().bits(),
                    arg_names_bits,
                    empty_bits,
                    1,
                    1,
                    0,
                );
                assert!(!original.is_null());
                let original_bits = MoltObject::from_ptr(original).bits();
                let first = crate::alloc_function_obj(_py, 0x101, 1);
                let sibling = crate::alloc_function_obj(_py, 0x303, 9);
                assert!(!first.is_null() && !sibling.is_null());
                let first_bits = MoltObject::from_ptr(first).bits();
                let sibling_bits = MoltObject::from_ptr(sibling).bits();

                function_set_trampoline_ptr(first, 0x202);
                assert!(function_set_code_bits(_py, first, original_bits));
                code_set_signature_bits(
                    _py,
                    original,
                    arg_names_bits,
                    posonly_bits,
                    empty_bits,
                    vararg_bits,
                    varkw_bits,
                )
                .expect("valid compiled signature");
                code_set_frame_slot_id(original, 23);
                assert_eq!(
                    code_publish_execution_kind(original, CodeExecutionKind::Generator),
                    Ok(())
                );

                // A different function cannot attach after publication or rewrite the
                // shared code object's callable or signature metadata.
                function_set_trampoline_ptr(sibling, 0x404);
                assert!(!function_set_code_bits(_py, sibling, original_bits));
                assert!(exception_pending(_py));
                crate::clear_exception(_py);
                assert_eq!(function_code_bits(sibling), 0);
                code_set_signature_bits(
                    _py,
                    original,
                    empty_bits,
                    MoltObject::from_int(0).bits(),
                    arg_names_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                )
                .expect("published signature remains immutable");
                assert_eq!(code_callable_fn_ptr(original), 0x101);
                assert_eq!(code_callable_trampoline_ptr(original), 0x202);
                assert_eq!(code_callable_arity(original), 1);
                assert_eq!(
                    code_callable_identity(original).unwrap().call_abi,
                    crate::FunctionCallAbi::Positional
                );
                assert_eq!(code_arg_names_bits(original), arg_names_bits);
                assert_eq!(code_signature_posonly_bits(original), posonly_bits);
                assert_eq!(code_kwonly_names_bits(original), empty_bits);
                assert_eq!(code_vararg_bits(original), vararg_bits);
                assert_eq!(code_varkw_bits(original), varkw_bits);

                let decorated_result = molt_types_coroutine(first_bits);
                assert_eq!(decorated_result, first_bits);
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, decorated_result);

                let clone_bits = function_code_bits(first);
                assert_ne!(clone_bits, original_bits);
                assert_eq!(function_code_bits(sibling), 0);
                let clone = obj_from_bits(clone_bits)
                    .as_ptr()
                    .expect("cloned code object");
                assert_eq!(code_execution_kind(original), CodeExecutionKind::Generator);
                assert_eq!(code_execution_kind(clone), CodeExecutionKind::Generator);
                assert_eq!(
                    code_flags(original) & (CO_GENERATOR | CO_ITERABLE_COROUTINE),
                    CO_GENERATOR
                );
                assert_eq!(
                    code_flags(clone) & (CO_GENERATOR | CO_ITERABLE_COROUTINE),
                    CO_GENERATOR | CO_ITERABLE_COROUTINE
                );

                assert_eq!(code_filename_bits(clone), code_filename_bits(original));
                assert_eq!(code_name_bits(clone), code_name_bits(original));
                assert_eq!(code_firstlineno(clone), code_firstlineno(original));
                assert_eq!(code_linetable_bits(clone), code_linetable_bits(original));
                assert_eq!(code_varnames_bits(clone), code_varnames_bits(original));
                assert_eq!(code_names_bits(clone), code_names_bits(original));
                assert_eq!(code_argcount(clone), code_argcount(original));
                assert_eq!(code_posonlyargcount(clone), code_posonlyargcount(original));
                assert_eq!(code_kwonlyargcount(clone), code_kwonlyargcount(original));
                assert_eq!(code_callable_fn_ptr(clone), code_callable_fn_ptr(original));
                assert_eq!(
                    code_callable_trampoline_ptr(clone),
                    code_callable_trampoline_ptr(original)
                );
                assert_eq!(code_callable_arity(clone), code_callable_arity(original));
                assert_eq!(
                    code_callable_identity(clone),
                    code_callable_identity(original)
                );
                assert_eq!(code_frame_slot_id(clone), code_frame_slot_id(original));
                assert_eq!(code_arg_names_bits(clone), code_arg_names_bits(original));
                assert_eq!(
                    code_signature_posonly_bits(clone),
                    code_signature_posonly_bits(original)
                );
                assert_eq!(
                    code_kwonly_names_bits(clone),
                    code_kwonly_names_bits(original)
                );
                assert_eq!(code_vararg_bits(clone), code_vararg_bits(original));
                assert_eq!(code_varkw_bits(clone), code_varkw_bits(original));

                let repeated_result = molt_types_coroutine(first_bits);
                assert_eq!(repeated_result, first_bits);
                assert_eq!(function_code_bits(first), clone_bits);
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, repeated_result);

                dec_ref_bits(_py, first_bits);
                dec_ref_bits(_py, sibling_bits);
                dec_ref_bits(_py, original_bits);
                dec_ref_bits(_py, arg_names_bits);
                dec_ref_bits(_py, empty_bits);
                dec_ref_bits(_py, varkw_bits);
                dec_ref_bits(_py, vararg_bits);
                dec_ref_bits(_py, arg_name_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, filename_bits);
            }
        });
    }

    #[test]
    fn types_coroutine_is_identity_for_coroutines_and_rejects_noncallables() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                use crate::object::layout::{
                    CodeExecutionKind, code_publish_execution_kind, function_set_code_bits,
                };

                let name = alloc_string(_py, b"native_coroutine");
                let empty = alloc_tuple(_py, &[]);
                assert!(!name.is_null() && !empty.is_null());
                let name_bits = MoltObject::from_ptr(name).bits();
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let code = crate::alloc_code_obj(
                    _py,
                    name_bits,
                    name_bits,
                    1,
                    MoltObject::none().bits(),
                    empty_bits,
                    empty_bits,
                    0,
                    0,
                    0,
                );
                let function = crate::alloc_function_obj(_py, 0x505, 0);
                assert!(!code.is_null() && !function.is_null());
                let code_bits = MoltObject::from_ptr(code).bits();
                let function_bits = MoltObject::from_ptr(function).bits();
                assert!(function_set_code_bits(_py, function, code_bits));
                assert_eq!(
                    code_publish_execution_kind(code, CodeExecutionKind::Coroutine),
                    Ok(())
                );

                let result = molt_types_coroutine(function_bits);
                assert_eq!(result, function_bits);
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, result);

                let rejected = molt_types_coroutine(MoltObject::from_int(7).bits());
                assert!(obj_from_bits(rejected).is_none());
                assert!(exception_pending(_py));
                clear_exception(_py);

                dec_ref_bits(_py, function_bits);
                dec_ref_bits(_py, code_bits);
                dec_ref_bits(_py, empty_bits);
                dec_ref_bits(_py, name_bits);
            }
        });
    }

    #[test]
    fn cell_class_publishes_comparison_hash_and_contents_protocol() {
        init_runtime();
        crate::with_gil_entry_nopanic!(py, {
            let class_bits = cell_class(py);
            assert_ne!(class_bits, 0);
            assert!(!exception_pending(py));
            let class = obj_from_bits(class_bits).as_ptr().unwrap();
            let dict_bits = unsafe { class_dict_bits(class) };
            let dict = obj_from_bits(dict_bits).as_ptr().unwrap();

            for name in ["__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"] {
                let key = attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
                let method = unsafe { dict_get_in_place(py, dict, key) }.unwrap();
                assert!(crate::builtins::callable::is_callable_impl(py, method));
                dec_ref_bits(py, key);
            }

            let hash_key = attr_name_bits_from_bytes(py, b"__hash__").unwrap();
            assert!(
                obj_from_bits(unsafe { dict_get_in_place(py, dict, hash_key) }.unwrap()).is_none()
            );
            dec_ref_bits(py, hash_key);

            let contents_key = attr_name_bits_from_bytes(py, b"cell_contents").unwrap();
            let descriptor = unsafe { dict_get_in_place(py, dict, contents_key) }.unwrap();
            let descriptor_ptr = obj_from_bits(descriptor).as_ptr().unwrap();
            assert_eq!(
                unsafe { crate::object::layout::native_descriptor_flavor(descriptor_ptr) },
                Some(NativeDescriptorFlavor::GetSet),
            );
            assert_eq!(
                unsafe { crate::object::layout::native_descriptor_owner_bits(descriptor_ptr) },
                class_bits,
            );
            dec_ref_bits(py, contents_key);
        });
    }
}
