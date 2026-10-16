//! Declared callable members/getsets over existing private function/binding storage.
use super::*;
use crate::builtins::functions::native_callable::CallableMetadata;
use crate::object::function_metadata::FunctionMetadataField as Field;

pub(crate) unsafe fn read(
    py: &PyToken<'_>,
    public: *mut u8,
    declared: CallableMetadata,
) -> Result<Option<u64>, ()> {
    unsafe {
        let kind = NativeCallableKind::from_class(py, object_class_bits(public));
        let field = match declared {
            CallableMetadata::Stored(field) => field,
            CallableMetadata::Builtins => {
                if kind.is_some() || object_type_id(public) != TYPE_ID_FUNCTION {
                    return Ok(None);
                }
                let value = crate::object::layout::function_builtins_bits(public);
                let value = if value == 0 {
                    MoltObject::none().bits()
                } else {
                    value
                };
                inc_ref_bits(py, value);
                return Ok(Some(value));
            }
        };
        if kind.is_none() && object_type_id(public) == TYPE_ID_FUNCTION {
            let value = field.load(public).unwrap_or(MoltObject::none().bits());
            inc_ref_bits(py, value);
            return Ok(Some(value));
        }
        let Some(kind) = kind else {
            return Ok(None);
        };
        let object = if object_type_id(public) == TYPE_ID_BOUND_METHOD {
            let Some(object) = obj_from_bits(bound_method_func_bits(public)).as_ptr() else {
                return Ok(None);
            };
            object
        } else {
            public
        };
        if field == Field::SelfValue && object_type_id(public) == TYPE_ID_BOUND_METHOD {
            let value = bound_method_self_bits(public);
            inc_ref_bits(py, value);
            return Ok(Some(value));
        }
        if field == Field::Module && kind.has_module() && public != object {
            let value = crate::object::layout::bound_method_module_bits(public);
            inc_ref_bits(py, value);
            return Ok(Some(value));
        }
        if field == Field::SelfValue
            && public == object
            && let Some(value) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .cfunction_self(MoltObject::from_ptr(object).bits())
        {
            return value.map(Some).map_err(|_| {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native callable __self__");
            });
        }
        if (field == Field::Doc || field == Field::TextSignature)
            && public == object
            && let Some(documentation) = molt_cpython_abi::bridge::GLOBAL_BRIDGE.cfunction_metadata(
                MoltObject::from_ptr(object).bits(),
                field == Field::TextSignature,
            )
        {
            return match documentation {
                None => Ok(Some(MoltObject::none().bits())),
                Some(text) => {
                    let pointer = alloc_string(py, &text);
                    if pointer.is_null() {
                        Err(())
                    } else {
                        Ok(Some(MoltObject::from_ptr(pointer).bits()))
                    }
                }
            };
        }
        if field == Field::Module
            && kind.has_module()
            && public == object
            && let Some(result) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .cfunction_module(MoltObject::from_ptr(object).bits())
        {
            return result.map(Some).map_err(|_| {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native callable __module__",
                );
            });
        }
        let value =
            crate::call::function::function_metadata_bits(py, object, field.name().as_bytes());
        if obj_from_bits(value).is_none()
            && matches!(field, Field::Name | Field::QualName | Field::Owner)
        {
            raise_exception::<()>(py, "AttributeError", field.name());
            return Err(());
        }
        inc_ref_bits(py, value);
        Ok(Some(value))
    }
}

pub(crate) unsafe fn write(
    py: &PyToken<'_>,
    object: *mut u8,
    declared: CallableMetadata,
    value: Option<u64>,
) -> Result<(), ()> {
    unsafe {
        let CallableMetadata::Stored(field) = declared else {
            raise_exception::<()>(py, "AttributeError", "readonly attribute");
            return Err(());
        };
        if object_type_id(object) == TYPE_ID_FUNCTION
            && NativeCallableKind::from_class(py, object_class_bits(object)).is_none()
        {
            return crate::object::function_metadata::write_public(py, object, field, value);
        }
        if field != Field::Module {
            raise_exception::<()>(py, "AttributeError", "readonly attribute");
            return Err(());
        }
        if object_type_id(object) == TYPE_ID_BOUND_METHOD {
            crate::object::layout::bound_method_set_module_bits(
                py,
                object,
                value.unwrap_or(MoltObject::none().bits()),
            );
        } else if let Some(ok) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .set_cfunction_module(MoltObject::from_ptr(object).bits(), value)
        {
            if !ok {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native callable __module__ assignment",
                );
                return Err(());
            }
        } else {
            let value = value.unwrap_or(MoltObject::none().bits());
            if !crate::call::class_init::function_set_attr_name(py, object, b"__module__", value) {
                return Err(());
            }
        }
        Ok(())
    }
}

pub(crate) unsafe fn publish(py: &PyToken<'_>) -> bool {
    unsafe {
        for kind in [
            NativeCallableKind::Function,
            NativeCallableKind::CMethod,
            NativeCallableKind::MethodDescriptor,
            NativeCallableKind::WrapperDescriptor,
            NativeCallableKind::ClassMethodDescriptor,
            NativeCallableKind::MethodWrapper,
        ] {
            if !publish_fields(py, kind.class(py), kind.metadata_fields()) {
                return false;
            }
        }
        if !publish_fields(
            py,
            builtin_classes(py).function,
            &CallableMetadata::PUBLIC_FUNCTION,
        ) {
            return false;
        }
        true
    }
}

unsafe fn publish_fields(py: &PyToken<'_>, owner: u64, fields: &[CallableMetadata]) -> bool {
    unsafe {
        let class = obj_from_bits(owner).as_ptr().unwrap();
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        let kind = NativeCallableKind::from_class(py, owner);
        for field in fields {
            let Some(name) = attr_name_bits_from_bytes(py, field.name().as_bytes()) else {
                return false;
            };
            // CPython's methodobject/descrobject/funcobject declaration kind
            // is independent of this runtime's shared typed metadata storage.
            let member = match (field, kind) {
                (CallableMetadata::Builtins, _) => true,
                (CallableMetadata::Stored(field), kind) => match kind {
                    Some(
                        NativeCallableKind::Constructor
                        | NativeCallableKind::Function
                        | NativeCallableKind::CMethod,
                    ) => *field == Field::Module,
                    Some(
                        NativeCallableKind::MethodDescriptor
                        | NativeCallableKind::WrapperDescriptor
                        | NativeCallableKind::ClassMethodDescriptor,
                    ) => matches!(field, Field::Name | Field::Owner),
                    Some(NativeCallableKind::MethodWrapper) => *field == Field::SelfValue,
                    None => matches!(field, Field::Doc | Field::Module),
                },
            };
            let descriptor = crate::builtins::types::alloc_native_descriptor(
                py,
                crate::builtins::types::NativeDescriptorSpec {
                    flavor: if member {
                        crate::object::layout::NativeDescriptorFlavor::CallableMember
                    } else {
                        crate::object::layout::NativeDescriptorFlavor::CallableGetSet
                    },
                    operation: field.operation(),
                    owner,
                    name,
                    doc: MoltObject::none().bits(),
                    getter: MoltObject::none().bits(),
                    setter: MoltObject::none().bits(),
                    deleter: MoltObject::none().bits(),
                },
            );
            let installed =
                !exception_pending(py) && dict_set_in_place(py, dictionary, name, descriptor);
            dec_ref_bits(py, descriptor);
            dec_ref_bits(py, name);
            if !installed || exception_pending(py) {
                return false;
            }
        }
        class_bump_layout_version(class);
        true
    }
}
