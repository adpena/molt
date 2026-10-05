//! Native storage delegate for runtime-owned object/type data descriptors.
//! This module publishes no root descriptor table; deferred annotations perform
//! ordinary attribute lookup for the evaluator so metaclass overrides participate.
use super::*;
use crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE;
use crate::api::{errors, mapping, object, refcount::OwnedPyObject, strings};

/// Python-visible type attributes, including callback-capable annotations.
/// Deliberately separate from the callback-free structural TypeMetadataField.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeAttributeField {
    Class = 0,
    Name = 1,
    QualName = 2,
    Base = 3,
    Bases = 4,
    Mro = 5,
    Dictionary = 6,
    Annotations = 7,
    Annotate = 8,
    Doc = 9,
    TextSignature = 10,
    AbstractMethods = 11,
}
impl TypeAttributeField {
    pub fn from_operation(operation: u32) -> Option<Self> {
        Some(match operation {
            0 => Self::Class,
            1 => Self::Name,
            2 => Self::QualName,
            3 => Self::Base,
            4 => Self::Bases,
            5 => Self::Mro,
            6 => Self::Dictionary,
            7 => Self::Annotations,
            8 => Self::Annotate,
            9 => Self::Doc,
            10 => Self::TextSignature,
            11 => Self::AbstractMethods,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Class => "__class__",
            Self::Name => "__name__",
            Self::QualName => "__qualname__",
            Self::Base => "__base__",
            Self::Bases => "__bases__",
            Self::Mro => "__mro__",
            Self::Dictionary => "__dict__",
            Self::Annotations => "__annotations__",
            Self::Annotate => "__annotate__",
            Self::Doc => "__doc__",
            Self::TextSignature => "__text_signature__",
            Self::AbstractMethods => "__abstractmethods__",
        }
    }
}
use TypeAttributeField as Field;

unsafe fn runtime_type(tp: *mut PyTypeObject) -> bool {
    GLOBAL_BRIDGE
        .observed_handle_for_pyobj(tp.cast())
        .is_some_and(|value| unsafe {
            (crate::hooks::hooks_or_stubs().classify_heap)(value.bits())
                == crate::abi_types::MoltTypeTag::Type as u8
        })
}

pub(super) unsafe fn add_type_documentation(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        if runtime_type(tp) {
            return 0;
        }
        let existing = mapping::_PyDict_GetItemStringWithError((*tp).tp_dict, c"__doc__".as_ptr());
        if descriptors::pending() {
            return -1;
        }
        if !existing.is_null() {
            return 0;
        }
        let qualified = std::ffi::CStr::from_ptr((*tp).tp_name).to_bytes();
        let name = qualified
            .rsplit(|byte| *byte == b'.')
            .next()
            .unwrap_or(qualified);
        let short_name = (*tp).tp_name.add(qualified.len() - name.len());
        let documentation = OwnedPyObject::from_owned(descriptors::documentation_part(
            short_name,
            (*tp).tp_doc,
            false,
        ));
        if documentation.as_ptr().is_null() {
            return -1;
        }
        mapping::PyDict_SetItemString((*tp).tp_dict, c"__doc__".as_ptr(), documentation.as_ptr())
    }
}

/// Internal native documentation is independent of the mutable namespace.
/// Heap docs bind an own descriptor; abstract methods never bind or inherit.
unsafe fn native_namespace_metadata(tp: *mut PyTypeObject, field: Field) -> *mut PyObject {
    unsafe {
        if field == Field::TextSignature
            || (field == Field::Doc
                && (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0
                && !(*tp).tp_doc.is_null())
        {
            let qualified = std::ffi::CStr::from_ptr((*tp).tp_name).to_bytes();
            let short = qualified
                .rsplit(|byte| *byte == b'.')
                .next()
                .unwrap_or(qualified);
            return descriptors::documentation_part(
                (*tp).tp_name.add(qualified.len() - short.len()),
                (*tp).tp_doc,
                field == Field::TextSignature,
            );
        }
        let value =
            if field == Field::AbstractMethods && tp == &raw mut crate::abi_types::PyType_Type {
                ptr::null_mut()
            } else {
                let dictionary = type_dict_borrowed(tp);
                if dictionary.is_null() {
                    return ptr::null_mut();
                }
                let key = if field == Field::Doc {
                    c"__doc__"
                } else {
                    c"__abstractmethods__"
                };
                mapping::_PyDict_GetItemStringWithError(dictionary, key.as_ptr())
            };
        if descriptors::pending() {
            return ptr::null_mut();
        }
        if value.is_null() {
            if field == Field::Doc {
                return object::Py_NewRef(&raw mut crate::abi_types::Py_None);
            }
            errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_AttributeError).cast(),
                c"__abstractmethods__".as_ptr(),
            );
            return ptr::null_mut();
        }
        let value = OwnedPyObject::from_borrowed(value);
        if field == Field::Doc {
            crate::api::descriptor::get(value.as_ptr(), ptr::null_mut(), tp.cast())
                .unwrap_or_else(|| value.into_ptr())
        } else {
            value.into_ptr()
        }
    }
}

