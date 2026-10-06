//! Runtime-owned object/type data descriptors, installed in every builtin cohort.
//! Managed receivers never cross the C ABI. Foreign receivers delegate storage.
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::refcount::OwnedPyObject;

pub(crate) unsafe fn read(py: &PyToken<'_>, receiver: u64, field: Field) -> u64 {
    unsafe {
        if let Some(ptr) = obj_from_bits(receiver).as_ptr()
            && object_type_id(ptr) == crate::TYPE_ID_FOREIGN
        {
            let raw = crate::object::foreign::foreign_ptr_from_obj(ptr) as *mut PyObject;
            let value = OwnedPyObject::from_owned(
                molt_cpython_abi::api::typeobj::native_type_attribute_get(
                    raw,
                    field,
                    pep649_enabled(py),
                ),
            );
            if value.as_ptr().is_null() {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native type metadata");
                return MoltObject::none().bits();
            }
            let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(value.as_ptr());
            if let Some(bits) = bits {
                return bits;
            }
            crate::cpython_abi_hooks::propagate_native_failure(py, "native metadata projection");
            return MoltObject::none().bits();
        }
        if field == Field::Class {
            let class = type_of_bits(py, receiver);
            inc_ref_bits(py, class);
            return class;
        }
        let Some(class) = obj_from_bits(receiver)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
        else {
            return raise_exception::<_>(py, "TypeError", "type metadata requires a type");
        };
        let value = match field {
            Field::Name => class_name_bits(class),
            Field::QualName => {
                let name = class_qualname_bits(class);
                if name == 0 {
                    class_name_bits(class)
                } else {
                    name
                }
            }
            Field::Base => match crate::object::class_layout::class_best_base(py, class) {
                Ok(Some(base)) => base.direct,
                Ok(None) => MoltObject::none().bits(),
                Err(()) => return MoltObject::none().bits(),
            },
            Field::Bases => class_bases_bits(class),
            Field::Mro => class_mro_bits(class),
            Field::Dictionary => {
                if !crate::builtins::methods::publish_builtin_class_methods(py, receiver) {
                    return MoltObject::none().bits();
                }
                return crate::builtins::types::mappingproxy_from_mapping(
                    py,
                    class_dict_bits(class),
                );
            }
            Field::Annotate | Field::Annotations => {
                return match read_type_annotations(py, class, field) {
                    Some(value) => value,
                    None if exception_pending(py) => MoltObject::none().bits(),
                    None => raise_exception::<_>(py, "AttributeError", field.name()),
                };
            }
            Field::Doc | Field::TextSignature | Field::AbstractMethods => {
                return read_namespace_metadata(py, class, field);
            }
            Field::Class => unreachable!("class field handled without heap admission"),
        };
        inc_ref_bits(py, value);
        value
    }
}

pub(crate) unsafe fn write(
    py: &PyToken<'_>,
    receiver: u64,
    field: Field,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let Some(ptr) = obj_from_bits(receiver).as_ptr() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "__class__ assignment only supported for mutable types or ModuleType subclasses",
            );
        };
        if object_type_id(ptr) == crate::TYPE_ID_FOREIGN {
            let raw = crate::object::foreign::foreign_ptr_from_obj(ptr) as *mut PyObject;
            let incoming = value.map(|bits| {
                inc_ref_bits(py, bits);
                OwnedPyObject::from_owned(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits),
                )
            });
            if incoming
                .as_ref()
                .is_some_and(|object| object.as_ptr().is_null())
            {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native metadata value");
                return MoltObject::none().bits();
            }
            let status = molt_cpython_abi::api::typeobj::native_type_attribute_set(
                raw,
                field,
                incoming
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |object| object.as_ptr()),
                pep649_enabled(py),
            );
            if status < 0 {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native metadata mutation");
            }
            return MoltObject::none().bits();
        }
        if field == Field::Class {
            let Some(value) = value else {
                return raise_exception::<_>(py, "TypeError", "can't delete __class__ attribute");
            };
            return crate::builtins::types::object_set_class(py, ptr, value);
        }
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>(py, "TypeError", "type metadata requires a type");
        }
        write_type_metadata(py, ptr, field, value)
    }
}

