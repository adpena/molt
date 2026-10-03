//! Physical descriptor storage, receiver admission and public metadata.
//! Every owned header edge participates in the runtime's mixed collector.

use crate::abi_types::*;
pub(super) use crate::api::errors::raised_error_pending as pending;
use crate::api::{errors, memory, object, refcount, strings};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

pub(super) struct Pins<const N: usize> {
    _owners: [refcount::OwnedPyObject; N],
}

impl<const N: usize> Pins<N> {
    pub(super) unsafe fn new(objects: [*mut PyObject; N]) -> Self {
        Self {
            _owners: objects
                .map(|object| unsafe { refcount::OwnedPyObject::from_borrowed(object) }),
        }
    }
}

pub(super) unsafe fn system_error(message: &CStr) {
    if !pending() {
        unsafe { errors::PyErr_SetString((&raw mut PyExc_SystemError).cast(), message.as_ptr()) };
    }
}

pub(super) unsafe fn type_error(message: &str) -> *mut PyObject {
    let message = std::ffi::CString::new(message.replace('\0', "\\0"))
        .expect("escaped descriptor diagnostic contains no NUL");
    unsafe { errors::PyErr_SetString((&raw mut PyExc_TypeError).cast(), message.as_ptr()) };
    ptr::null_mut()
}

pub(super) unsafe fn checked_result(result: *mut PyObject) -> *mut PyObject {
    unsafe { errors::check_native_result(result, "native descriptor callback") }
}

pub(super) unsafe fn checked_status(status: c_int) -> c_int {
    unsafe { errors::check_native_status(status, "native descriptor callback") }
}

pub(super) unsafe fn new_reference(value: *mut PyObject) -> *mut PyObject {
    if value.is_null() {
        unsafe { system_error(c"descriptor metadata has been cleared") };
    } else {
        unsafe { refcount::Py_INCREF(value) };
    }
    value
}

pub(super) unsafe fn allocate(
    kind: *mut PyTypeObject,
    owner: *mut PyTypeObject,
    name: *const c_char,
) -> *mut PyDescrObject {
    if owner.is_null() || name.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let _owner = unsafe { refcount::OwnedPyObject::from_borrowed(owner.cast()) };
    let name_object = unsafe { strings::PyUnicode_InternFromString(name) };
    if name_object.is_null() {
        return ptr::null_mut();
    }
    let descr = unsafe { memory::_PyObject_GC_New(kind) }.cast::<PyDescrObject>();
    if descr.is_null() {
        unsafe {
            system_error(c"native descriptor allocation failed without an exception");
            errors::release_preserving_error(&[name_object]);
        }
        return ptr::null_mut();
    }
    unsafe {
        refcount::Py_INCREF(owner.cast());
        (*descr).d_type = owner;
        (*descr).d_name = name_object;
    }
    descr
}

pub(super) unsafe fn publish(object: *mut PyObject) -> *mut PyObject {
    unsafe { memory::PyObject_GC_Track(object.cast()) };
    if unsafe { memory::PyObject_GC_IsTracked(object) } == 0 || pending() {
        unsafe {
            system_error(c"native descriptor GC publication failed");
            errors::release_preserving_error(&[object]);
        }
        ptr::null_mut()
    } else {
        object
    }
}

pub(super) unsafe fn receiver(descr: *mut PyObject, object: *mut PyObject) -> bool {
    if descr.is_null() || object.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return false;
    }
    let header = descr.cast::<PyDescrObject>();
    let owner = unsafe { (*header).d_type };
    if owner.is_null() {
        unsafe { system_error(c"descriptor owner has been cleared") };
        return false;
    }
    if unsafe { super::PyObject_TypeCheck(object, owner) } != 0 {
        return true;
    }
    if pending() {
        return false;
    }
    let name = unsafe { strings::unicode_bytes((*header).d_name) }
        .map(String::from_utf8_lossy)
        .unwrap_or_else(|| "?".into());
    let expected = unsafe { (*owner).tp_name };
    let expected = if expected.is_null() {
        "?".into()
    } else {
        unsafe { CStr::from_ptr(expected) }.to_string_lossy()
    };
    let actual = unsafe { super::object_type_name(object) };
    unsafe {
        type_error(&format!(
            "descriptor '{name}' for '{expected}' objects doesn't apply to a '{actual}' object"
        ))
    };
    false
}