unsafe fn set_native_namespace_metadata(
    tp: *mut PyTypeObject,
    field: Field,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        if field == Field::Doc && value.is_null() {
            errors::PyErr_Format(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                c"cannot delete '__doc__' attribute of immutable type '%s'".as_ptr(),
                (*tp).tp_name,
            );
            return -1;
        }
        let incoming = OwnedPyObject::from_borrowed(value);
        let abstract_type = if field == Field::AbstractMethods && !value.is_null() {
            let truth = object::PyObject_IsTrue(value);
            if truth < 0 {
                return -1;
            }
            truth != 0
        } else {
            false
        };
        let dictionary = OwnedPyObject::from_owned(PyType_GetDict(tp));
        if dictionary.as_ptr().is_null() {
            return -1;
        }
        let key =
            OwnedPyObject::from_owned(strings::PyUnicode_FromString(if field == Field::Doc {
                c"__doc__".as_ptr()
            } else {
                c"__abstractmethods__".as_ptr()
            }));
        if key.as_ptr().is_null() {
            return -1;
        }
        struct Publication {
            tp: *mut PyTypeObject,
            field: Field,
            abstract_type: bool,
        }
        unsafe extern "C" fn publish(context: *mut std::ffi::c_void) -> c_int {
            let state = unsafe { &*context.cast::<Publication>() };
            unsafe {
                if state.field == Field::AbstractMethods {
                    if state.abstract_type {
                        (*state.tp).tp_flags |= crate::abi_types::Py_TPFLAGS_IS_ABSTRACT;
                    } else {
                        (*state.tp).tp_flags &= !crate::abi_types::Py_TPFLAGS_IS_ABSTRACT;
                    }
                }
                PyType_Modified(state.tp);
            }
            0
        }
        let mut state = Publication {
            tp,
            field,
            abstract_type,
        };
        let status = mapping::dict_mutate(
            dictionary.as_ptr(),
            key.as_ptr(),
            incoming.as_ptr(),
            value.is_null(),
            Some(publish),
            (&raw mut state).cast(),
        );
        if status < 0
            && value.is_null()
            && errors::PyErr_ExceptionMatches((&raw mut crate::abi_types::PyExc_KeyError).cast())
                != 0
        {
            errors::PyErr_Clear();
            errors::PyErr_SetObject(
                (&raw mut crate::abi_types::PyExc_AttributeError).cast(),
                key.as_ptr(),
            );
        }
        status
    }
}

struct MetadataMutation(*mut PyTypeObject);
impl Drop for MetadataMutation {
    fn drop(&mut self) {
        unsafe { PyType_Modified(self.0) };
    }
}

pub unsafe fn native_type_attribute_get(
    object: *mut PyObject,
    field: Field,
    deferred: bool,
) -> *mut PyObject {
    unsafe {
        if field == Field::Class {
            return PyObject_Type(object);
        }
        if PyType_Check(object) == 0 {
            reject_type_layout(c"type metadata requires a type");
            return ptr::null_mut();
        }
        let tp = object.cast::<PyTypeObject>();
        match field {
            Field::Name => PyType_GetName(tp),
            Field::QualName => PyType_GetQualName(tp),
            Field::Dictionary => {
                let dictionary = type_dict_borrowed(tp);
                if dictionary.is_null() {
                    ptr::null_mut()
                } else {
                    mapping::PyDictProxy_New(dictionary)
                }
            }
            Field::Base | Field::Bases | Field::Mro => {
                if PyType_Ready(tp) < 0 {
                    return ptr::null_mut();
                }
                let value = match field {
                    Field::Base => {
                        if (*tp).tp_base.is_null() {
                            &raw mut crate::abi_types::Py_None
                        } else {
                            (*tp).tp_base.cast()
                        }
                    }
                    Field::Bases => (*tp).tp_bases,
                    _ => (*tp).tp_mro,
                };
                object::Py_NewRef(value)
            }
            Field::Annotations | Field::Annotate => {
                native_annotation(tp, field, ptr::null_mut(), false, false, deferred)
            }
            Field::Doc | Field::TextSignature | Field::AbstractMethods => {
                native_namespace_metadata(tp, field)
            }
            Field::Class => unreachable!(),
        }
    }
}

