use super::*;
use crate::BuiltinExc;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::builtins::methods::is_missing_bits;
use crate::object::seq_access::with_immutable_tuple_slice;

unsafe fn mappingproxy_mapping_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn mappingproxy_set_mapping_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

pub(crate) fn mappingproxy_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let methods = [
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            molt_types_mappingproxy_new as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderInit,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_init as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGetitem,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_getitem as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderIter,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_iter as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLen,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_len as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderContains,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_contains as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::Get,
            NativeCallableKind::MethodDescriptor,
            molt_types_mappingproxy_get as *const () as usize as u64,
            3,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Keys,
            NativeCallableKind::MethodDescriptor,
            molt_types_mappingproxy_keys as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Items,
            NativeCallableKind::MethodDescriptor,
            molt_types_mappingproxy_items as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Values,
            NativeCallableKind::MethodDescriptor,
            molt_types_mappingproxy_values as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderRepr,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_repr as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Copy,
            NativeCallableKind::MethodDescriptor,
            mappingproxy_copy as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderReversed,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_reversed as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderStr,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_str as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderHash,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_hash as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderIor,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_ior as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderOr,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_or as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderRor,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_ror as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderEq,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderNe,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_ne as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLt,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_lt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLe,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_le as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGt,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_gt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGe,
            NativeCallableKind::WrapperDescriptor,
            mappingproxy_ge as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderSetitem,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_setitem as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderDelitem,
            NativeCallableKind::WrapperDescriptor,
            molt_types_mappingproxy_delitem as *const () as usize as u64,
            2,
        ),
    ];
    init_cached_runtime_class_configured(
        _py,
        &state.mappingproxy_class,
        "mappingproxy",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            native_module: None,
            layout_size: 16,
            instance_shape: Some(crate::object::ObjectShapeId::TypesMappingProxy),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        |class_bits, dict_ptr| {
            use molt_cpython_abi::hooks::NativeProtocolSlot as P;
            unsafe {
                crate::object::class_storage::class_declare_native_protocols(
                    obj_from_bits(class_bits).as_ptr().unwrap(),
                    &[P::MappingLength, P::MappingSubscript, P::SequenceContains],
                )
            };
            configure_runtime_class_methods(_py, class_bits, dict_ptr, &methods)
        },
    )
}

pub(crate) fn mappingproxy_from_mapping(py: &PyToken<'_>, mapping: u64) -> u64 {
    let class = mappingproxy_class(py);
    if class == 0 || exception_pending(py) {
        return MoltObject::none().bits();
    }
    molt_types_mappingproxy_new_attached_body(py, class, mapping)
}

pub(crate) fn mappingproxy_class_bits(_py: &PyToken<'_>) -> u64 {
    mappingproxy_class(_py)
}

