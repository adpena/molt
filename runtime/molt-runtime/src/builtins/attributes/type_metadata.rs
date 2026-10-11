//! Runtime-owned object/type data descriptors, installed in every builtin cohort.
//! Managed receivers never cross the C ABI. Foreign receivers delegate storage.
use crate::object::foreign::NativeObjectOwner;
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::refcount::ProtectedPyObject;

pub(crate) unsafe fn read(py: &PyToken<'_>, receiver: u64, field: Field) -> Result<u64, ()> {
    unsafe {
        if let Some(ptr) = obj_from_bits(receiver).as_ptr()
            && object_type_id(ptr) == crate::TYPE_ID_FOREIGN
        {
            let raw = crate::object::foreign::foreign_ptr_from_obj(ptr) as *mut PyObject;
            let value = NativeObjectOwner::from_owned(
                molt_cpython_abi::api::typeobj::native_type_attribute_get(
                    raw,
                    field,
                    pep649_enabled(py),
                ),
            );
            if value.as_ptr().is_null() {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native type metadata");
                return Err(());
            }
            return molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(value.as_ptr())
                .ok_or_else(|| {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "native metadata projection",
                    );
                });
        }
        if field == Field::Class {
            let class = type_of_bits(py, receiver);
            inc_ref_bits(py, class);
            return Ok(class);
        }
        let Some(class) = obj_from_bits(receiver)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
        else {
            raise_exception::<()>(py, "TypeError", "type metadata requires a type");
            return Err(());
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
            Field::Base => match crate::object::class_layout::class_best_base(py, class)? {
                Some(base) => base.direct,
                None => MoltObject::none().bits(),
            },
            Field::Bases => class_bases_bits(class),
            Field::Mro => class_mro_bits(class),
            Field::Dictionary => {
                if !crate::builtins::methods::publish_builtin_class_methods(py, receiver) {
                    return Err(());
                }
                let value =
                    crate::builtins::types::mappingproxy_from_mapping(py, class_dict_bits(class));
                return if obj_from_bits(value).is_none() {
                    Err(())
                } else {
                    Ok(value)
                };
            }
            Field::Annotate | Field::Annotations => {
                // This materializer has no missing outcome: absent storage is
                // created, or an actual exception is selected.
                return read_type_annotations(py, class, field).ok_or(());
            }
            Field::Doc | Field::TextSignature | Field::AbstractMethods => {
                return read_namespace_metadata(py, class, field);
            }
            Field::Class => unreachable!("class field handled without heap admission"),
        };
        inc_ref_bits(py, value);
        Ok(value)
    }
}