pub unsafe fn native_type_attribute_set(
    object: *mut PyObject,
    field: Field,
    value: *mut PyObject,
    deferred: bool,
) -> c_int {
    unsafe {
        if field == Field::Class {
            if value.is_null() {
                return reject_type_layout(c"can't delete __class__ attribute");
            }
            if PyType_Check(value) == 0 {
                return reject_type_layout(c"__class__ must be set to a class");
            }
            return hierarchy::assign_class(object, value.cast());
        }
        if PyType_Check(object) == 0 {
            return reject_type_layout(c"type metadata requires a type");
        }
        let tp = object.cast::<PyTypeObject>();
        if field != Field::AbstractMethods
            && ((*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0
                || (*tp).tp_flags & Py_TPFLAGS_IMMUTABLETYPE != 0)
        {
            return reject_type_layout(c"cannot set metadata of an immutable type");
        }
        match field {
            Field::Name | Field::QualName => set_native_name(tp, field, value),
            Field::Bases => hierarchy::set_bases(tp, value),
            Field::Doc | Field::AbstractMethods => set_native_namespace_metadata(tp, field, value),
            Field::Annotations | Field::Annotate => {
                let result = native_annotation(tp, field, value, true, value.is_null(), deferred);
                if result.is_null() {
                    -1
                } else {
                    errors::release_preserving_error(&[result]);
                    0
                }
            }
            _ => {
                errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_AttributeError).cast(),
                    c"type metadata is read-only".as_ptr(),
                );
                -1
            }
        }
    }
}

pub(crate) unsafe extern "C" fn type_setattro(
    object: *mut PyObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        if (*object.cast::<PyTypeObject>()).tp_flags & Py_TPFLAGS_IMMUTABLETYPE != 0 {
            errors::PyErr_Format(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                c"cannot set %R attribute of immutable type '%s'".as_ptr(),
                name,
                (*object.cast::<PyTypeObject>()).tp_name,
            );
            return -1;
        }
        if !object::require_attribute_name(name) {
            return -1;
        }
        // CPython type_setattro canonicalizes string subclasses here. Normal
        // user overrides and raw GenericSetAttr keep their original name object.
        let name = OwnedPyObject::from_owned(strings::PyUnicode_FromObject(name));
        if name.as_ptr().is_null() {
            return -1;
        }
        if GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_some() {
            return object::managed_set_attr(
                object,
                name.as_ptr(),
                value,
                crate::hooks::AttributeMutation::TypeDefault,
            );
        }
        native_type_namespace_mutation(object.cast(), name.as_ptr(), value)
    }
}

/// Native type defaults own namespace mutation and physical slot publication.
/// Descriptor writers keep their own metadata transactions. Namespace values,
/// name/value aliases and the type/dictionary stay pinned until publication and
/// cache invalidation complete, matching the annotation transaction below.
unsafe fn native_type_namespace_mutation(
    tp: *mut PyTypeObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        let _type_owner = OwnedPyObject::from_borrowed(tp.cast());
        let incoming = OwnedPyObject::from_borrowed(value);
        let meta = crate::bridge::semantic_type(tp.cast());
        if meta.is_null() {
            return -1;
        }
        let _meta_owner = OwnedPyObject::from_borrowed(meta.cast());
        let descriptor = OwnedPyObject::from_borrowed(_PyType_Lookup(meta, name));
        if descriptors::pending() {
            return -1;
        }
        if let Some(status) = crate::api::descriptor::set(descriptor.as_ptr(), tp.cast(), value) {
            return status;
        }
        let dictionary = OwnedPyObject::from_owned(PyType_GetDict(tp));
        if dictionary.as_ptr().is_null() {
            return -1;
        }
        let slots = match super::native_slot_mutation::prepare(tp, name) {
            Ok(slots) => slots,
            Err(()) => return -1,
        };
        struct Publication<'a> {
            tp: *mut PyTypeObject,
            slots: &'a super::native_slot_mutation::Mutation,
        }
        unsafe extern "C" fn publish(context: *mut std::ffi::c_void) -> c_int {
            let publication = unsafe { &*context.cast::<Publication<'_>>() };
            unsafe {
                PyType_Modified(publication.tp);
                publication.slots.publish()
            }
        }
        let mut publication = Publication { tp, slots: &slots };
        let status = mapping::dict_mutate(
            dictionary.as_ptr(),
            name,
            incoming.as_ptr(),
            value.is_null(),
            Some(publish),
            (&raw mut publication).cast(),
        );
        if status < 0
            && value.is_null()
            && errors::PyErr_ExceptionMatches((&raw mut crate::abi_types::PyExc_KeyError).cast())
                != 0
        {
            errors::PyErr_Clear();
            errors::PyErr_Format(
                (&raw mut crate::abi_types::PyExc_AttributeError).cast(),
                c"type object '%s' has no attribute %R".as_ptr(),
                (*tp).tp_name,
                name,
            );
        }
        status
    }
}