/// Publish `frame.f_locals`, the read-only getset over each frame object's
/// typed binding source, and `frame.clear()` over the same owner
/// (`builtins/frames/locals_proxy.rs`), before the first frame object is
/// materialized. `false`: an exception is pending.
pub(crate) fn frame_class_ready(_py: &PyToken<'_>) -> bool {
    let state = types_state(_py);
    if state
        .frame_f_locals_descriptor
        .load(AtomicOrdering::Acquire)
        != 0
    {
        return true;
    }
    let descriptor = init_atomic_bits(_py, &state.frame_f_locals_descriptor, || {
        let frame_class = builtin_classes(_py).frame;
        let Some(class_ptr) = obj_from_bits(frame_class).as_ptr() else {
            return 0;
        };
        let Some(dict_ptr) = obj_from_bits(unsafe { class_dict_bits(class_ptr) }).as_ptr() else {
            return 0;
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return 0;
        }
        let getter = builtin_func_bits(
            _py,
            NativeCallableSpec::function(&state.frame_f_locals_get_fn),
            crate::builtins::frames::molt_frame_f_locals_get as *const () as usize as u64,
            2,
        );
        if getter == 0 || exception_pending(_py) {
            return 0;
        }
        let Some(name) = crate::NameId::FLocals.owned_bits(_py) else {
            return 0;
        };
        let none = MoltObject::none().bits();
        let descriptor = alloc_native_descriptor(
            _py,
            NativeDescriptorSpec {
                flavor: NativeDescriptorFlavor::GetSet,
                operation: 0,
                owner: frame_class,
                name,
                doc: none,
                getter,
                setter: none,
                deleter: none,
            },
        );
        dec_ref_bits(_py, name);
        if descriptor == 0 || exception_pending(_py) {
            return 0;
        }
        if !set_class_method(_py, dict_ptr, crate::NameId::FLocals, descriptor) {
            dec_ref_bits(_py, descriptor);
            return 0;
        }
        let getters: [(crate::NameId, &AtomicU64, extern "C" fn(u64, u64) -> u64); 2] = [
            (
                crate::NameId::FLineno,
                &state.frame_f_lineno_get_fn,
                crate::builtins::frames::molt_frame_f_lineno_get,
            ),
            (
                crate::NameId::FLasti,
                &state.frame_f_lasti_get_fn,
                crate::builtins::frames::molt_frame_f_lasti_get,
            ),
        ];
        for (field, function_slot, entry) in getters {
            let getter = builtin_func_bits(
                _py,
                NativeCallableSpec::function(function_slot),
                entry as *const () as usize as u64,
                2,
            );
            let Some(name) = field.owned_bits(_py) else {
                dec_ref_bits(_py, descriptor);
                return 0;
            };
            if getter == 0 || exception_pending(_py) {
                dec_ref_bits(_py, name);
                dec_ref_bits(_py, descriptor);
                return 0;
            }
            let location_descriptor = alloc_native_descriptor(
                _py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::GetSet,
                    operation: 0,
                    owner: frame_class,
                    name,
                    doc: none,
                    getter,
                    setter: none,
                    deleter: none,
                },
            );
            dec_ref_bits(_py, name);
            if location_descriptor == 0
                || exception_pending(_py)
                || !set_class_method(_py, dict_ptr, field, location_descriptor)
            {
                dec_ref_bits(_py, descriptor);
                return 0;
            }
            dec_ref_bits(_py, location_descriptor);
        }
        let clear = builtin_func_bits(
            _py,
            NativeCallableSpec::declared(
                NativeCallableKind::MethodDescriptor,
                builtin_classes(_py).frame,
                crate::NameId::Clear,
            ),
            crate::builtins::frames::molt_frame_clear as *const () as usize as u64,
            1,
        );
        if clear == 0
            || exception_pending(_py)
            || !set_class_method(_py, dict_ptr, crate::NameId::Clear, clear)
        {
            dec_ref_bits(_py, descriptor);
            return 0;
        }
        unsafe { class_bump_layout_version(class_ptr) };
        // The runtime state keeps this reference; it marks publication done.
        descriptor
    });
    if descriptor == 0 && !exception_pending(_py) {
        let _ = raise_exception::<u64>(
            _py,
            BuiltinExc::SystemError,
            "frame.f_locals publication failed",
        );
    }
    descriptor != 0
}

