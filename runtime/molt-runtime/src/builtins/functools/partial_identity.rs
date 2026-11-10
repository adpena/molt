//! Physical partial construction and state, following CPython's partial type.
use super::*;
use crate::builtins::functions::native_callable::NativeCallableSpec;
use crate::builtins::types::{NativeDescriptorFlavor, NativeDescriptorSpec};
use molt_runtime_core::ProtectedRuntimeValue as Value;

#[derive(Clone, Copy)]
#[repr(u32)]
enum PartialMember {
    Function,
    Arguments,
    Keywords,
    Dictionary,
}

impl PartialMember {
    fn from_operation(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Function),
            1 => Some(Self::Arguments),
            2 => Some(Self::Keywords),
            3 => Some(Self::Dictionary),
            _ => None,
        }
    }
}

fn partial_pointer(py: &PyToken<'_>, bits: u64) -> Option<*mut u8> {
    let pointer = obj_from_bits(bits).as_ptr().filter(|pointer| {
        crate::object::object_shape_id(*pointer) == crate::object::ObjectShapeId::FunctoolsPartial
    });
    if pointer.is_none() {
        raise_exception::<u64>(
            py,
            BuiltinExc::TypeError,
            "expected a functools.partial object",
        );
    }
    pointer
}

fn tuple<'a>(py: &'a PyToken<'_>, items: &[u64]) -> Option<Value<'a>> {
    let pointer = alloc_tuple(py, items);
    if pointer.is_null() {
        None
    } else {
        Some(unsafe {
            Value::from_owned_bits(py.core_token(), MoltObject::from_ptr(pointer).bits())
        })
    }
}

pub(super) fn publish_members(py: &PyToken<'_>, class: u64, namespace: *mut u8) -> bool {
    let getter = crate::builtins::methods::alloc_builtin_function(
        py,
        molt_functools_partial_member_get as *const () as usize as u64,
        2,
    );
    if getter == 0 {
        return false;
    }
    let _getter = unsafe { Value::from_owned_bits(py.core_token(), getter) };
    for (member, name) in [
        (PartialMember::Function, crate::NameId::Func),
        (PartialMember::Arguments, crate::NameId::Args),
        (PartialMember::Keywords, crate::NameId::Keywords),
    ] {
        let Some(key) = name.owned_bits(py) else {
            return false;
        };
        let key = unsafe { Value::from_owned_bits(py.core_token(), key) };
        let descriptor = crate::builtins::types::alloc_native_descriptor(
            py,
            NativeDescriptorSpec {
                flavor: NativeDescriptorFlavor::Member,
                operation: member as u32,
                owner: class,
                name: key.bits(),
                doc: MoltObject::none().bits(),
                getter,
                setter: MoltObject::none().bits(),
                deleter: MoltObject::none().bits(),
            },
        );
        if exception_pending(py) {
            dec_ref_bits(py, descriptor);
            return false;
        }
        let descriptor = unsafe { Value::from_owned_bits(py.core_token(), descriptor) };
        if !unsafe { dict_set_in_place(py, namespace, key.bits(), descriptor.bits()) } {
            return false;
        }
    }
    let dictionary_setter = crate::builtins::methods::alloc_builtin_function(
        py,
        molt_functools_partial_dictionary_set as *const () as usize as u64,
        3,
    );
    if dictionary_setter == 0 {
        return false;
    }
    let dictionary_setter = unsafe { Value::from_owned_bits(py.core_token(), dictionary_setter) };
    let dictionary_deleter = crate::builtins::methods::alloc_builtin_function(
        py,
        molt_functools_partial_dictionary_delete as *const () as usize as u64,
        2,
    );
    if dictionary_deleter == 0 {
        return false;
    }
    let dictionary_deleter = unsafe { Value::from_owned_bits(py.core_token(), dictionary_deleter) };
    let Some(name) = crate::NameId::DunderDict.owned_bits(py) else {
        return false;
    };
    let name = unsafe { Value::from_owned_bits(py.core_token(), name) };
    let descriptor = crate::builtins::types::alloc_native_descriptor(
        py,
        NativeDescriptorSpec {
            flavor: NativeDescriptorFlavor::GetSet,
            operation: PartialMember::Dictionary as u32,
            owner: class,
            name: name.bits(),
            doc: MoltObject::none().bits(),
            getter,
            setter: dictionary_setter.bits(),
            deleter: dictionary_deleter.bits(),
        },
    );
    let descriptor = unsafe { Value::from_owned_bits(py.core_token(), descriptor) };
    if exception_pending(py)
        || !unsafe { dict_set_in_place(py, namespace, name.bits(), descriptor.bits()) }
    {
        return false;
    }
    if crate::object::ops_sys::runtime_target_at_least(py, 3, 13)
        && crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
            py,
            NativeCallableSpec::declared(
                NativeCallableKind::WrapperDescriptor,
                class,
                crate::NameId::DunderGet,
            ),
            molt_functools_partial_get as *const () as usize as u64,
            3,
            &[MoltObject::none().bits()],
        ) == 0
    {
        return false;
    }
    true
}