unsafe fn set_native_name(tp: *mut PyTypeObject, field: Field, value: *mut PyObject) -> c_int {
    unsafe {
        if value.is_null() {
            return reject_type_layout(c"cannot delete type name");
        }
        if strings::PyUnicode_Check(value) == 0 {
            return reject_type_layout(c"type name must be a string");
        }
        let name = OwnedPyObject::from_borrowed(value);
        let Some(heap) = heap_type_storage(tp) else {
            return reject_type_layout(c"type name mutation requires proven heap storage");
        };
        let old = if field == Field::Name {
            let mut size = 0;
            let bytes = strings::PyUnicode_AsUTF8AndSize(value, &raw mut size);
            if bytes.is_null() {
                return -1;
            }
            if std::slice::from_raw_parts(bytes.cast::<u8>(), size as usize).contains(&0) {
                errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                    c"type name must not contain null characters".as_ptr(),
                );
                return -1;
            }
            let owned = crate::api::memory::PyMem_Malloc(size as usize + 1).cast::<c_char>();
            if owned.is_null() {
                errors::PyErr_NoMemory();
                return -1;
            }
            ptr::copy_nonoverlapping(bytes, owned, size as usize + 1);
            let previous = std::mem::replace(&mut (*heap)._ht_tpname, owned);
            (*tp).tp_name = owned;
            let old = std::mem::replace(&mut (*heap).ht_name, name.into_ptr());
            crate::api::memory::PyMem_Free(previous.cast());
            old
        } else {
            std::mem::replace(&mut (*heap).ht_qualname, name.into_ptr())
        };
        PyType_Modified(tp);
        errors::release_preserving_error(&[old]);
        0
    }
}

unsafe fn annotation_entry(dictionary: *mut PyObject, key: &std::ffi::CStr) -> *mut PyObject {
    unsafe { mapping::_PyDict_GetItemStringWithError(dictionary, key.as_ptr()) }
}

unsafe fn delete_annotation_entry(dictionary: *mut PyObject, key: &std::ffi::CStr) -> c_int {
    unsafe {
        let existing = annotation_entry(dictionary, key);
        if descriptors::pending() {
            return -1;
        }
        if existing.is_null() {
            return 0;
        }
        if mapping::PyDict_DelItemString(dictionary, key.as_ptr()) < 0 {
            -1
        } else {
            1
        }
    }
}

unsafe fn missing_annotation(field: Field) -> *mut PyObject {
    unsafe {
        errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_AttributeError).cast(),
            if field == Field::Annotate {
                c"__annotate__".as_ptr()
            } else {
                c"__annotations__".as_ptr()
            },
        );
        ptr::null_mut()
    }
}