/// PEP 667's view of an optimized frame's bindings (3.13 onward). Instances
/// exist only as `frame.f_locals`; each holds its frame's binding source.
pub(crate) fn frame_locals_proxy_class(_py: &PyToken<'_>) -> u64 {
    use crate::builtins::frames::{
        molt_frame_locals_proxy_contains, molt_frame_locals_proxy_copy,
        molt_frame_locals_proxy_delitem, molt_frame_locals_proxy_eq, molt_frame_locals_proxy_get,
        molt_frame_locals_proxy_getitem, molt_frame_locals_proxy_items,
        molt_frame_locals_proxy_iter, molt_frame_locals_proxy_keys, molt_frame_locals_proxy_len,
        molt_frame_locals_proxy_pop, molt_frame_locals_proxy_repr,
        molt_frame_locals_proxy_setdefault, molt_frame_locals_proxy_setitem,
        molt_frame_locals_proxy_update, molt_frame_locals_proxy_values,
    };
    let state = types_state(_py);
    let varargs = RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true);
    let methods = [
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGetitem,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_getitem as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderContains,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_contains as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLen,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_len as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderIter,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_iter as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::Get,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_get as *const () as usize as u64,
            3,
            varargs,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Keys,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_keys as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Values,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_values as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Items,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_items as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::Copy,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_copy as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderRepr,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_repr as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderEq,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderSetitem,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_setitem as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderDelitem,
            NativeCallableKind::WrapperDescriptor,
            molt_frame_locals_proxy_delitem as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::Setdefault,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_setdefault as *const () as usize as u64,
            3,
            varargs,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::Pop,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_pop as *const () as usize as u64,
            3,
            varargs,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::Update,
            NativeCallableKind::MethodDescriptor,
            molt_frame_locals_proxy_update as *const () as usize as u64,
            3,
            varargs,
        ),
    ];
    init_cached_runtime_class_configured(
        _py,
        &state.frame_locals_proxy_class,
        "FrameLocalsProxy",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            native_module: None,
            layout_size: 16,
            instance_shape: Some(crate::object::ObjectShapeId::TypesFrameLocalsProxy),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        |class_bits, dict_ptr| {
            use molt_cpython_abi::hooks::NativeProtocolSlot as P;
            unsafe {
                crate::object::class_storage::class_declare_native_protocols(
                    obj_from_bits(class_bits).as_ptr().unwrap(),
                    &[
                        P::MappingLength,
                        P::MappingSubscript,
                        P::MappingAssignSubscript,
                        P::SequenceContains,
                    ],
                )
            };
            configure_runtime_class_methods(_py, class_bits, dict_ptr, &methods)
                && set_class_method(
                    _py,
                    dict_ptr,
                    crate::NameId::DunderHash,
                    MoltObject::none().bits(),
                )
        },
    )
}

/// Ordinary Python methods only; native bound callable classes keep their own protocol.
pub(crate) fn method_ptr_from_bits(py: &PyToken<'_>, bits: u64) -> Option<*mut u8> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != crate::TYPE_ID_BOUND_METHOD {
        return None;
    }
    (unsafe { crate::object_class_bits(ptr) } == method_class(py)).then_some(ptr)
}

pub(crate) fn method_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let methods = [
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            molt_types_method_new as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderInit,
            NativeCallableKind::WrapperDescriptor,
            molt_types_method_init as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderEq,
            NativeCallableKind::WrapperDescriptor,
            crate::object::ops_compare::molt_method_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderNe,
            NativeCallableKind::WrapperDescriptor,
            crate::object::ops_compare::molt_method_ne as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderHash,
            NativeCallableKind::WrapperDescriptor,
            crate::object::ops_hash::molt_method_hash as *const () as usize as u64,
            1,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &state.method_class,
        "method",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            native_module: None,
            layout_size: 16,
            instance_shape: None,
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::native(
                crate::TYPE_ID_BOUND_METHOD,
            )),
        },
        &methods,
    )
}

pub(crate) fn simplenamespace_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let methods = [
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderInit,
            NativeCallableKind::WrapperDescriptor,
            molt_types_simplenamespace_init as *const () as usize as u64,
            3,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderRepr,
            NativeCallableKind::WrapperDescriptor,
            molt_types_simplenamespace_repr as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderEq,
            NativeCallableKind::WrapperDescriptor,
            molt_types_simplenamespace_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderReplace,
            NativeCallableKind::MethodDescriptor,
            molt_types_simplenamespace_replace as *const () as usize as u64,
            2,
            RuntimeMethodSignature::new(NO_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
    ];
    let target = crate::object::ops_sys::runtime_target_python_info(runtime_state(_py));
    let method_count = if (target.major, target.minor) >= (3, 13) {
        4
    } else {
        3
    };
    init_cached_runtime_class(
        _py,
        &state.simplenamespace_class,
        "SimpleNamespace",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(true),
            native_module: Some("types"),
            layout_size: 8,
            instance_shape: None,
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy {
                allows_dict: true,
                allows_weakref: false,
                variable_sized: false,
            }),
        },
        &methods[..method_count],
    )
}

pub(crate) fn capsule_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let methods = [RuntimeClassMethodSpec::fixed(
        crate::NameId::DunderNew,
        NativeCallableKind::Constructor,
        molt_types_capsule_new as *const () as usize as u64,
        1,
    )];
    init_cached_runtime_class(
        _py,
        &state.capsule_class,
        "capsule",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}