pub(crate) fn molt_functools_partial_member_get_attached_body(
    _py: &crate::PyToken<'_>,
    descriptor: u64,
    instance: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(pointer) = partial_pointer(py, instance) else {
            return MoltObject::none().bits();
        };
        let operation = obj_from_bits(descriptor)
            .as_ptr()
            .and_then(|pointer| unsafe {
                crate::object::layout::native_descriptor_operation(pointer)
            });
        let value = match operation.and_then(PartialMember::from_operation) {
            Some(PartialMember::Function) => unsafe { partial_func_bits(pointer) },
            Some(PartialMember::Arguments) => unsafe { partial_args_bits(pointer) },
            Some(PartialMember::Keywords) => unsafe { partial_kwargs_bits(pointer) },
            Some(PartialMember::Dictionary) => {
                let dictionary = unsafe { crate::instance_dict_bits(pointer) };
                if dictionary != 0 {
                    dictionary
                } else {
                    let dictionary = crate::alloc_dict_with_pairs(py, &[]);
                    if dictionary.is_null() {
                        return MoltObject::none().bits();
                    }
                    let bits = MoltObject::from_ptr(dictionary).bits();
                    unsafe {
                        crate::object::instance_set_dict_bits(py, pointer, bits);
                    }
                    bits
                }
            }
            _ => {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::SystemError,
                    "invalid partial member declaration",
                );
            }
        };
        inc_ref_bits(py, value);
        value
    })
}

// This is the physical PyObject_GenericSetDict boundary of partial, which
// also permits recovery from the admitted 3.12 __setstate__ representation.

pub(crate) fn molt_functools_partial_dictionary_set_attached_body(
    _py: &crate::PyToken<'_>,
    _descriptor: u64,
    instance: u64,
    value: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(pointer) = partial_pointer(py, instance) else {
            return MoltObject::none().bits();
        };
        if !obj_from_bits(value)
            .as_ptr()
            .is_some_and(|p| unsafe { object_type_id(p) } == TYPE_ID_DICT)
        {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "__dict__ must be set to a dictionary",
            );
        }
        inc_ref_bits(py, value);
        let old = unsafe { crate::instance_dict_bits(pointer) };
        unsafe {
            crate::object::instance_set_dict_bits(py, pointer, value);
        }
        if old != 0 {
            dec_ref_bits(py, old);
        }
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_functools_partial_dictionary_delete_attached_body(
    _py: &crate::PyToken<'_>,
    _descriptor: u64,
    instance: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if partial_pointer(py, instance).is_none() {
            return MoltObject::none().bits();
        }
        raise_exception::<u64>(py, BuiltinExc::TypeError, "cannot delete __dict__")
    })
}