pub(crate) unsafe fn write(
    py: &PyToken<'_>,
    receiver: u64,
    field: Field,
    value: Option<u64>,
) -> Result<(), ()> {
    unsafe {
        let Some(ptr) = obj_from_bits(receiver).as_ptr() else {
            raise_exception::<()>(
                py,
                "TypeError",
                "__class__ assignment only supported for mutable types or ModuleType subclasses",
            );
            return Err(());
        };
        if object_type_id(ptr) == crate::TYPE_ID_FOREIGN {
            let raw = crate::object::foreign::foreign_ptr_from_obj(ptr) as *mut PyObject;
            let incoming = value.map(|bits| {
                inc_ref_bits(py, bits);
                ProtectedPyObject::from_owned(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits),
                )
            });
            if incoming
                .as_ref()
                .is_some_and(|object| object.as_ptr().is_null())
            {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native metadata value");
                return Err(());
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
                return Err(());
            }
            return Ok(());
        }
        if field == Field::Class {
            let Some(value) = value else {
                raise_exception::<()>(py, "TypeError", "can't delete __class__ attribute");
                return Err(());
            };
            return crate::builtins::types::object_set_class(py, ptr, value);
        }
        if object_type_id(ptr) != TYPE_ID_TYPE {
            raise_exception::<()>(py, "TypeError", "type metadata requires a type");
            return Err(());
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
unsafe fn read_namespace_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
) -> Result<u64, ()> {
    unsafe {
        let bits = MoltObject::from_ptr(class).bits();
        if bits == builtin_classes(py).type_obj {
            return if field == Field::AbstractMethods {
                raise_exception::<()>(py, "AttributeError", field.name());
                Err(())
            } else {
                Ok(MoltObject::none().bits())
            };
        }
        if field == Field::TextSignature && crate::object::class_storage::class_is_heap_type(class)
        {
            use crate::object::class_storage::ClassReferenceSlot;
            let doc = ClassReferenceSlot::CreationDoc.load(class);
            let Some(doc) = obj_from_bits(doc).as_ptr() else {
                return Ok(MoltObject::none().bits());
            };
            let name = obj_from_bits(class_name_bits(class))
                .as_ptr()
                .expect("class name");
            let name = std::slice::from_raw_parts(string_bytes(name), string_len(name));
            let doc = std::slice::from_raw_parts(string_bytes(doc), string_len(doc));
            let Some(signature) =
                molt_cpython_abi::api::typeobj::documentation_bytes(name, doc, true)
            else {
                return Ok(MoltObject::none().bits());
            };
            let result = alloc_string(py, signature);
            return if result.is_null() {
                Err(())
            } else {
                Ok(MoltObject::from_ptr(result).bits())
            };
        }
        match annotation_entry(py, class, field.name().as_bytes()) {
            Some(value) if field == Field::Doc => {
                crate::builtins::attr::descriptor_bind(py, value, Some(bits), None)?.ok_or(())
            }
            Some(value) => {
                inc_ref_bits(py, value);
                Ok(value)
            }
            None if field == Field::AbstractMethods => {
                raise_exception::<()>(py, "AttributeError", field.name());
                Err(())
            }
            None => Ok(MoltObject::none().bits()),
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
) -> Result<(), ()> {
    unsafe {
        if field == Field::Doc && value.is_none() {
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "cannot delete '__doc__' attribute of immutable type '{}'",
                    class_name_for_error(MoltObject::from_ptr(class).bits())
                ),
            );
            return Err(());
        }
        let abstract_type = if field == Field::AbstractMethods {
            match value {
                Some(value) => crate::object::ops::truth_value(py, obj_from_bits(value))?,
                None => false,
            }
        } else {
            false
        };
        let name = attr_name_bits_from_bytes(py, field.name().as_bytes()).ok_or(())?;
        let _name =
            crate::PtrDropGuard::preserving(obj_from_bits(name).as_ptr().expect("metadata name"));
        let mut publication = super::TypeMutation::prepare(py, class, name).ok_or(())?;
        if field == Field::Doc && !publication.invalidate_native() {
            return Err(());
        }
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        let retired = match value {
            Some(value) => Some(crate::object::ops::dict_set_deferred(
                py, dictionary, name, value,
            )?),
            None => crate::object::ops::dict_del_deferred(py, dictionary, name)?,
        };
        let Some(retired) = retired else {
            raise_exception::<()>(py, "AttributeError", field.name());
            return Err(());
        };
        class_bump_layout_version(class);
        drop(retired);
        if field == Field::AbstractMethods {
            if !publication.publish() {
                // The namespace and flag are committed even when later native
                // invalidation fails. Keep that selected failure while latching.
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    let _ = crate::object::class_storage::class_set_abstract(class, abstract_type);
                });
                return Err(());
            }
            if crate::object::class_storage::class_set_abstract(class, abstract_type).is_err() {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "abstract type flag publication",
                );
                return Err(());
            }
        }
        Ok(())
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
                names.push(attr_name_bits_from_bytes(py, name).ok_or(())?);
            }
            publication =
                Some(super::TypeMutation::prepare(py, class, *names.first().ok_or(())?).ok_or(())?);
            for ((_, value), name) in updates.iter().zip(names.iter().copied()) {
                if let Some(value) = value {
                    retired.push(crate::object::ops::dict_set_deferred(
                        py, dictionary, name, *value,
                    )?);
                } else if let Some(previous) =
                    crate::object::ops::dict_del_deferred(py, dictionary, name)?
                {
                    retired.push(previous);
                }
            }
            Ok::<(), ()>(())
        })();
        let changed = !retired.is_empty();
        if changed {
            class_bump_layout_version(class);
        }
        drop(retired);
        let mut published = true;
        if changed {
            let publication = publication.as_mut().expect("prepared annotation mutation");
            published = publication.publish();
        }
        drop(publication);
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            for name in names {
                dec_ref_bits(py, name);
            }
        });
        result.is_ok() && published
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
        if let Some(value) = existing {
            // descriptor_bind owns the borrowed entry across __get__, passes
            // Python None as instance and the actual class as descriptor owner.
            return match crate::builtins::attr::descriptor_bind(py, value, Some(class_bits), None) {
                Ok(value) => value,
                Err(()) => None,
            };
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
            let result = if exception_pending(py) {
                dec_ref_bits(py, result);
                None
            } else {
                Some(result)
            };
            dec_ref_bits(py, annotate);
            result?
        } else {
            let dict = alloc_dict_with_pairs(py, &[]);
            if dict.is_null() {
                return None;
            }
            MoltObject::from_ptr(dict).bits()
        };
        if obj_from_bits(result)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_DICT)
        {
            let message = format!(
                "__annotate__ returned non-dict of type '{}'",
                type_name(py, obj_from_bits(result))
            );
            raise_exception::<()>(py, "TypeError", &message);
            dec_ref_bits(py, result);
            return None;
        }
        if !update_annotations_namespace(py, class, &[(cache_key, Some(result))]) {
            dec_ref_bits(py, result);
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
) -> Result<(), ()> {
    unsafe {
        let deferred = pep649_enabled(py);
        let mut updates: Vec<(&[u8], Option<u64>)> = Vec::with_capacity(4);
        if field == Field::Annotate && deferred {
            let Some(value) = value else {
                raise_exception::<()>(py, "TypeError", "cannot delete __annotate__ attribute");
                return Err(());
            };
            if !obj_from_bits(value).is_none() {
                let callable = crate::builtins::callable::is_callable_impl(py, value);
                if !callable {
                    raise_exception::<()>(py, "TypeError", "__annotate__ must be callable or None");
                    return Err(());
                }
            }
            updates.push((b"__annotate_func__", Some(value)));
            if !obj_from_bits(value).is_none() {
                updates.push((b"__annotations_cache__", None));
            }
        } else {
            let explicit = annotation_entry(py, class, field.name().as_bytes());
            let key = if deferred && field == Field::Annotations && explicit.is_none() {
                b"__annotations_cache__".as_slice()
            } else {
                field.name().as_bytes()
            };
            if value.is_none() && annotation_entry(py, class, key).is_none() {
                raise_exception::<()>(py, "AttributeError", field.name());
                return Err(());
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
        if update_annotations_namespace(py, class, &updates) {
            Ok(())
        } else {
            Err(())
        }
    }
}

pub(crate) unsafe fn write_type_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
    value: Option<u64>,
) -> Result<(), ()> {
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
            raise_exception::<()>(py, "AttributeError", "type metadata is read-only");
            return Err(());
        }
        let attr_name = field.name();
        // CPython's abstract-method getset admits direct descriptor writes on
        // immutable types. Normal type mutation enforces immutability earlier.
        if field != Field::AbstractMethods && crate::object::class_is_immutable(py, class) {
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "cannot set '{attr_name}' attribute of immutable type '{}'",
                    class_name_for_error(class_bits)
                ),
            );
            return Err(());
        }
        if matches!(field, Field::Annotations | Field::Annotate) {
            return write_type_annotations(py, class, field, value);
        }
        if matches!(field, Field::Doc | Field::AbstractMethods) {
            return write_namespace_metadata(py, class, field, value);
        }
        let Some(value) = value else {
            raise_exception::<()>(
                py,
                "TypeError",
                &format!("cannot delete {attr_name} attribute"),
            );
            return Err(());
        };
        if field == Field::Bases {
            if obj_from_bits(value)
                .as_ptr()
                .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_TUPLE)
            {
                raise_exception::<()>(py, "TypeError", "__bases__ must be a tuple of classes");
                return Err(());
            }
            return crate::builtins::types::class_set_base(py, class_bits, value);
        }
        if obj_from_bits(value)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_STRING)
        {
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "can only assign string to {}.{attr_name}, not '{}'",
                    class_name_for_error(class_bits),
                    type_name(py, obj_from_bits(value))
                ),
            );
            return Err(());
        }
        let mut publication = super::TypeMutation::runtime_only(py, class);
        let published = if field == Field::Name {
            class_set_name_bits(py, class, value)
        } else {
            class_set_qualname_bits(py, class, value)
        };
        if published && publication.publish() {
            Ok(())
        } else {
            Err(())
        }
    }
}