pub(crate) fn cell_class(_py: &PyToken<'_>) -> u64 {
    let state = types_state(_py);
    let methods = [
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            molt_types_cell_new as *const () as usize as u64,
            3,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderEq,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderNe,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_ne as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLt,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_lt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderLe,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_le as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGt,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_gt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderGe,
            NativeCallableKind::WrapperDescriptor,
            crate::molt_cell_ge as *const () as usize as u64,
            2,
        ),
    ];
    init_cached_runtime_class_configured(
        _py,
        &state.cell_class,
        "cell",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::static_type(false),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        |class_bits, dict_ptr| {
            if !configure_runtime_class_methods(_py, class_bits, dict_ptr, &methods)
                || !set_class_method(
                    _py,
                    dict_ptr,
                    crate::NameId::DunderHash,
                    MoltObject::none().bits(),
                )
            {
                return false;
            }
            let getter = builtin_func_bits(
                _py,
                NativeCallableSpec::function(&state.cell_contents_get_fn),
                molt_types_cell_contents_get as *const () as usize as u64,
                2,
            );
            let setter = builtin_func_bits(
                _py,
                NativeCallableSpec::function(&state.cell_contents_set_fn),
                molt_types_cell_contents_set as *const () as usize as u64,
                3,
            );
            let deleter = builtin_func_bits(
                _py,
                NativeCallableSpec::function(&state.cell_contents_delete_fn),
                molt_types_cell_contents_delete as *const () as usize as u64,
                2,
            );
            if [getter, setter, deleter]
                .into_iter()
                .any(|bits| bits == 0 || exception_pending(_py))
            {
                return false;
            }
            let Some(name) = crate::NameId::CellContents.owned_bits(_py) else {
                return false;
            };
            let descriptor = alloc_native_descriptor(
                _py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::GetSet,
                    operation: 0,
                    owner: class_bits,
                    name,
                    doc: MoltObject::none().bits(),
                    getter,
                    setter,
                    deleter,
                },
            );
            dec_ref_bits(_py, name);
            if descriptor == 0 || exception_pending(_py) {
                return false;
            }
            let published =
                set_class_method(_py, dict_ptr, crate::NameId::CellContents, descriptor);
            dec_ref_bits(_py, descriptor);
            published
        },
    )
}

pub(crate) fn molt_types_method_new_attached_body(
    _py: &crate::PyToken<'_>,
    _cls_bits: u64,
    func_bits: u64,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if !crate::builtins::callable::is_callable_impl(_py, func_bits) {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "first argument must be callable",
            );
        }
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "instance must not be None");
        }
        crate::builtins::functions::explicit_bound_method_new(_py, func_bits, self_bits)
    })
}

pub(crate) fn molt_types_method_init_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _func_bits: u64,
    _self_arg: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { MoltObject::none().bits() })
}

pub(crate) fn molt_types_mappingproxy_new_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    mapping_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if obj_from_bits(mapping_bits).is_none() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "mappingproxy() argument cannot be None",
            );
        }
        match mappingproxy_admits(_py, mapping_bits) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "mappingproxy() argument must be a mapping",
                );
            }
            Err(()) => return MoltObject::none().bits(),
        }
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "mappingproxy() expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "mappingproxy() expects type",
                );
            }
        }
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            mappingproxy_set_mapping_bits(inst_ptr, mapping_bits);
        }
        inc_ref_bits(_py, mapping_bits);
        inst_bits
    })
}

pub(crate) fn molt_types_mappingproxy_init_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _mapping_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { MoltObject::none().bits() })
}

pub(crate) fn molt_types_mappingproxy_getitem_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_index_attached_body(_py, mapping_bits, key_bits)
    })
}

pub(crate) fn molt_types_mappingproxy_iter_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        let iter_bits = molt_iter_attached_body(_py, mapping_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, mapping_bits);
        }
        iter_bits
    })
}

pub(crate) fn molt_types_mappingproxy_len_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_len_attached_body(_py, mapping_bits)
    })
}

pub(crate) fn molt_types_mappingproxy_contains_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_contains_attached_body(_py, mapping_bits, key_bits)
    })
}