pub(crate) fn molt_functools_partial_new_attached_body(
    _py: &crate::PyToken<'_>,
    class: u64,
    arguments: u64,
    keywords: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(arguments_ptr) = obj_from_bits(arguments).as_ptr() else {
            return MoltObject::none().bits();
        };
        let Some(arguments) = (unsafe {
            crate::object::seq_access::snapshot(py, arguments_ptr, "partial arguments")
        }) else {
            return MoltObject::none().bits();
        };
        let Some(&function) = arguments.first() else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "type 'partial' takes at least one argument",
            );
        };
        if !crate::builtins::callable::is_callable_impl(py, function) {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "the first argument must be callable",
            );
        }
        let mut function = Value::retain(py.core_token(), function);
        let mut positional = arguments[1..].to_vec();
        let mut inherited_keywords = None;
        if let Some(pointer) = obj_from_bits(function.bits())
            .as_ptr()
            .filter(|pointer| unsafe {
                crate::object::object_shape_id(*pointer)
                    == crate::object::ObjectShapeId::FunctoolsPartial
                    && crate::instance_dict_bits(*pointer) == 0
            })
        {
            // A subclass overriding __call__ must remain the selected callable.
            let Some(key) = crate::NameId::DunderCall.owned_bits(py) else {
                return MoltObject::none().bits();
            };
            let key = unsafe { Value::from_owned_bits(py.core_token(), key) };
            let owner = unsafe { object_class_bits(pointer) };
            let method = unsafe {
                crate::class_attr_lookup_raw_mro(
                    py,
                    obj_from_bits(owner).as_ptr().unwrap(),
                    key.bits(),
                )
            };
            let method = match method {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            let native_call = method
                .and_then(|bits| obj_from_bits(bits).as_ptr())
                .is_some_and(|method| unsafe {
                    object_type_id(method) == crate::TYPE_ID_FUNCTION
                        && crate::function_fn_ptr(method)
                            == crate::molt_functools_partial_call as *const () as usize as u64
                });
            if native_call || crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                let stored = Value::retain(py.core_token(), unsafe { partial_args_bits(pointer) });
                let stored_ptr = obj_from_bits(stored.bits()).as_ptr().unwrap();
                let Some(stored) = (unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        stored_ptr,
                        "partial flattening arguments",
                    )
                }) else {
                    return MoltObject::none().bits();
                };
                let mut combined = stored.to_vec();
                combined.extend_from_slice(&positional);
                positional = combined;
                inherited_keywords = Some(Value::retain(py.core_token(), unsafe {
                    partial_kwargs_bits(pointer)
                }));
                function = Value::retain(py.core_token(), unsafe { partial_func_bits(pointer) });
            }
        }
        let Some(positional) = tuple(py, &positional) else {
            return MoltObject::none().bits();
        };
        let dictionary = if let Some(stored) = inherited_keywords {
            let copy = crate::dict_copy_method(_py.attached_context(), stored.bits()) as u64;
            let copy = unsafe { Value::from_owned_bits(py.core_token(), copy) };
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            if !obj_from_bits(keywords).is_none()
                && !unsafe {
                    dict_update_apply(py, copy.bits(), dict_update_set_in_place, keywords)
                }
            {
                return MoltObject::none().bits();
            }
            copy
        } else {
            let copy = if obj_from_bits(keywords).is_none() {
                let pointer = crate::alloc_dict_with_pairs(py, &[]);
                if pointer.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(pointer).bits()
            } else {
                crate::dict_copy_method(_py.attached_context(), keywords) as u64
            };
            if exception_pending(py) {
                dec_ref_bits(py, copy);
                return MoltObject::none().bits();
            }
            unsafe { Value::from_owned_bits(py.core_token(), copy) }
        };
        let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "partial.__new__ requires a type",
            );
        };
        match issubclass_runtime(py, class, partial_class(py)) {
            Some(true) => (),
            Some(false) => {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "partial.__new__ requires a partial subtype",
                );
            }
            None => return MoltObject::none().bits(),
        }
        let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        let Some(pointer) = obj_from_bits(instance).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            partial_set_func_bits(pointer, function.into_bits());
            partial_set_args_bits(pointer, positional.into_bits());
            partial_set_kwargs_bits(pointer, dictionary.into_bits());
        }
        instance
    })
}