pub(super) unsafe extern "C" fn traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    context: *mut c_void,
) -> c_int {
    if object.is_null() || visit.is_null() {
        return 0;
    }
    let callback: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
        unsafe { std::mem::transmute(visit) };
    let header = object.cast::<PyDescrObject>();
    for edge in unsafe {
        [
            (*header).d_type.cast(),
            (*header).d_name,
            (*header).d_qualname,
        ]
    } {
        if !edge.is_null() {
            let status = unsafe { callback(edge, context) };
            if status != 0 {
                return status;
            }
        }
    }
    0
}

pub(super) unsafe extern "C" fn clear(object: *mut PyObject) -> c_int {
    let header = object.cast::<PyDescrObject>();
    let edges = unsafe {
        [
            std::mem::replace(&mut (*header).d_type, ptr::null_mut()).cast(),
            std::mem::replace(&mut (*header).d_name, ptr::null_mut()),
            std::mem::replace(&mut (*header).d_qualname, ptr::null_mut()),
        ]
    };
    unsafe { errors::release_preserving_error(&edges) };
    0
}

pub(super) unsafe extern "C" fn dealloc(object: *mut PyObject) {
    if object.is_null() {
        return;
    }
    unsafe {
        memory::PyObject_GC_UnTrack(object.cast());
        clear(object);
        memory::PyObject_GC_Del(object.cast());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewGetSet(
    owner: *mut PyTypeObject,
    definition: *mut PyGetSetDef,
) -> *mut PyObject {
    if definition.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let descr = unsafe { allocate(&raw mut PyGetSetDescr_Type, owner, (*definition).name) }
        .cast::<PyGetSetDescrObject>();
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*descr).d_getset = definition;
        publish(descr.cast())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewMember(
    owner: *mut PyTypeObject,
    definition: *mut PyMemberDef,
) -> *mut PyObject {
    if definition.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let descr = unsafe { allocate(&raw mut PyMemberDescr_Type, owner, (*definition).name) }
        .cast::<PyMemberDescrObject>();
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*descr).d_member = definition;
        publish(descr.cast())
    }
}

unsafe extern "C" fn getset_get(
    descr: *mut PyObject,
    object: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    if object.is_null() {
        return unsafe { new_reference(descr) };
    }
    let _pins = unsafe { Pins::new([descr, object]) };
    if !unsafe { receiver(descr, object) } {
        return ptr::null_mut();
    }
    let definition = unsafe { (*descr.cast::<PyGetSetDescrObject>()).d_getset };
    if definition.is_null() {
        unsafe { system_error(c"getset descriptor has no definition") };
        return ptr::null_mut();
    }
    if let Some(get) = unsafe { (*definition).get } {
        return unsafe { checked_result(get(object, (*definition).closure)) };
    }
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_AttributeError).cast(),
            c"unreadable attribute".as_ptr(),
        )
    };
    ptr::null_mut()
}

unsafe extern "C" fn getset_set(
    descr: *mut PyObject,
    object: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    let _pins = unsafe { Pins::new([descr, object, value]) };
    if !unsafe { receiver(descr, object) } {
        return -1;
    }
    let definition = unsafe { (*descr.cast::<PyGetSetDescrObject>()).d_getset };
    if definition.is_null() {
        unsafe { system_error(c"getset descriptor has no definition") };
        return -1;
    }
    if let Some(set) = unsafe { (*definition).set } {
        return unsafe { checked_status(set(object, value, (*definition).closure)) };
    }
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_AttributeError).cast(),
            c"readonly attribute".as_ptr(),
        )
    };
    -1
}