pub(crate) fn molt_types_mappingproxy_get_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let Some(args_ptr) = args_ptr else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "mappingproxy.get() expects arguments",
            );
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "mappingproxy.get() expects arguments",
                );
            }
        }
        let Some((args_len, key_bits, default_bits)) = (unsafe {
            with_immutable_tuple_slice(args_ptr, |args| {
                let default_bits = args
                    .get(1)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                (args.len(), args.first().copied(), default_bits)
            })
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "mappingproxy.get() expects arguments",
            );
        };
        if args_len == 0 || args_len > 2 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "mappingproxy.get() takes 1 or 2 arguments",
            );
        }
        if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT && crate::dict_len(kwargs_ptr) != 0 {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "mappingproxy.get() takes no keyword arguments",
                    );
                }
            }
        }
        let key_bits = key_bits.expect("non-empty mappingproxy.get arguments");
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        if obj_from_bits(mapping_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT })
        {
            return molt_dict_get_attached_body(_py, mapping_bits, key_bits, default_bits);
        }
        let Some(name_bits) = crate::NameId::Get.owned_bits(_py) else {
            return MoltObject::none().bits();
        };
        let method =
            molt_getattr_builtin_attached_body(_py, mapping_bits, name_bits, missing_bits(_py));
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if is_missing_bits(_py, method) {
            return raise_exception::<_>(_py, BuiltinExc::AttributeError, "get");
        }
        let result = unsafe { call_callable2(_py, method, key_bits, default_bits) };
        dec_ref_bits(_py, method);
        result
    })
}

fn mappingproxy_call_noargs(_py: &PyToken<'_>, self_bits: u64, name: &str) -> u64 {
    let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
    let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
    let missing = missing_bits(_py);
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let method_bits = molt_getattr_builtin_attached_body(_py, mapping_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if method_bits == missing {
        return raise_exception::<_>(_py, BuiltinExc::AttributeError, name);
    }
    let res_bits = unsafe { call_callable0(_py, method_bits) };
    dec_ref_bits(_py, method_bits);
    res_bits
}

pub(crate) fn molt_types_mappingproxy_keys_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        mappingproxy_call_noargs(_py, self_bits, "keys")
    })
}

pub(crate) fn molt_types_mappingproxy_items_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        mappingproxy_call_noargs(_py, self_bits, "items")
    })
}

pub(crate) fn molt_types_mappingproxy_values_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        mappingproxy_call_noargs(_py, self_bits, "values")
    })
}

pub(crate) fn molt_types_mappingproxy_repr_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        let mapping_repr_bits = molt_repr_from_obj_attached_body(_py, mapping_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mapping_repr =
            string_obj_to_owned(obj_from_bits(mapping_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, mapping_repr_bits);
        let out = format!("mappingproxy({mapping_repr})");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

pub(crate) fn molt_types_mappingproxy_setitem_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _key_bits: u64,
    _val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "'mappingproxy' object does not support item assignment",
        )
    })
}

pub(crate) fn molt_types_mappingproxy_delitem_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _key_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "'mappingproxy' object does not support item deletion",
        )
    })
}

// These methods delegate to the same mapping retained by the read-only view.
// They do not copy the namespace or publish a second C proxy representation.
fn mappingproxy_admits(py: &PyToken<'_>, mapping: u64) -> Result<bool, ()> {
    unsafe {
        let roots = builtin_classes(py);
        for excluded in [roots.list, roots.tuple] {
            match crate::object::class_layout::try_is_real_instance(py, mapping, excluded) {
                Ok(true) => return Ok(false),
                Ok(false) => {}
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "mappingproxy receiver admission",
                    );
                    return Err(());
                }
            }
        }
        crate::object::ops::value_supports_mp_subscript(py, mapping)
    }
}

extern "C" fn mappingproxy_copy(__context: u64, self_bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        mappingproxy_call_noargs(py, self_bits, "copy")
    })
}

extern "C" fn mappingproxy_reversed(__context: u64, self_bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, _py, {
        let pointer = obj_from_bits(self_bits).as_ptr().unwrap();
        crate::molt_reversed_builtin_attached_body(_py, unsafe {
            mappingproxy_mapping_bits(pointer)
        })
    })
}