pub(crate) unsafe fn publish(py: &PyToken<'_>) -> bool {
    unsafe {
        let roots = builtin_classes(py);
        for (owner, fields) in [
            (roots.object, &[Field::Class][..]),
            (
                roots.type_obj,
                &[
                    Field::Name,
                    Field::QualName,
                    Field::Base,
                    Field::Bases,
                    Field::Mro,
                    Field::Dictionary,
                    Field::Doc,
                    Field::TextSignature,
                    Field::AbstractMethods,
                    Field::Annotations,
                ][..],
            ),
        ] {
            let class = obj_from_bits(owner).as_ptr().unwrap();
            let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
            for field in fields {
                let Some(name) = attr_name_bits_from_bytes(py, field.name().as_bytes()) else {
                    return false;
                };
                let descriptor = crate::builtins::types::alloc_native_descriptor(
                    py,
                    crate::builtins::types::NativeDescriptorSpec {
                        flavor: crate::object::layout::NativeDescriptorFlavor::RootMetadata,
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
        }
        true
    }
}

use super::*;
use molt_cpython_abi::api::typeobj::TypeAttributeField as Field;

// Public doc and abstract methods read only the receiver's namespace. A heap
// signature reads the private creation doc, interpreted with the current name.
unsafe fn read_namespace_metadata(py: &PyToken<'_>, class: *mut u8, field: Field) -> u64 {
    unsafe {
        let bits = MoltObject::from_ptr(class).bits();
        if bits == builtin_classes(py).type_obj {
            // The root's own namespace contains these descriptors. It has no
            // internal documentation/signature declaration to expose.
            return if field == Field::AbstractMethods {
                raise_exception::<_>(py, "AttributeError", field.name())
            } else {
                MoltObject::none().bits()
            };
        }
        if field == Field::TextSignature && crate::object::class_storage::class_is_heap_type(class)
        {
            use crate::object::class_storage::ClassReferenceSlot;
            let doc = ClassReferenceSlot::CreationDoc.load(class);
            let Some(doc) = obj_from_bits(doc).as_ptr() else {
                return MoltObject::none().bits();
            };
            let name = obj_from_bits(class_name_bits(class))
                .as_ptr()
                .expect("class name");
            let name = std::slice::from_raw_parts(string_bytes(name), string_len(name));
            let doc = std::slice::from_raw_parts(string_bytes(doc), string_len(doc));
            let Some(signature) =
                molt_cpython_abi::api::typeobj::documentation_bytes(name, doc, true)
            else {
                return MoltObject::none().bits();
            };
            let result = alloc_string(py, signature);
            return if result.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(result).bits()
            };
        }
        let value = annotation_entry(py, class, field.name().as_bytes());
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        match value {
            Some(value) if field == Field::Doc => {
                crate::builtins::attr::descriptor_bind(py, value, Some(bits), None)
                    .unwrap_or_else(|| MoltObject::none().bits())
            }
            Some(value) => {
                inc_ref_bits(py, value);
                value
            }
            None if field == Field::AbstractMethods => {
                raise_exception::<_>(py, "AttributeError", field.name())
            }
            None => MoltObject::none().bits(),
        }
    }
}

/// Preserve descriptor-owned CPython ordering. Doc invalidates before store;
/// abstract methods release the old namespace value, invalidate, then latch the
/// flag. Truth conversion still runs exactly once before dictionary mutation.
unsafe fn write_namespace_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
    value: Option<u64>,
) -> u64 {
    unsafe {
        if field == Field::Doc && value.is_none() {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "cannot delete '__doc__' attribute of immutable type '{}'",
                    class_name_for_error(MoltObject::from_ptr(class).bits())
                ),
            );
        }
        let abstract_type = if field == Field::AbstractMethods {
            value.is_some_and(|value| is_truthy(py, obj_from_bits(value)))
        } else {
            false
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(name) = attr_name_bits_from_bytes(py, field.name().as_bytes()) else {
            return MoltObject::none().bits();
        };
        let _name =
            crate::PtrDropGuard::preserving(obj_from_bits(name).as_ptr().expect("metadata name"));
        let Some(mut publication) = super::TypeMutation::prepare(py, class, name) else {
            return MoltObject::none().bits();
        };
        if field == Field::Doc && !publication.invalidate_native() {
            return MoltObject::none().bits();
        }
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        let retired = match value {
            Some(value) => crate::object::ops::dict_set_deferred(py, dictionary, name, value).ok(),
            None => crate::object::ops::dict_del_deferred(py, dictionary, name),
        };
        if let Some(retired) = retired {
            // The dictionary is committed before releasing its old owner. Do
            // not move release across the descriptor's observable C invalidation.
            class_bump_layout_version(class);
            molt_cpython_abi::api::errors::with_preserved_error(|| drop(retired));
            if field == Field::AbstractMethods {
                publication.publish();
                if exception_pending(py) {
                    // The dictionary and runtime flag remain committed even if
                    // an existing C view is in failed/recursive publication.
                    // Preserve the first publication failure during flag update.
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        let _ =
                            crate::object::class_storage::class_set_abstract(class, abstract_type);
                    });
                } else if crate::object::class_storage::class_set_abstract(class, abstract_type)
                    .is_err()
                {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "abstract type flag publication",
                    );
                }
            }
        } else if !exception_pending(py) {
            raise_exception::<()>(py, "AttributeError", field.name());
        }
        MoltObject::none().bits()
    }
}