pub(crate) fn molt_functools_partial_reduce_attached_body(
    _py: &crate::PyToken<'_>,
    instance: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(pointer) = partial_pointer(py, instance) else {
            return MoltObject::none().bits();
        };
        let class = Value::retain(py.core_token(), unsafe { object_class_bits(pointer) });
        let function = Value::retain(py.core_token(), unsafe { partial_func_bits(pointer) });
        let args = Value::retain(py.core_token(), unsafe { partial_args_bits(pointer) });
        let keywords = Value::retain(py.core_token(), unsafe { partial_kwargs_bits(pointer) });
        let dictionary = unsafe { crate::instance_dict_bits(pointer) };
        let dictionary = Value::retain(
            py.core_token(),
            if dictionary == 0 {
                MoltObject::none().bits()
            } else {
                dictionary
            },
        );
        let Some(arguments) = tuple(py, &[function.bits()]) else {
            return MoltObject::none().bits();
        };
        let Some(state) = tuple(
            py,
            &[
                function.bits(),
                args.bits(),
                keywords.bits(),
                dictionary.bits(),
            ],
        ) else {
            return MoltObject::none().bits();
        };
        tuple(py, &[class.bits(), arguments.bits(), state.bits()])
            .map_or(MoltObject::none().bits(), Value::into_bits)
    })
}