extern "C" fn mappingproxy_str(__context: u64, self_bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::molt_str_from_obj_attached_body(__attached_py, mappingproxy_unwrap(self_bits))
}

extern "C" fn mappingproxy_hash(__context: u64, self_bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::molt_hash_builtin_attached_body(__attached_py, mappingproxy_unwrap(self_bits))
}

extern "C" fn mappingproxy_ior(__context: u64, _self_bits: u64, _other: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        raise_exception::<_>(
            py,
            BuiltinExc::TypeError,
            "'|=' is not supported by mappingproxy; use '|' instead",
        )
    })
}

fn mappingproxy_unwrap(bits: u64) -> u64 {
    match obj_from_bits(bits).as_ptr() {
        Some(pointer)
            if crate::object::object_shape_id(pointer)
                == crate::object::ObjectShapeId::TypesMappingProxy =>
        unsafe { mappingproxy_mapping_bits(pointer) },
        _ => bits,
    }
}

extern "C" fn mappingproxy_or(__context: u64, self_bits: u64, other: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::molt_bit_or_attached_body(
        __attached_py,
        mappingproxy_unwrap(self_bits),
        mappingproxy_unwrap(other),
    )
}

extern "C" fn mappingproxy_ror(__context: u64, self_bits: u64, other: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::molt_bit_or_attached_body(
        __attached_py,
        mappingproxy_unwrap(other),
        mappingproxy_unwrap(self_bits),
    )
}

macro_rules! mappingproxy_comparison {
    ($name:ident, $operation:path) => {
        extern "C" fn $name(__context: u64, self_bits: u64, other: u64) -> u64 {
            let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

            $operation(
                __attached_py,
                mappingproxy_unwrap(self_bits),
                mappingproxy_unwrap(other),
            )
        }
    };
}
mappingproxy_comparison!(mappingproxy_eq, crate::molt_eq_attached_body);
mappingproxy_comparison!(mappingproxy_ne, crate::molt_ne_attached_body);
mappingproxy_comparison!(mappingproxy_lt, crate::molt_lt_attached_body);
mappingproxy_comparison!(mappingproxy_le, crate::molt_le_attached_body);
mappingproxy_comparison!(mappingproxy_gt, crate::molt_gt_attached_body);
mappingproxy_comparison!(mappingproxy_ge, crate::molt_ge_attached_body);

pub(crate) fn molt_types_capsule_new_attached_body(
    _py: &crate::PyToken<'_>,
    _cls_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "cannot create 'capsule' instances",
        )
    })
}

pub(crate) fn molt_types_cell_new_attached_body(
    _py: &crate::PyToken<'_>,
    _cls_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if !obj_from_bits(kwargs_bits).is_none() {
            let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "CellType takes no keyword arguments",
                );
            };
            if unsafe { object_type_id(kwargs_ptr) } != TYPE_ID_DICT
                || unsafe { crate::dict_len(kwargs_ptr) } != 0
            {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "CellType takes no keyword arguments",
                );
            }
        }
        let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "CellType expects arguments");
        };
        if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "CellType expects arguments");
        }
        let Some((len, value_bits)) = (unsafe {
            with_immutable_tuple_slice(args_ptr, |args| {
                (
                    args.len(),
                    args.first().copied().unwrap_or_else(|| missing_bits(_py)),
                )
            })
        }) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "CellType expects arguments");
        };
        if len > 1 {
            let msg = format!("CellType expected at most 1 argument, got {len}");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        }
        let ptr = crate::object::cells::alloc_cell(_py, value_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

pub(crate) fn molt_types_cell_contents_get_attached_body(
    _py: &crate::PyToken<'_>,
    _descriptor_bits: u64,
    cell_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(ptr) = crate::object::cells::cell_ptr_from_bits(cell_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected cell");
        };
        let value = unsafe { crate::object::cells::cell_value_bits(ptr) };
        if is_missing_bits(_py, value) {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "Cell is empty");
        }
        inc_ref_bits(_py, value);
        value
    })
}