unsafe extern "C" fn member_get(
    descr: *mut PyObject,
    object: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    if object.is_null() {
        return unsafe { new_reference(descr) };
    }
    let _pins = unsafe { Pins::new([descr, object]) };
    if !unsafe { receiver(descr, object) } {
        return ptr::null_mut();
    }
    let definition = unsafe { (*descr.cast::<PyMemberDescrObject>()).d_member };
    if definition.is_null() {
        unsafe { system_error(c"member descriptor has no definition") };
        return ptr::null_mut();
    }
    unsafe { checked_result(super::PyMember_GetOne(object.cast(), definition)) }
}

unsafe extern "C" fn member_set(
    descr: *mut PyObject,
    object: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    let _pins = unsafe { Pins::new([descr, object, value]) };
    if !unsafe { receiver(descr, object) } {
        return -1;
    }
    let definition = unsafe { (*descr.cast::<PyMemberDescrObject>()).d_member };
    if definition.is_null() {
        unsafe { system_error(c"member descriptor has no definition") };
        return -1;
    }
    unsafe { checked_status(super::PyMember_SetOne(object.cast(), definition, value)) }
}

pub(super) unsafe fn header(object: *mut PyObject) -> *mut PyDescrObject {
    if unsafe { (*object).ob_type } == &raw mut _PyMethodWrapper_Type {
        unsafe { (*object.cast::<PyMethodWrapperObject>()).descr.cast() }
    } else {
        object.cast()
    }
}

unsafe extern "C" fn name(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    let header = unsafe { header(object) };
    if header.is_null() {
        unsafe { system_error(c"method wrapper has been cleared") };
        return ptr::null_mut();
    }
    unsafe { new_reference((*header).d_name) }
}

unsafe extern "C" fn objclass(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    let header = unsafe { header(object) };
    if header.is_null() {
        unsafe { system_error(c"method wrapper has been cleared") };
        return ptr::null_mut();
    }
    unsafe { new_reference((*header).d_type.cast()) }
}

pub(super) unsafe extern "C" fn qualname(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    let header = unsafe { header(object) };
    if header.is_null() {
        unsafe { system_error(c"method wrapper has been cleared") };
        return ptr::null_mut();
    }
    if unsafe { (*header).d_qualname.is_null() } {
        let owner = unsafe { (*header).d_type.cast::<PyObject>() };
        let name = unsafe { (*header).d_name };
        if owner.is_null() || name.is_null() {
            unsafe { system_error(c"descriptor metadata has been cleared") };
            return ptr::null_mut();
        }
        let _pins = unsafe { Pins::new([object, header.cast(), owner, name]) };
        let type_name = unsafe { object::PyObject_GetAttrString(owner, c"__qualname__".as_ptr()) };
        if type_name.is_null() {
            return ptr::null_mut();
        }
        let rendered = match (unsafe { strings::unicode_bytes(type_name) }, unsafe {
            strings::unicode_bytes(name)
        }) {
            (Some(owner), Some(name)) => unsafe {
                strings::unicode_from_python_bytes(&[owner, b".", name].concat())
            },
            _ => unsafe {
                type_error("<descriptor>.__objclass__.__qualname__ is not a unicode object")
            },
        };
        unsafe { errors::release_preserving_error(&[type_name]) };
        if rendered.is_null() {
            return ptr::null_mut();
        }
        if unsafe { (*header).d_type.cast::<PyObject>() != owner || (*header).d_name != name } {
            unsafe {
                errors::release_preserving_error(&[rendered]);
                system_error(c"descriptor metadata changed during qualification");
            }
            return ptr::null_mut();
        }
        // A reentrant observer can populate the same cache. Retain the first
        // successfully published result and release our losing candidate.
        if unsafe { (*header).d_qualname.is_null() } {
            unsafe { (*header).d_qualname = rendered };
        } else {
            unsafe { errors::release_preserving_error(&[rendered]) };
        }
    }
    unsafe { new_reference((*header).d_qualname) }
}

