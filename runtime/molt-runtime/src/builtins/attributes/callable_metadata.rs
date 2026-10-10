//! Declared callable getsets over existing private function/binding storage.
use super::*;
use crate::builtins::functions::native_callable::CallableMetadata as Field;

pub(crate) unsafe fn read(_py: &PyToken<'_>, public_ptr: *mut u8, field: Field) -> Option<u64> {
    unsafe {
        let kind = NativeCallableKind::from_class(_py, object_class_bits(public_ptr));
        if kind.is_none() && object_type_id(public_ptr) == TYPE_ID_FUNCTION {
            let value = field.load(public_ptr).unwrap_or(MoltObject::none().bits());
            inc_ref_bits(_py, value);
            return Some(value);
        }
        let kind = kind?;
        let obj_ptr = if object_type_id(public_ptr) == TYPE_ID_BOUND_METHOD {
            obj_from_bits(bound_method_func_bits(public_ptr)).as_ptr()?
        } else {
            public_ptr
        };
        if field == Field::SelfValue && object_type_id(public_ptr) == TYPE_ID_BOUND_METHOD {
            let value = bound_method_self_bits(public_ptr);
            inc_ref_bits(_py, value);
            return Some(value);
        }
        if field == Field::Module && kind.has_module() && public_ptr != obj_ptr {
            let value = crate::object::layout::bound_method_module_bits(public_ptr);
            inc_ref_bits(_py, value);
            return Some(value);
        }
        if field == Field::SelfValue
            && public_ptr == obj_ptr
            && let Some(value) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .cfunction_self(MoltObject::from_ptr(obj_ptr).bits())
        {
            return match value {
                Ok(bits) => Some(bits),
                Err(molt_cpython_abi::ErrorIndicatorSet) => {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        _py,
                        "native callable __self__",
                    );
                    None
                }
            };
        }
        if (field == Field::Doc || field == Field::TextSignature)
            && public_ptr == obj_ptr
            && let Some(documentation) = molt_cpython_abi::bridge::GLOBAL_BRIDGE.cfunction_metadata(
                MoltObject::from_ptr(obj_ptr).bits(),
                field == Field::TextSignature,
            )
        {
            return documentation.map_or_else(
                || Some(MoltObject::none().bits()),
                |text| {
                    let ptr = alloc_string(_py, &text);
                    (!ptr.is_null()).then(|| MoltObject::from_ptr(ptr).bits())
                },
            );
        }
        if field == Field::Module
            && kind.has_module()
            && public_ptr == obj_ptr
            && let Some(result) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .cfunction_module(MoltObject::from_ptr(obj_ptr).bits())
        {
            return match result {
                Ok(bits) => Some(bits),
                Err(molt_cpython_abi::ErrorIndicatorSet) => {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        _py,
                        "native callable __module__",
                    );
                    None
                }
            };
        }
        let value =
            crate::call::function::function_metadata_bits(_py, obj_ptr, field.name().as_bytes());
        if obj_from_bits(value).is_none()
            && matches!(field, Field::Name | Field::QualName | Field::Owner)
        {
            raise_exception::<()>(_py, "AttributeError", field.name());
            return None;
        }
        inc_ref_bits(_py, value);
        Some(value)
    }
}

pub(crate) unsafe fn write(
    py: &PyToken<'_>,
    object: *mut u8,
    field: Field,
    value: Option<u64>,
) -> u64 {
    unsafe {
        if object_type_id(object) == TYPE_ID_FUNCTION
            && NativeCallableKind::from_class(py, object_class_bits(object)).is_none()
        {
            return crate::object::function_metadata::write_public(py, object, field, value);
        }
        if field != Field::Module {
            return raise_exception::<_>(py, "AttributeError", "readonly attribute");
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
            }
        } else {
            let value = value.unwrap_or(MoltObject::none().bits());
            if !crate::call::class_init::function_set_attr_name(py, object, b"__module__", value) {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
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
        if !publish_fields(py, builtin_classes(py).function, &Field::PUBLIC_FUNCTION) {
            return false;
        }
        true
    }
}

unsafe fn publish_fields(py: &PyToken<'_>, owner: u64, fields: &[Field]) -> bool {
    unsafe {
        let class = obj_from_bits(owner).as_ptr().unwrap();
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        for field in fields {
            let Some(name) = attr_name_bits_from_bytes(py, field.name().as_bytes()) else {
                return false;
            };
            let descriptor = crate::builtins::types::alloc_native_descriptor(
                py,
                crate::builtins::types::NativeDescriptorSpec {
                    flavor: crate::object::layout::NativeDescriptorFlavor::CallableMetadata,
                    operation: *field as u32,
                    owner,
                    name,
                    doc: MoltObject::none().bits(),
                    getter: MoltObject::none().bits(),
                    setter: MoltObject::none().bits(),
                    deleter: MoltObject::none().bits(),
                },
            );
            if !exception_pending(py) {
                dict_set_in_place(py, dictionary, name, descriptor);
            }
            dec_ref_bits(py, descriptor);
            dec_ref_bits(py, name);
            if exception_pending(py) {
                return false;
            }
        }
        class_bump_layout_version(class);
        true
    }
}