// Class annotations have exactly one storage owner: the class namespace.
// Python 3.14 separates explicit values from generated evaluators/lazy caches.
unsafe fn annotation_entry(py: &PyToken<'_>, class: *mut u8, key: &[u8]) -> Option<u64> {
    unsafe {
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr()?;
        crate::object::ops::dict_get_str_bytes_borrowed(py, dictionary, key)
    }
}

/// Commit annotations before releasing displaced values, then invalidate C
/// metadata as the descriptor does. Incoming aliases remain live through every
/// update, including partial errors; retirement is not delayed across watchers.
unsafe fn update_annotations_namespace(
    py: &PyToken<'_>,
    class: *mut u8,
    updates: &[(&[u8], Option<u64>)],
) -> bool {
    unsafe {
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        let mut names = Vec::with_capacity(updates.len());
        let mut retired = Vec::with_capacity(updates.len());
        let mut publication = None;
        let result = (|| {
            for (name, _) in updates {
                names.push(attr_name_bits_from_bytes(py, name)?);
            }
            publication = Some(super::TypeMutation::prepare(py, class, *names.first()?)?);
            for ((_, value), name) in updates.iter().zip(names.iter().copied()) {
                if let Some(value) = value {
                    retired.push(
                        crate::object::ops::dict_set_deferred(py, dictionary, name, *value).ok()?,
                    );
                } else if let Some(previous) =
                    crate::object::ops::dict_del_deferred(py, dictionary, name)
                {
                    retired.push(previous);
                }
                if exception_pending(py) {
                    return None;
                }
            }
            Some(())
        })();
        let changed = !retired.is_empty();
        if changed {
            class_bump_layout_version(class);
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| drop(retired));
        let mut published = true;
        if changed {
            let publication = publication.as_mut().expect("prepared annotation mutation");
            if exception_pending(py) {
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    published = publication.publish();
                });
            } else {
                published = publication.publish();
            }
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            drop(publication);
            for name in names {
                dec_ref_bits(py, name);
            }
        });
        result.is_some() && published
    }
}

pub(crate) unsafe fn read_type_annotations(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
) -> Option<u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(class).bits();
        if !crate::object::class_storage::class_is_heap_type(class) {
            return raise_exception::<_>(py, "AttributeError", field.name());
        }
        let deferred = pep649_enabled(py);
        let public_key = field.name().as_bytes();
        let cache_key: &[u8] = match field {
            Field::Annotate if deferred => b"__annotate_func__",
            Field::Annotations if deferred => b"__annotations_cache__",
            _ => public_key,
        };
        let existing = annotation_entry(py, class, public_key)
            .or_else(|| annotation_entry(py, class, cache_key));
        if exception_pending(py) {
            return None;
        }
        if let Some(value) = existing {
            // descriptor_bind owns the borrowed entry across __get__, passes
            // Python None as instance and the actual class as descriptor owner.
            return crate::builtins::attr::descriptor_bind(py, value, Some(class_bits), None);
        }
        if field == Field::Annotate {
            if !deferred {
                return raise_exception::<_>(py, "AttributeError", field.name());
            }
            let none = MoltObject::none().bits();
            return update_annotations_namespace(py, class, &[(cache_key, Some(none))])
                .then_some(none);
        }
        let result = if deferred {
            let name = attr_name_bits_from_bytes(py, b"__annotate__")?;
            // The owning normal lookup includes metaclass overrides. Its result
            // must survive removal/replacement of the namespace's last edge.
            let annotate = molt_get_attr_name(class_bits, name);
            dec_ref_bits(py, name);
            let result = if exception_pending(py) {
                MoltObject::none().bits()
            } else if is_truthy(py, obj_from_bits(molt_is_callable(annotate))) {
                call_callable1(py, annotate, MoltObject::from_int(1).bits())
            } else if exception_pending(py) {
                MoltObject::none().bits()
            } else {
                let dict = alloc_dict_with_pairs(py, &[]);
                if dict.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(dict).bits()
                }
            };
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, annotate));
            result
        } else {
            let dict = alloc_dict_with_pairs(py, &[]);
            if dict.is_null() {
                return None;
            }
            MoltObject::from_ptr(dict).bits()
        };
        if exception_pending(py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
            return None;
        }
        if obj_from_bits(result)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_DICT)
        {
            let message = format!(
                "__annotate__ returned non-dict of type '{}'",
                type_name(py, obj_from_bits(result))
            );
            raise_exception::<()>(py, "TypeError", &message);
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
            return None;
        }
        if !update_annotations_namespace(py, class, &[(cache_key, Some(result))]) {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
            return None;
        }
        Some(result)
    }
}