unsafe fn internal_doc(object: *mut PyObject) -> (*const c_char, *const c_char) {
    let header = unsafe { header(object) };
    if header.is_null() {
        return (ptr::null(), ptr::null());
    }
    let kind = unsafe { (*header).ob_base.ob_type };
    unsafe {
        if kind == &raw mut PyWrapperDescr_Type {
            let base = (*header.cast::<PyWrapperDescrObject>()).d_base;
            if !base.is_null() {
                return ((*base).name, (*base).doc);
            }
        } else if kind == &raw mut PyMemberDescr_Type {
            let member = (*header.cast::<PyMemberDescrObject>()).d_member;
            if !member.is_null() {
                return (ptr::null(), (*member).doc);
            }
        } else if kind == &raw mut PyGetSetDescr_Type {
            let getset = (*header.cast::<PyGetSetDescrObject>()).d_getset;
            if !getset.is_null() {
                return (ptr::null(), (*getset).doc);
            }
        } else if kind == &raw mut PyMethodDescr_Type || kind == &raw mut PyClassMethodDescr_Type {
            let method = (*header.cast::<PyMethodDescrObject>()).d_method;
            if !method.is_null() {
                return ((*method).ml_name, (*method).ml_doc);
            }
        }
    }
    (ptr::null(), ptr::null())
}

unsafe fn doc_part(object: *mut PyObject, signature: bool) -> *mut PyObject {
    let (name, doc) = unsafe { internal_doc(object) };
    unsafe { documentation_part(name, doc, signature) }
}

pub(super) unsafe fn documentation_part(
    name: *const c_char,
    doc: *const c_char,
    signature: bool,
) -> *mut PyObject {
    if doc.is_null() {
        return unsafe { new_reference(&raw mut Py_None) };
    }
    let bytes = unsafe { CStr::from_ptr(doc).to_bytes() };
    let prefix = if name.is_null() {
        &[][..]
    } else {
        unsafe { CStr::from_ptr(name).to_bytes() }
    };
    let split = bytes
        .windows(5)
        .position(|part| part == b"\n--\n\n")
        .filter(|_| {
            !prefix.is_empty()
                && bytes.starts_with(prefix)
                && bytes.get(prefix.len()) == Some(&b'(')
        });
    let part = if signature {
        let Some(end) = split else {
            return unsafe { new_reference(&raw mut Py_None) };
        };
        &bytes[prefix.len()..end]
    } else {
        split.map_or(bytes, |end| &bytes[end + 5..])
    };
    unsafe { strings::PyUnicode_FromStringAndSize(part.as_ptr().cast(), part.len() as Py_ssize_t) }
}

unsafe extern "C" fn doc(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    unsafe { doc_part(object, false) }
}
unsafe extern "C" fn signature(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    unsafe { doc_part(object, true) }
}
type Getter = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> *mut PyObject;
const fn field(name: &CStr, get: Getter) -> PyGetSetDef {
    PyGetSetDef {
        name: name.as_ptr(),
        get: Some(get),
        set: None,
        doc: ptr::null(),
        closure: ptr::null_mut(),
    }
}
const END: PyGetSetDef = PyGetSetDef {
    name: ptr::null(),
    get: None,
    set: None,
    doc: ptr::null(),
    closure: ptr::null_mut(),
};
static COMMON_METADATA: [PyGetSetDef; 3] = [
    field(c"__qualname__", qualname),
    field(c"__doc__", doc),
    END,
];
static CALLABLE_METADATA: [PyGetSetDef; 4] = [
    field(c"__qualname__", qualname),
    field(c"__doc__", doc),
    field(c"__text_signature__", signature),
    END,
];
static METHOD_METADATA: [PyGetSetDef; 6] = [
    field(c"__objclass__", objclass),
    field(c"__name__", name),
    field(c"__qualname__", qualname),
    field(c"__doc__", doc),
    field(c"__text_signature__", signature),
    END,
];
const MEMBER_END: PyMemberDef = PyMemberDef {
    name: ptr::null(),
    type_: 0,
    offset: 0,
    flags: 0,
    doc: ptr::null(),
};
const fn object_member(name: &CStr, offset: usize) -> PyMemberDef {
    PyMemberDef {
        name: name.as_ptr(),
        type_: super::PY_T_OBJECT,
        offset: offset as Py_ssize_t,
        flags: super::PY_READONLY,
        doc: ptr::null(),
    }
}
static COMMON_MEMBERS: [PyMemberDef; 3] = [
    object_member(c"__objclass__", std::mem::offset_of!(PyDescrObject, d_type)),
    object_member(c"__name__", std::mem::offset_of!(PyDescrObject, d_name)),
    MEMBER_END,
];
static METHOD_MEMBERS: [PyMemberDef; 2] = [
    object_member(
        c"__self__",
        std::mem::offset_of!(PyMethodWrapperObject, self_),
    ),
    MEMBER_END,
];