pub(crate) fn molt_functools_partial_setstate_attached_body(
    _py: &crate::PyToken<'_>,
    instance: u64,
    state: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(pointer) = partial_pointer(py, instance) else {
            return MoltObject::none().bits();
        };
        let state_ptr = obj_from_bits(state)
            .as_ptr()
            .filter(|pointer| unsafe { object_type_id(*pointer) } == TYPE_ID_TUPLE);
        let Some(state_ptr) = state_ptr else {
            return raise_exception::<u64>(py, BuiltinExc::TypeError, "invalid partial state");
        };
        let Some(state) =
            (unsafe { crate::object::seq_access::snapshot(py, state_ptr, "partial state") })
        else {
            return MoltObject::none().bits();
        };
        if state.len() != 4
            || !crate::builtins::callable::is_callable_impl(py, state[0])
            || !obj_from_bits(state[1])
                .as_ptr()
                .is_some_and(|p| unsafe { object_type_id(p) } == TYPE_ID_TUPLE)
            || (!obj_from_bits(state[2]).is_none()
                && crate::isinstance_runtime(py, state[2], builtin_classes(py).dict) != Some(true))
            || (crate::object::ops_sys::runtime_target_at_least(py, 3, 13)
                && !obj_from_bits(state[3]).is_none()
                && !obj_from_bits(state[3])
                    .as_ptr()
                    .is_some_and(|p| unsafe { object_type_id(p) } == TYPE_ID_DICT))
        {
            return raise_exception::<u64>(py, BuiltinExc::TypeError, "invalid partial state");
        }
        let function = Value::retain(py.core_token(), state[0]);
        let args = if type_of_bits(py, state[1]) == builtin_classes(py).tuple {
            Value::retain(py.core_token(), state[1])
        } else {
            let Some(bits) = (unsafe { crate::object::ops::tuple_from_iter_bits(py, state[1]) })
            else {
                return MoltObject::none().bits();
            };
            unsafe { Value::from_owned_bits(py.core_token(), bits) }
        };
        let keywords = if obj_from_bits(state[2]).is_none() {
            let p = crate::alloc_dict_with_pairs(py, &[]);
            if p.is_null() {
                return MoltObject::none().bits();
            }
            unsafe { Value::from_owned_bits(py.core_token(), MoltObject::from_ptr(p).bits()) }
        } else if type_of_bits(py, state[2]) == builtin_classes(py).dict {
            Value::retain(py.core_token(), state[2])
        } else {
            let copy = crate::dict_copy_method(_py.attached_context(), state[2]) as u64;
            if exception_pending(py) {
                dec_ref_bits(py, copy);
                return MoltObject::none().bits();
            }
            unsafe { Value::from_owned_bits(py.core_token(), copy) }
        };
        let dictionary =
            (!obj_from_bits(state[3]).is_none()).then(|| Value::retain(py.core_token(), state[3]));
        // CPython commits and retires each native field in order. A finalizer
        // can reenter after any individual store; subsequent displaced owners
        // must therefore be read at their own transition, not snapshotted here.
        let old = unsafe { partial_func_bits(pointer) };
        unsafe {
            partial_set_func_bits(pointer, function.into_bits());
        }
        dec_ref_bits(py, old);
        let old = unsafe { partial_args_bits(pointer) };
        unsafe {
            partial_set_args_bits(pointer, args.into_bits());
        }
        dec_ref_bits(py, old);
        let old = unsafe { partial_kwargs_bits(pointer) };
        unsafe {
            partial_set_kwargs_bits(pointer, keywords.into_bits());
        }
        dec_ref_bits(py, old);
        let old = unsafe { crate::instance_dict_bits(pointer) };
        // The physical partial protocol owns this slot. The admitted 3.12
        // source accepts any object here; ordinary attribute access diagnoses
        // a non-dictionary. The admitted 3.13+ sources reject that state.
        unsafe {
            crate::object::instance_set_dict_bits(
                py,
                pointer,
                dictionary.map_or(0, Value::into_bits),
            );
        }
        if old != 0 {
            dec_ref_bits(py, old);
        }
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_functools_partial_get_attached_body(
    _py: &crate::PyToken<'_>,
    instance: u64,
    receiver: u64,
    _owner: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if partial_pointer(py, instance).is_none() {
            return MoltObject::none().bits();
        }
        if obj_from_bits(receiver).is_none() {
            inc_ref_bits(py, instance);
            return instance;
        }
        if !crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
            let message = alloc_string(py, b"functools.partial will be a method descriptor in future Python versions; wrap it in staticmethod() if you want to preserve the old behavior");
            if message.is_null() {
                return MoltObject::none().bits();
            }
            let message = unsafe {
                Value::from_owned_bits(py.core_token(), MoltObject::from_ptr(message).bits())
            };
            let category =
                crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::FutureWarning);
            let result = crate::molt_warnings_warn_attached_body(
                py,
                message.bits(),
                category,
                MoltObject::from_int(1).bits(),
            );
            dec_ref_bits(py, result);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(py, instance);
            return instance;
        }
        crate::builtins::functions::explicit_bound_method_new(py, instance, receiver)
    })
}

fn append_representation(py: &PyToken<'_>, value: u64, output: &mut Vec<u8>, repr: bool) -> bool {
    let rendered = if repr {
        crate::molt_repr_from_obj_attached_body(py, value)
    } else {
        crate::molt_str_from_obj_attached_body(py, value)
    };
    let rendered = unsafe { Value::from_owned_bits(py.core_token(), rendered) };
    if exception_pending(py) {
        return false;
    }
    if unsafe {
        crate::object::ops_format::with_string_bytes(obj_from_bits(rendered.bits()), |bytes| {
            output.extend_from_slice(bytes)
        })
    }
    .is_none()
    {
        raise_exception::<u64>(
            py,
            BuiltinExc::SystemError,
            "partial representation expected a string",
        );
        return false;
    }
    true
}