pub(crate) fn molt_types_cell_contents_set_attached_body(
    _py: &crate::PyToken<'_>,
    _descriptor_bits: u64,
    cell_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(ptr) = crate::object::cells::cell_ptr_from_bits(cell_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected cell");
        };
        unsafe { crate::object::cells::cell_replace_value(_py, ptr, value_bits) };
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_types_cell_contents_delete_attached_body(
    _py: &crate::PyToken<'_>,
    _descriptor_bits: u64,
    cell_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(ptr) = crate::object::cells::cell_ptr_from_bits(cell_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected cell");
        };
        unsafe { crate::object::cells::cell_replace_value(_py, ptr, missing_bits(_py)) };
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_types_simplenamespace_init_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "SimpleNamespace requires a namespace receiver",
            );
        };
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let has_args = if let Some(args_ptr) = args_ptr {
            unsafe {
                if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "SimpleNamespace expects arguments",
                    );
                }
                with_immutable_tuple_slice(args_ptr, |args| !args.is_empty()).unwrap_or(false)
            }
        } else {
            false
        };
        if has_args {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "no positional arguments expected",
            );
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT
                    && !dict_update_apply(_py, dict_bits, dict_update_set_in_place, kwargs_bits)
                {
                    dec_ref_bits(_py, dict_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    dec_ref_bits(_py, dict_bits);
                    return MoltObject::none().bits();
                }
                let Some(order) = crate::object::ops_dict::dict_snapshot(
                    _py,
                    dict_ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                ) else {
                    dec_ref_bits(_py, dict_bits);
                    return MoltObject::none().bits();
                };
                let mut idx = 0;
                while idx + 1 < order.len() {
                    let key_bits = order[idx];
                    let val_bits = order[idx + 1];
                    let Some(key_ptr) = obj_from_bits(key_bits).as_ptr() else {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    };
                    if object_type_id(key_ptr) != TYPE_ID_STRING {
                        dec_ref_bits(_py, dict_bits);
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "keywords must be strings",
                        );
                    }
                    // SimpleNamespace.__init__ populates its dictionary
                    // directly, like CPython: neither subclass setters nor
                    // data descriptors participate in constructor state.
                    if crate::object::field_storage::set_item(_py, self_ptr, key_bits, val_bits)
                        .is_err()
                    {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    }
                    idx += 2;
                }
            }
        }
        dec_ref_bits(_py, dict_bits);
        MoltObject::none().bits()
    })
}

// CPython v3.13.9/v3.14.0 Objects/namespaceobject.c: namespace_replace.
// Call the actual subtype without arguments, then update the actual dictionaries;
// subclass __setattr__ and data descriptors do not participate.
pub(crate) fn molt_types_simplenamespace_replace_attached_body(
    _py: &crate::PyToken<'_>,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(call) = crate::builtins::native_arguments::NativeArguments::read(
            py,
            "__replace__",
            args_bits,
            kwargs_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let Some(self_bits) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        if !call.values().is_empty() {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "__replace__() takes no positional arguments",
            );
        }
        let class = type_of_bits(py, self_bits);
        let result = unsafe { call_callable0(py, class) };
        if exception_pending(py) {
            return result;
        }
        let Some(result_ptr) = obj_from_bits(result).as_ptr() else {
            return result;
        };
        let result_owner = crate::PtrDropGuard::new(py, result_ptr);
        let source_ptr = obj_from_bits(self_bits)
            .as_ptr()
            .expect("namespace descriptor receiver");
        for dictionary in [unsafe { instance_dict_bits(source_ptr) }, kwargs_bits] {
            let Some(ptr) = obj_from_bits(dictionary).as_ptr() else {
                continue;
            };
            let Some(entries) = (unsafe {
                crate::object::ops_dict::dict_snapshot(
                    py,
                    ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }) else {
                return MoltObject::none().bits();
            };
            for pair in entries.as_chunks::<2>().0 {
                if unsafe {
                    crate::object::field_storage::set_item(py, result_ptr, pair[0], pair[1])
                }
                .is_err()
                {
                    return MoltObject::none().bits();
                }
            }
        }
        inc_ref_bits(py, result);
        drop(result_owner);
        result
    })
}