pub(super) unsafe extern "C" fn repr(object: *mut PyObject) -> *mut PyObject {
    let header = unsafe { header(object) };
    if header.is_null() {
        unsafe { system_error(c"method wrapper has been cleared") };
        return ptr::null_mut();
    }
    let name = unsafe { strings::unicode_bytes((*header).d_name) }
        .unwrap_or(b"?")
        .to_vec();
    let owner = unsafe { (*header).d_type };
    if owner.is_null() {
        unsafe { system_error(c"descriptor owner has been cleared") };
        return ptr::null_mut();
    }
    let owner_name = unsafe { (*owner).tp_name };
    let owner_name = if owner_name.is_null() {
        &b"?"[..]
    } else {
        unsafe { CStr::from_ptr(owner_name).to_bytes() }
    };
    let kind = unsafe { (*object).ob_type };
    let prefix = if kind == &raw mut PyWrapperDescr_Type {
        b"<slot wrapper '".as_slice()
    } else if kind == &raw mut PyMemberDescr_Type {
        b"<member '".as_slice()
    } else if kind == &raw mut PyMethodDescr_Type || kind == &raw mut PyClassMethodDescr_Type {
        b"<method '".as_slice()
    } else {
        b"<attribute '".as_slice()
    };
    unsafe {
        strings::unicode_from_python_bytes(
            &[prefix, &name, b"' of '", owner_name, b"' objects>"].concat(),
        )
    }
}

pub(super) unsafe fn init_header_type(kind: *mut PyTypeObject, size: usize) {
    unsafe {
        (*kind).tp_basicsize = size as Py_ssize_t;
        (*kind).tp_flags |= Py_TPFLAGS_HAVE_GC;
        (*kind).tp_base = &raw mut PyBaseObject_Type;
        (*kind).tp_getattro = Some(object::PyObject_GenericGetAttr);
        (*kind).tp_getset = COMMON_METADATA.as_ptr().cast_mut();
        (*kind).tp_members = COMMON_MEMBERS.as_ptr().cast_mut();
        (*kind).tp_repr = Some(repr);
        (*kind).tp_traverse = Some(traverse);
        (*kind).tp_clear = Some(clear);
        (*kind).tp_dealloc = Some(dealloc);
        (*kind).tp_free = Some(memory::PyObject_GC_Del);
    }
}

pub(super) unsafe fn init_callable_type(kind: *mut PyTypeObject, size: usize) {
    unsafe {
        init_header_type(kind, size);
        (*kind).tp_getset = CALLABLE_METADATA.as_ptr().cast_mut();
    }
}

pub(super) unsafe fn init() {
    unsafe {
        init_header_type(
            &raw mut PyGetSetDescr_Type,
            std::mem::size_of::<PyGetSetDescrObject>(),
        );
        PyGetSetDescr_Type.tp_descr_get = Some(getset_get);
        PyGetSetDescr_Type.tp_descr_set = Some(getset_set);
        init_header_type(
            &raw mut PyMemberDescr_Type,
            std::mem::size_of::<PyMemberDescrObject>(),
        );
        PyMemberDescr_Type.tp_descr_get = Some(member_get);
        PyMemberDescr_Type.tp_descr_set = Some(member_set);
        init_callable_type(
            &raw mut PyWrapperDescr_Type,
            std::mem::size_of::<PyWrapperDescrObject>(),
        );
        _PyMethodWrapper_Type.tp_getset = METHOD_METADATA.as_ptr().cast_mut();
        _PyMethodWrapper_Type.tp_members = METHOD_MEMBERS.as_ptr().cast_mut();
    }
}