unsafe fn write_type_annotations(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let deferred = pep649_enabled(py);
        let mut updates: Vec<(&[u8], Option<u64>)> = Vec::with_capacity(4);
        if field == Field::Annotate && deferred {
            let Some(value) = value else {
                return raise_exception::<_>(
                    py,
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                );
            };
            if !obj_from_bits(value).is_none() {
                let callable = is_truthy(py, obj_from_bits(molt_is_callable(value)));
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                if !callable {
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        "__annotate__ must be callable or None",
                    );
                }
            }
            updates.push((b"__annotate_func__", Some(value)));
            if !obj_from_bits(value).is_none() {
                updates.push((b"__annotations_cache__", None));
            }
        } else {
            let explicit = annotation_entry(py, class, field.name().as_bytes());
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let key = if deferred && field == Field::Annotations && explicit.is_none() {
                b"__annotations_cache__".as_slice()
            } else {
                field.name().as_bytes()
            };
            if value.is_none() && annotation_entry(py, class, key).is_none() {
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(py, "AttributeError", field.name());
            }
            updates.push((key, value));
            if field == Field::Annotations && deferred {
                if explicit.is_some() {
                    updates.push((b"__annotations_cache__", None));
                }
                updates.push((b"__annotate_func__", None));
                updates.push((b"__annotate__", None));
            }
        }
        update_annotations_namespace(py, class, &updates);
        MoltObject::none().bits()
    }
}

pub(crate) unsafe fn write_type_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(class).bits();
        if !matches!(
            field,
            Field::Name
                | Field::QualName
                | Field::Bases
                | Field::Annotations
                | Field::Annotate
                | Field::Doc
                | Field::AbstractMethods
        ) {
            return raise_exception::<_>(py, "AttributeError", "type metadata is read-only");
        }
        let attr_name = field.name();
        // CPython's abstract-method getset admits direct descriptor writes on
        // immutable types. Normal type mutation enforces immutability earlier.
        if field != Field::AbstractMethods && crate::object::class_is_immutable(py, class) {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "cannot set '{attr_name}' attribute of immutable type '{}'",
                    class_name_for_error(class_bits)
                ),
            );
        }
        if matches!(field, Field::Annotations | Field::Annotate) {
            return write_type_annotations(py, class, field, value);
        }
        if matches!(field, Field::Doc | Field::AbstractMethods) {
            return write_namespace_metadata(py, class, field, value);
        }
        let Some(value) = value else {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!("cannot delete {attr_name} attribute"),
            );
        };
        if field == Field::Bases {
            if obj_from_bits(value)
                .as_ptr()
                .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_TUPLE)
            {
                return raise_exception::<_>(
                    py,
                    "TypeError",
                    "__bases__ must be a tuple of classes",
                );
            }
            return molt_class_set_base(class_bits, value);
        }
        if obj_from_bits(value)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_STRING)
        {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "can only assign string to {}.{attr_name}, not '{}'",
                    class_name_for_error(class_bits),
                    type_name(py, obj_from_bits(value))
                ),
            );
        }
        let mut publication = super::TypeMutation::runtime_only(py, class);
        let published = if field == Field::Name {
            class_set_name_bits(py, class, value)
        } else {
            class_set_qualname_bits(py, class, value)
        };
        if published {
            publication.publish();
        }
        MoltObject::none().bits()
    }
}