pub(crate) fn molt_types_simplenamespace_repr_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mut out = String::from("namespace(");
        let dict_bits = unsafe { instance_dict_bits(self_ptr) };
        if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
            let dict_ptr = obj_from_bits(dict_bits).as_ptr();
            if let Some(dict_ptr) = dict_ptr {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let Some(order) = crate::object::ops_dict::dict_snapshot(
                            _py,
                            dict_ptr,
                            crate::object::ops_dict::DictSnapshotKind::Entries,
                        ) else {
                            return MoltObject::none().bits();
                        };
                        let mut idx = 0;
                        let mut first = true;
                        while idx + 1 < order.len() {
                            let key_bits = order[idx];
                            let val_bits = order[idx + 1];
                            let key_str = string_obj_to_owned(obj_from_bits(key_bits))
                                .unwrap_or_else(|| "<key>".to_string());
                            let val_repr_bits = molt_repr_from_obj_attached_body(_py, val_bits);
                            let val_repr = string_obj_to_owned(obj_from_bits(val_repr_bits))
                                .unwrap_or_default();
                            dec_ref_bits(_py, val_repr_bits);
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            out.push_str(&key_str);
                            out.push('=');
                            out.push_str(&val_repr);
                            idx += 2;
                        }
                    }
                }
            }
        }
        out.push(')');
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

pub(crate) fn molt_types_simplenamespace_eq_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let other_ptr = obj_from_bits(other_bits).as_ptr();
        let Some(other_ptr) = other_ptr else {
            return crate::builtins::methods::not_implemented_bits(_py);
        };
        let self_class = unsafe { object_class_bits(self_ptr) };
        let other_class = unsafe { object_class_bits(other_ptr) };
        if self_class == 0 || other_class == 0 || self_class != other_class {
            return crate::builtins::methods::not_implemented_bits(_py);
        }
        let self_dict_bits = unsafe { instance_dict_bits(self_ptr) };
        let other_dict_bits = unsafe { instance_dict_bits(other_ptr) };
        if self_dict_bits == 0 && other_dict_bits == 0 {
            return MoltObject::from_bool(true).bits();
        }
        let mut created = Vec::new();
        let left_bits = if self_dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(dict_ptr).bits();
            created.push(bits);
            bits
        } else {
            self_dict_bits
        };
        let right_bits = if other_dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                for bits in created.iter() {
                    dec_ref_bits(_py, *bits);
                }
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(dict_ptr).bits();
            created.push(bits);
            bits
        } else {
            other_dict_bits
        };
        let eq_bits = molt_eq_attached_body(_py, left_bits, right_bits);
        for bits in created.iter() {
            dec_ref_bits(_py, *bits);
        }
        eq_bits
    })
}

pub(crate) unsafe fn types_visit_owned_edges(
    shape: crate::object::ObjectShapeId,
    ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    match shape {
        crate::object::ObjectShapeId::TypesMappingProxy => {
            visit(unsafe { mappingproxy_mapping_bits(ptr) });
        }
        crate::object::ObjectShapeId::TypesFrame => unsafe {
            crate::builtins::frames::frame_object_visit(ptr, visit);
        },
        crate::object::ObjectShapeId::TypesFrameLocalsProxy => unsafe {
            crate::builtins::frames::frame_locals_proxy_visit(ptr, visit);
        },
        _ => unreachable!("non-types object shape"),
    }
}

pub(crate) unsafe fn types_detach_owned_edges(
    shape: crate::object::ObjectShapeId,
    ptr: *mut u8,
    mut detach: impl FnMut(u64),
) {
    match shape {
        crate::object::ObjectShapeId::TypesMappingProxy => unsafe {
            let old = mappingproxy_mapping_bits(ptr);
            mappingproxy_set_mapping_bits(ptr, MoltObject::none().bits());
            detach(old);
        },
        crate::object::ObjectShapeId::TypesFrame => unsafe {
            crate::builtins::frames::frame_object_detach(ptr, detach);
        },
        crate::object::ObjectShapeId::TypesFrameLocalsProxy => unsafe {
            crate::builtins::frames::frame_locals_proxy_detach(ptr, detach);
        },
        _ => unreachable!("non-types object shape"),
    }
}

include!("../../attached_exports_generated/builtins/types/concrete_types.rs");