unsafe fn native_annotation(
    tp: *mut PyTypeObject,
    field: Field,
    value: *mut PyObject,
    write: bool,
    delete: bool,
    deferred: bool,
) -> *mut PyObject {
    unsafe {
        // Static native types never gain annotation storage as a side effect of
        // reading a runtime-owned descriptor, even when tp_dict has that key.
        if !write && (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0 {
            return missing_annotation(field);
        }
        let dictionary = OwnedPyObject::from_owned(PyType_GetDict(tp));
        let dict = dictionary.as_ptr();
        if dict.is_null() {
            return ptr::null_mut();
        }
        let name = if field == Field::Annotate {
            c"__annotate__"
        } else {
            c"__annotations__"
        };
        if write {
            if field == Field::Annotate && deferred {
                if delete {
                    reject_type_layout(c"cannot delete __annotate__ attribute");
                    return ptr::null_mut();
                }
                if value != &raw mut crate::abi_types::Py_None && PyCallable_Check(value) == 0 {
                    reject_type_layout(c"__annotate__ must be callable or None");
                    return ptr::null_mut();
                }
            }
            // Retain the incoming alias and every displaced namespace value
            // until PyType_Modified has committed, including partial failures.
            let incoming = OwnedPyObject::from_borrowed(value);
            let _retired = [
                c"__annotations__",
                c"__annotations_cache__",
                c"__annotate__",
                c"__annotate_func__",
            ]
            .map(|key| OwnedPyObject::from_borrowed(annotation_entry(dict, key)));
            if descriptors::pending() {
                return ptr::null_mut();
            }
            let _invalidate = MetadataMutation(tp);
            if field == Field::Annotate && deferred {
                if mapping::PyDict_SetItemString(
                    dict,
                    c"__annotate_func__".as_ptr(),
                    incoming.as_ptr(),
                ) < 0
                {
                    return ptr::null_mut();
                }
                if value != &raw mut crate::abi_types::Py_None
                    && delete_annotation_entry(dict, c"__annotations_cache__") < 0
                {
                    return ptr::null_mut();
                }
            } else {
                let explicit = annotation_entry(dict, name);
                if descriptors::pending() {
                    return ptr::null_mut();
                }
                let key = if field == Field::Annotations && deferred && explicit.is_null() {
                    c"__annotations_cache__"
                } else {
                    name
                };
                if delete {
                    match delete_annotation_entry(dict, key) {
                        -1 => return ptr::null_mut(),
                        0 => return missing_annotation(field),
                        _ => {}
                    }
                } else if mapping::PyDict_SetItemString(dict, key.as_ptr(), incoming.as_ptr()) < 0 {
                    return ptr::null_mut();
                }
                if field == Field::Annotations && deferred {
                    if !explicit.is_null()
                        && delete_annotation_entry(dict, c"__annotations_cache__") < 0
                    {
                        return ptr::null_mut();
                    }
                    if delete_annotation_entry(dict, c"__annotate_func__") < 0
                        || delete_annotation_entry(dict, c"__annotate__") < 0
                    {
                        return ptr::null_mut();
                    }
                }
            }
            return object::Py_NewRef(&raw mut crate::abi_types::Py_None);
        }
        let cache_key = match field {
            Field::Annotate if deferred => c"__annotate_func__",
            Field::Annotations if deferred => c"__annotations_cache__",
            _ => name,
        };
        let mut existing = annotation_entry(dict, name);
        if descriptors::pending() {
            return ptr::null_mut();
        }
        if existing.is_null() && deferred {
            existing = annotation_entry(dict, cache_key);
        }
        if descriptors::pending() {
            return ptr::null_mut();
        }
        if !existing.is_null() {
            let existing = OwnedPyObject::from_borrowed(existing);
            // Annotations and evaluator descriptors bind with a NULL receiver
            // and the actual class owner. The borrowed entry stays owned.
            return crate::api::descriptor::get(existing.as_ptr(), ptr::null_mut(), tp.cast())
                .unwrap_or_else(|| existing.into_ptr());
        }
        if field == Field::Annotate {
            if !deferred {
                return missing_annotation(field);
            }
            if mapping::PyDict_SetItemString(
                dict,
                cache_key.as_ptr(),
                &raw mut crate::abi_types::Py_None,
            ) < 0
            {
                return ptr::null_mut();
            }
            PyType_Modified(tp);
            return object::Py_NewRef(&raw mut crate::abi_types::Py_None);
        }
        let result = if deferred {
            // Ordinary lookup honors metaclass overrides and returns an owning
            // reference across callbacks that remove their namespace edge.
            let annotate = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                tp.cast(),
                c"__annotate__".as_ptr(),
            ));
            if annotate.as_ptr().is_null() {
                return ptr::null_mut();
            }
            if PyCallable_Check(annotate.as_ptr()) != 0 {
                let format = OwnedPyObject::from_owned(crate::api::numbers::PyLong_FromLong(1));
                if format.as_ptr().is_null() {
                    return ptr::null_mut();
                }
                object::PyObject_CallOneArg(annotate.as_ptr(), format.as_ptr())
            } else {
                mapping::PyDict_New()
            }
        } else {
            mapping::PyDict_New()
        };
        let result = OwnedPyObject::from_owned(result);
        if result.as_ptr().is_null() {
            return ptr::null_mut();
        }
        if mapping::PyDict_Check(result.as_ptr()) == 0 {
            reject_type_layout(c"__annotate__ returned non-dict");
            return ptr::null_mut();
        }
        // A callback may already have filled the cache. Retain that displaced
        // value until invalidation so reentrant finalizers observe the commit.
        let _retired = OwnedPyObject::from_borrowed(annotation_entry(dict, cache_key));
        if descriptors::pending() {
            return ptr::null_mut();
        }
        let _invalidate = MetadataMutation(tp);
        if mapping::PyDict_SetItemString(dict, cache_key.as_ptr(), result.as_ptr()) < 0 {
            return ptr::null_mut();
        }
        result.into_ptr()
    }
}
