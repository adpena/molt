//! Runtime-owned object/type data descriptors, installed in every builtin cohort.
//! Managed receivers never cross the C ABI. Foreign receivers delegate storage.
use crate::BuiltinExc;
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
            raise_exception::<()>(py, BuiltinExc::TypeError, "type metadata requires a type");
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
            Field::Module => match crate::object::class_storage::class_module_bits(py, class)? {
                Some(value) => value,
                None => {
                    raise_exception::<()>(py, BuiltinExc::AttributeError, field.name());
                    return Err(());
                }
            },
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
                BuiltinExc::TypeError,
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
                raise_exception::<()>(
                    py,
                    BuiltinExc::TypeError,
                    "can't delete __class__ attribute",
                );
                return Err(());
            };
            return crate::builtins::types::object_set_class(py, ptr, value);
        }
        if object_type_id(ptr) != TYPE_ID_TYPE {
            raise_exception::<()>(py, BuiltinExc::TypeError, "type metadata requires a type");
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
                    Field::Module,
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
            for &field in fields {
                let Some(name) = crate::NameId::from(field).owned_bits(py) else {
                    return false;
                };
                let descriptor = crate::builtins::types::alloc_native_descriptor(
                    py,
                    crate::builtins::types::NativeDescriptorSpec {
                        flavor: crate::object::layout::NativeDescriptorFlavor::RootMetadata,
                        operation: field as u32,
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
        }
        true
    }
}

use super::*;
use molt_cpython_abi::api::typeobj::TypeAttributeField as Field;

/// CPython's `type_get_text_signature` reads each static type's own tp_doc;
/// these are the static types whose internal doc carries a signature, from
/// the first target minor that has it (CPython 3.12-3.15 tp_doc). The newest
/// applicable row wins. Every other static type answers None, and nothing
/// inherits: the value is per type.
fn static_text_signature(py: &PyToken<'_>, class_bits: u64) -> Option<&'static str> {
    let roots = builtin_classes(py);
    let minor = crate::object::ops_sys::runtime_target_minor(py);
    [
        (roots.object, 12, "()"),
        (
            roots.property,
            12,
            "(fget=None, fset=None, fdel=None, doc=None)",
        ),
        (roots.memoryview, 12, "(object)"),
        (roots.enumerate, 12, "(iterable, start=0)"),
        (roots.reversed, 12, "(sequence, /)"),
        (roots.reversed, 15, "(object, /)"),
        (roots.complex, 12, "(real=0, imag=0)"),
        (roots.float, 12, "(x=0, /)"),
        (roots.list, 12, "(iterable=(), /)"),
        (roots.tuple, 12, "(iterable=(), /)"),
        (roots.bool, 13, "(object=False, /)"),
        (roots.set, 13, "(iterable=(), /)"),
        (roots.frozenset, 13, "(iterable=(), /)"),
        (roots.staticmethod, 13, "(function, /)"),
        (roots.classmethod, 13, "(function, /)"),
        (roots.zip, 13, "(*iterables, strict=False)"),
        (roots.map, 13, "(function, iterable, /, *iterables)"),
        (
            roots.map,
            14,
            "(function, iterable, /, *iterables, strict=False)",
        ),
        (roots.filter, 13, "(function, iterable, /)"),
    ]
    .into_iter()
    .rfind(|&(owner, since, _)| owner == class_bits && since <= minor)
    .map(|(_, _, signature)| signature)
}

// Public doc and abstract methods read only the receiver's namespace. A
// signature is never namespace content: a heap type reads its private creation
// doc, interpreted with the current name; a static type reads its declaration.
unsafe fn read_namespace_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    field: Field,
) -> Result<u64, ()> {
    unsafe {
        let bits = MoltObject::from_ptr(class).bits();
        if bits == builtin_classes(py).type_obj {
            return if field == Field::AbstractMethods {
                raise_exception::<()>(py, BuiltinExc::AttributeError, field.name());
                Err(())
            } else {
                Ok(MoltObject::none().bits())
            };
        }
        if field == Field::TextSignature {
            let signature = if crate::object::class_storage::class_is_heap_type(class) {
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
                molt_cpython_abi::api::typeobj::documentation_bytes(name, doc, true)
            } else {
                static_text_signature(py, bits).map(str::as_bytes)
            };
            let Some(signature) = signature else {
                return Ok(MoltObject::none().bits());
            };
            let result = alloc_string(py, signature);
            return if result.is_null() {
                Err(())
            } else {
                Ok(MoltObject::from_ptr(result).bits())
            };
        }
        match annotation_entry(py, class, crate::NameId::from(field)) {
            Some(value) if field == Field::Doc => {
                crate::builtins::attr::descriptor_bind(py, value, Some(bits), None)?.ok_or(())
            }
            Some(value) => {
                inc_ref_bits(py, value);
                Ok(value)
            }
            None if field == Field::AbstractMethods => {
                raise_exception::<()>(py, BuiltinExc::AttributeError, field.name());
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
        if matches!(field, Field::Doc | Field::Module) && value.is_none() {
            raise_exception::<()>(
                py,
                BuiltinExc::TypeError,
                &format!(
                    "cannot delete '{}' attribute of immutable type '{}'",
                    field.name(),
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
        let name = crate::NameId::from(field).owned_bits(py).ok_or(())?;
        let _name = crate::PtrDropGuard::preserving(
            py,
            obj_from_bits(name).as_ptr().expect("metadata name"),
        );
        let mut publication = super::TypeMutation::prepare(py, class, name).ok_or(())?;
        if matches!(field, Field::Doc | Field::Module) && !publication.invalidate_native() {
            return Err(());
        }
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        if field == Field::Module && crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
            let firstline = crate::NameId::DunderFirstlineno.owned_bits(py).ok_or(())?;
            let _firstline =
                crate::PtrDropGuard::preserving(py, obj_from_bits(firstline).as_ptr().unwrap());
            let retired = crate::object::ops::dict_del_deferred(py, dictionary, firstline)?;
            if retired.is_some() {
                class_bump_layout_version(class);
            }
            drop(retired);
        }
        let retired = match value {
            Some(value) => Some(crate::object::ops::dict_set_deferred(
                py, dictionary, name, value,
            )?),
            None => crate::object::ops::dict_del_deferred(py, dictionary, name)?,
        };
        let Some(retired) = retired else {
            raise_exception::<()>(py, BuiltinExc::AttributeError, field.name());
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
unsafe fn annotation_entry(py: &PyToken<'_>, class: *mut u8, key: crate::NameId) -> Option<u64> {
    unsafe {
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr()?;
        crate::object::ops::dict_get_str_bytes_borrowed(py, dictionary, key.as_str().as_bytes())
    }
}

/// Commit annotations before releasing displaced values, then invalidate C
/// metadata as the descriptor does. Incoming aliases remain live through every
/// update, including partial errors; retirement is not delayed across watchers.
unsafe fn update_annotations_namespace(
    py: &PyToken<'_>,
    class: *mut u8,
    updates: &[(crate::NameId, Option<u64>)],
) -> bool {
    unsafe {
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr().unwrap();
        let mut names = Vec::with_capacity(updates.len());
        let mut retired = Vec::with_capacity(updates.len());
        let mut publication = None;
        let result = (|| {
            for (name, _) in updates {
                names.push(name.owned_bits(py).ok_or(())?);
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
            return raise_exception::<_>(py, BuiltinExc::AttributeError, field.name());
        }
        let deferred = pep649_enabled(py);
        let public_key = crate::NameId::from(field);
        let cache_key: crate::NameId = match field {
            Field::Annotate if deferred => crate::NameId::DunderAnnotateFunc,
            Field::Annotations if deferred => crate::NameId::DunderAnnotationsCache,
            _ => public_key,
        };
        let existing = annotation_entry(py, class, public_key)
            .or_else(|| annotation_entry(py, class, cache_key));
        if let Some(value) = existing {
            // descriptor_bind owns the borrowed entry across __get__, passes
            // Python None as instance and the actual class as descriptor owner.
            return crate::builtins::attr::descriptor_bind(py, value, Some(class_bits), None)
                .unwrap_or_default();
        }
        if field == Field::Annotate {
            if !deferred {
                return raise_exception::<_>(py, BuiltinExc::AttributeError, field.name());
            }
            let none = MoltObject::none().bits();
            return update_annotations_namespace(py, class, &[(cache_key, Some(none))])
                .then_some(none);
        }
        let result = if deferred {
            let name = crate::NameId::DunderAnnotate.owned_bits(py)?;
            // The owning normal lookup includes metaclass overrides. Its result
            // must survive removal/replacement of the namespace's last edge.
            let annotate = molt_get_attr_name_attached_body(py, class_bits, name);
            dec_ref_bits(py, name);
            let result = if exception_pending(py) {
                MoltObject::none().bits()
            } else if is_truthy(
                py,
                obj_from_bits(molt_is_callable_attached_body(py, annotate)),
            ) {
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
            raise_exception::<()>(py, BuiltinExc::TypeError, &message);
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
        let mut updates: Vec<(crate::NameId, Option<u64>)> = Vec::with_capacity(4);
        if field == Field::Annotate && deferred {
            let Some(value) = value else {
                raise_exception::<()>(
                    py,
                    BuiltinExc::TypeError,
                    "cannot delete __annotate__ attribute",
                );
                return Err(());
            };
            if !obj_from_bits(value).is_none() {
                let callable = crate::builtins::callable::is_callable_impl(py, value);
                if !callable {
                    raise_exception::<()>(
                        py,
                        BuiltinExc::TypeError,
                        "__annotate__ must be callable or None",
                    );
                    return Err(());
                }
            }
            updates.push((crate::NameId::DunderAnnotateFunc, Some(value)));
            if !obj_from_bits(value).is_none() {
                updates.push((crate::NameId::DunderAnnotationsCache, None));
            }
        } else {
            let explicit = annotation_entry(py, class, crate::NameId::from(field));
            let key = if deferred && field == Field::Annotations && explicit.is_none() {
                crate::NameId::DunderAnnotationsCache
            } else {
                crate::NameId::from(field)
            };
            if value.is_none() && annotation_entry(py, class, key).is_none() {
                raise_exception::<()>(py, BuiltinExc::AttributeError, field.name());
                return Err(());
            }
            updates.push((key, value));
            if field == Field::Annotations && deferred {
                if explicit.is_some() {
                    updates.push((crate::NameId::DunderAnnotationsCache, None));
                }
                updates.push((crate::NameId::DunderAnnotateFunc, None));
                updates.push((crate::NameId::DunderAnnotate, None));
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
                | Field::Module
                | Field::AbstractMethods
        ) {
            raise_exception::<()>(py, BuiltinExc::AttributeError, "type metadata is read-only");
            return Err(());
        }
        let attr_name = field.name();
        // CPython's abstract-method getset admits direct descriptor writes on
        // immutable types. Normal type mutation enforces immutability earlier.
        if field != Field::AbstractMethods && crate::object::class_is_immutable(py, class) {
            raise_exception::<()>(
                py,
                BuiltinExc::TypeError,
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
        if matches!(field, Field::Doc | Field::Module | Field::AbstractMethods) {
            return write_namespace_metadata(py, class, field, value);
        }
        let Some(value) = value else {
            raise_exception::<()>(
                py,
                BuiltinExc::TypeError,
                &format!("cannot delete {attr_name} attribute"),
            );
            return Err(());
        };
        if field == Field::Bases {
            if obj_from_bits(value)
                .as_ptr()
                .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_TUPLE)
            {
                raise_exception::<()>(
                    py,
                    BuiltinExc::TypeError,
                    "__bases__ must be a tuple of classes",
                );
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
                BuiltinExc::TypeError,
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