pub(crate) fn molt_functools_partial_repr_attached_body(
    _py: &crate::PyToken<'_>,
    instance: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(pointer) = partial_pointer(py, instance) else {
            return MoltObject::none().bits();
        };
        let guard = crate::object::ops_format::ReprGuard::new(py, pointer);
        if !guard.active() {
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let text = alloc_string(py, b"...");
            return if text.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(text).bits()
            };
        }
        let function = Value::retain(py.core_token(), unsafe { partial_func_bits(pointer) });
        let args = Value::retain(py.core_token(), unsafe { partial_args_bits(pointer) });
        let keywords = Value::retain(py.core_token(), unsafe { partial_kwargs_bits(pointer) });
        let args_pointer = obj_from_bits(args.bits()).as_ptr().unwrap();
        let Some(args) = (unsafe {
            crate::object::seq_access::snapshot(py, args_pointer, "partial repr arguments")
        }) else {
            return MoltObject::none().bits();
        };
        let keyword_pointer = obj_from_bits(keywords.bits()).as_ptr().unwrap();
        let Some(keywords) = (unsafe {
            crate::object::ops_dict::dict_snapshot(
                py,
                keyword_pointer,
                crate::object::ops_dict::DictSnapshotKind::Entries,
            )
        }) else {
            return MoltObject::none().bits();
        };
        let mut suffix = Vec::new();
        for &argument in args.iter() {
            suffix.extend_from_slice(b", ");
            if !append_representation(py, argument, &mut suffix, true) {
                return MoltObject::none().bits();
            }
        }
        for pair in keywords.as_chunks::<2>().0 {
            suffix.extend_from_slice(b", ");
            if !append_representation(py, pair[0], &mut suffix, false) {
                return MoltObject::none().bits();
            }
            suffix.push(b'=');
            if !append_representation(py, pair[1], &mut suffix, true) {
                return MoltObject::none().bits();
            }
        }
        let class = unsafe { object_class_bits(pointer) };
        let mut output = Vec::new();
        if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
            let class = obj_from_bits(class).as_ptr().unwrap();
            let module = match unsafe { crate::object::class_storage::class_module_bits(py, class) }
            {
                Ok(Some(bits)) => Value::retain(py.core_token(), bits),
                Ok(None) => {
                    return raise_exception::<u64>(py, BuiltinExc::AttributeError, "__module__");
                }
                Err(()) => return MoltObject::none().bits(),
            };
            let qualified = Value::retain(py.core_token(), unsafe {
                crate::object::layout::class_qualname_bits(class)
            });
            if !append_representation(py, module.bits(), &mut output, false) {
                return MoltObject::none().bits();
            }
            output.push(b'.');
            if !append_representation(py, qualified.bits(), &mut output, false) {
                return MoltObject::none().bits();
            }
        } else if class == partial_class(py) {
            let identity =
                crate::intrinsics::public_objects_generated::PublicObjectId::FunctoolsPartialType
                    .identity();
            output.extend_from_slice(identity.module.as_str().as_bytes());
            output.push(b'.');
            output.extend_from_slice(identity.qualname.as_str().as_bytes());
        } else {
            let name = unsafe { crate::class_name_bits(obj_from_bits(class).as_ptr().unwrap()) };
            if unsafe {
                crate::object::ops_format::with_string_bytes(obj_from_bits(name), |bytes| {
                    output.extend_from_slice(bytes)
                })
            }
            .is_none()
            {
                return MoltObject::none().bits();
            }
        }
        output.push(b'(');
        let mut function_repr = Vec::new();
        if !append_representation(py, function.bits(), &mut function_repr, true) {
            return MoltObject::none().bits();
        }
        output.extend_from_slice(&function_repr);
        output.extend_from_slice(&suffix);
        output.push(b')');
        let result = alloc_string(py, &output);
        if result.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result).bits()
        }
    })
}

include!("../../attached_exports_generated/builtins/functools/partial_identity.rs");
