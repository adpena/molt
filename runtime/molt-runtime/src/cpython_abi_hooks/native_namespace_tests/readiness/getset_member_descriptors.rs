//! Native member/getset readiness, public construction and binding use the
//! real runtime's namespace, class identity, text and reference owners.
//! Extension declarations and native payloads retain their physical C layout.

use super::*;
use molt_cpython_abi::api::object;
use molt_obj_model::MoltObject;
use std::os::raw::c_long;

/// A struct shaped like a numpy descriptor object: a `PyObject` header followed
/// by members that a `tp_members` table addresses by offset.
#[repr(C)]
struct DescriptorPayload {
    ob_base: PyObject,
    // T_OBJECT member (like arraydescr `typeobj`).
    type_obj: *mut PyObject,
    // T_CHAR member (like arraydescr `kind`).
    kind: c_char,
    // T_INT member (like arraydescr `type_num`).
    type_num: c_int,
    // T_PYSSIZET member (like arraydescr `elsize`).
    elsize: Py_ssize_t,
}

fn member(name: &'static [u8], type_: c_int, offset: usize, flags: c_int) -> PyMemberDef {
    assert_eq!(
        *name.last().unwrap(),
        0,
        "member name must be NUL-terminated"
    );
    PyMemberDef {
        name: name.as_ptr() as *const c_char,
        type_,
        offset: offset as Py_ssize_t,
        flags,
        doc: ptr::null(),
    }
}

fn member_sentinel() -> PyMemberDef {
    PyMemberDef {
        name: ptr::null(),
        type_: 0,
        offset: 0,
        flags: 0,
        doc: ptr::null(),
    }
}

// Member type codes (Py_T_*).
const T_INT: c_int = 1;
const T_OBJECT: c_int = 6;
const T_CHAR: c_int = 7;
const T_PYSSIZET: c_int = 19;
const READONLY: c_int = 1;

// ---------------------------------------------------------------------------
// A getter/setter pair for a getset table (like arraydescr `names`).
// ---------------------------------------------------------------------------

// The getter returns a fixed sentinel int so the test can observe it was
// actually invoked through the descriptor protocol.
unsafe extern "C" fn declared_getter(
    _self: *mut PyObject,
    closure: *mut std::ffi::c_void,
) -> *mut PyObject {
    let value = if closure.is_null() {
        1234
    } else {
        unsafe { *(closure.cast::<c_int>()) }
    };
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(value as c_long) }
}

fn getset_def(name: &'static [u8], get: Option<getter>, set: Option<setter>) -> PyGetSetDef {
    assert_eq!(
        *name.last().unwrap(),
        0,
        "getset name must be NUL-terminated"
    );
    PyGetSetDef {
        name: name.as_ptr() as *const c_char,
        get,
        set,
        doc: ptr::null(),
        closure: ptr::null_mut(),
    }
}

fn getset_sentinel() -> PyGetSetDef {
    PyGetSetDef {
        name: ptr::null(),
        get: None,
        set: None,
        doc: ptr::null(),
        closure: ptr::null_mut(),
    }
}

// ===========================================================================
// (1) PyType_Ready populates tp_dict with real member/getset descriptors.
// ===========================================================================

#[test]
fn ready_populates_tp_dict_from_members_and_getset() {
    let _transaction = init();

    let mut members = [
        member(
            b"type\0",
            T_OBJECT,
            std::mem::offset_of!(DescriptorPayload, type_obj),
            READONLY,
        ),
        member(
            b"kind\0",
            T_CHAR,
            std::mem::offset_of!(DescriptorPayload, kind),
            READONLY,
        ),
        member(
            b"num\0",
            T_INT,
            std::mem::offset_of!(DescriptorPayload, type_num),
            READONLY,
        ),
        member(
            b"itemsize\0",
            T_PYSSIZET,
            std::mem::offset_of!(DescriptorPayload, elsize),
            READONLY,
        ),
        member_sentinel(),
    ];
    let mut getter_value: c_int = 1234;
    let mut getsets = [
        PyGetSetDef {
            name: c"names".as_ptr(),
            get: Some(declared_getter),
            set: None,
            doc: ptr::null(),
            closure: (&mut getter_value as *mut c_int).cast(),
        },
        getset_sentinel(),
    ];

    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.ob_base.ob_base.ob_refcnt = 1;
    tp.tp_name = c"numpy.dtype_shaped".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as Py_ssize_t;
    tp.tp_members = members.as_mut_ptr();
    tp.tp_getset = getsets.as_mut_ptr();

    let rc = unsafe { ready(&mut *tp) };
    // The real namespace must retain every descriptor after publication.
    assert_eq!(
        rc, 0,
        "PyType_Ready must succeed for a type carrying tp_members + tp_getset"
    );
    assert!(!tp.tp_dict.is_null(), "tp_dict must be created");
    assert_ne!(
        tp.tp_flags & Py_TPFLAGS_READY,
        0,
        "the type must be marked READY after member/getset population"
    );

    let num_key = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"num".as_ptr()) };
    let num_descr = unsafe { molt_cpython_abi::api::typeobj::_PyType_Lookup(&mut *tp, num_key) };
    assert!(
        !num_descr.is_null(),
        "tp_members entry must be visible in tp_dict"
    );
    unsafe {
        assert_eq!((*num_descr).ob_type, &raw mut PyMemberDescr_Type);
    }
    assert!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(num_descr)
            .is_none(),
        "a physical descriptor must not masquerade as a managed ABI view"
    );
    let foreign_bits =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(num_descr) }
            .expect("member_descriptor must retain first-class foreign custody");
    assert!(MoltObject::from_bits(foreign_bits).is_ptr());
    unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().dec_ref)(foreign_bits) };

    let names_key =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"names".as_ptr()) };
    let names_descr =
        unsafe { molt_cpython_abi::api::typeobj::_PyType_Lookup(&mut *tp, names_key) };
    assert!(
        !names_descr.is_null(),
        "tp_getset entry must be visible in tp_dict"
    );
    unsafe {
        assert_eq!((*names_descr).ob_type, &raw mut PyGetSetDescr_Type);
    }

    let mut inst = DescriptorPayload {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &mut *tp,
        },
        type_obj: ptr::null_mut(),
        kind: b'i' as c_char,
        type_num: 7,
        elsize: 8,
    };
    let inst_ptr = (&mut inst as *mut DescriptorPayload).cast::<PyObject>();
    let num_value =
        unsafe { molt_cpython_abi::api::object::PyObject_GenericGetAttr(inst_ptr, num_key) };
    assert!(
        !num_value.is_null(),
        "member descriptor must bind through GenericGetAttr"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(num_value) },
        7
    );

    let inst_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let shadow_value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(99) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(inst_dict, num_key, shadow_value) },
        0
    );
    let shadowed = unsafe {
        molt_cpython_abi::api::object::_PyObject_GenericGetAttrWithDict(
            inst_ptr, num_key, inst_dict, 0,
        )
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(shadowed) },
        7,
        "member_descriptor is a data descriptor and must beat instance dict values"
    );

    let names_value =
        unsafe { molt_cpython_abi::api::object::PyObject_GenericGetAttr(inst_ptr, names_key) };
    assert!(
        !names_value.is_null(),
        "getset descriptor must bind through GenericGetAttr"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(names_value) },
        1234
    );
    unsafe {
        for object in [
            names_value,
            shadowed,
            shadow_value,
            inst_dict,
            num_value,
            names_key,
            num_key,
        ] {
            molt_cpython_abi::api::refcount::Py_DECREF(object);
        }
    }
}

// ===========================================================================
// (2) PyDescr_NewMember / PyDescr_NewGetSet mint real, correctly-typed objects.
// ===========================================================================

#[test]
fn new_member_descriptor_has_correct_type_and_name() {
    let _transaction = init();
    let mut memb = member(
        b"num\0",
        T_INT,
        std::mem::offset_of!(DescriptorPayload, type_num),
        READONLY,
    );
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut *tp, &mut memb) };
    assert!(
        !descr.is_null(),
        "PyDescr_NewMember must return a real object"
    );
    let none = &raw mut Py_None;
    assert!(
        !std::ptr::eq(descr, none),
        "PyDescr_NewMember must not return the Py_None stub"
    );
    unsafe {
        assert_eq!(
            (*descr).ob_type,
            &raw mut PyMemberDescr_Type,
            "member descriptor must have type member_descriptor"
        );
        // The interned name must be readable via PyDescr_NAME.
        let name = molt_cpython_abi::api::typeobj::PyDescr_NAME(descr);
        assert!(!name.is_null(), "descriptor must carry its interned name");
    }
    // A member descriptor is a data descriptor (its type has tp_descr_set).
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_IsData(descr) },
        1,
        "member_descriptor must report as a data descriptor"
    );
    unsafe { refcount::Py_DECREF(descr) };
}

#[test]
fn new_getset_descriptor_has_correct_type() {
    let _transaction = init();
    let mut gs = getset_def(b"names\0", Some(declared_getter), None);
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut *tp, &mut gs) };
    assert!(!descr.is_null());
    unsafe {
        assert_eq!(
            (*descr).ob_type,
            &raw mut PyGetSetDescr_Type,
            "getset descriptor must have type getset_descriptor"
        );
        refcount::Py_DECREF(descr);
    }
}

// ===========================================================================
// (3) The descriptor protocol reads real values.
// ===========================================================================

#[test]
fn member_descriptor_get_reads_struct_field() {
    let _transaction = init();
    // Build an instance with a known type_num, then read it back through the
    // member_descriptor's tp_descr_get.
    let mut inst = DescriptorPayload {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        },
        type_obj: ptr::null_mut(),
        kind: b'i' as c_char,
        type_num: 7,
        elsize: 8,
    };
    let mut memb = member(
        b"num\0",
        T_INT,
        std::mem::offset_of!(DescriptorPayload, type_num),
        READONLY,
    );
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);

    inst.ob_base.ob_type = &mut *tp;
    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut *tp, &mut memb) };
    assert!(!descr.is_null());

    let descr_type = unsafe { (*descr).ob_type };
    let get =
        unsafe { (*descr_type).tp_descr_get }.expect("member_descriptor must wire tp_descr_get");
    let value = unsafe {
        get(
            descr,
            (&mut inst as *mut DescriptorPayload).cast::<PyObject>(),
            (&mut *tp as *mut PyTypeObject).cast::<PyObject>(),
        )
    };
    assert!(
        !value.is_null(),
        "reading a T_INT member must yield an int object"
    );
    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) };
    assert_eq!(
        got, 7,
        "member_descriptor.__get__ must read the field value"
    );
    unsafe {
        refcount::Py_DECREF(value);
        refcount::Py_DECREF(descr);
    }
}

#[test]
fn getset_descriptor_get_invokes_getter() {
    let _transaction = init();
    let mut gs = getset_def(b"names\0", Some(declared_getter), None);
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);
    let mut inst = PyObject {
        ob_refcnt: 1,
        ob_type: &mut *tp,
    };

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut *tp, &mut gs) };
    assert!(!descr.is_null());
    let descr_type = unsafe { (*descr).ob_type };
    let get =
        unsafe { (*descr_type).tp_descr_get }.expect("getset_descriptor must wire tp_descr_get");
    let value = unsafe {
        get(
            descr,
            &mut inst as *mut PyObject,
            (&mut *tp as *mut PyTypeObject).cast::<PyObject>(),
        )
    };
    assert!(!value.is_null(), "getset __get__ must invoke the getter");
    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) };
    assert_eq!(
        got, 1234,
        "getset_descriptor.__get__ must call the underlying getter"
    );
    unsafe {
        refcount::Py_DECREF(value);
        refcount::Py_DECREF(descr);
    }
}

#[test]
fn readonly_getset_set_raises_attributeerror() {
    let _transaction = init();
    // A getset with no setter is read-only; writing through it must raise
    // AttributeError (never a silent success).
    let mut gs = getset_def(b"names\0", Some(declared_getter), None);
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);
    let mut inst = PyObject {
        ob_refcnt: 1,
        ob_type: &mut *tp,
    };

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut *tp, &mut gs) };
    let descr_type = unsafe { (*descr).ob_type };
    let set =
        unsafe { (*descr_type).tp_descr_set }.expect("getset_descriptor must wire tp_descr_set");
    // Clear any pending error first.
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let rc = unsafe { set(descr, &mut inst as *mut PyObject, val) };
    assert_eq!(rc, -1, "writing a read-only getset must fail");
    let pending = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        !pending.is_null(),
        "a read-only getset write must leave a pending exception (never a contentless failure)"
    );
    assert_eq!(pending, (&raw mut PyExc_AttributeError).cast());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        refcount::Py_DECREF(val);
        refcount::Py_DECREF(descr);
    }
}

// ===========================================================================
// (4) NULL definitions fail with an exact, normalized SystemError.
// ===========================================================================

#[test]
fn new_descriptor_with_null_def_raises_system_error() {
    let _transaction = init();
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    tp.tp_base = &raw mut PyBaseObject_Type;
    tp.tp_name = c"owner".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<DescriptorPayload>() as isize;
    assert_eq!(unsafe { ready(&mut *tp) }, 0);

    let getset_rc =
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut *tp, ptr::null_mut()) };
    assert!(
        getset_rc.is_null(),
        "PyDescr_NewGetSet(NULL) must return NULL, not a Py_None stub"
    );
    unsafe { assert_descriptor_system_error() };
    let member_rc =
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut *tp, ptr::null_mut()) };
    assert!(
        member_rc.is_null(),
        "PyDescr_NewMember(NULL) must return NULL"
    );
    unsafe { assert_descriptor_system_error() };
}

unsafe fn assert_descriptor_system_error() {
    use molt_cpython_abi::api::errors;
    unsafe {
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        let raised = errors::PyErr_GetRaisedException();
        let raised_owner = refcount::OwnedPyObject::from_owned(raised);
        assert!(
            !raised.is_null(),
            "invalid descriptors carry a normalized exception"
        );
        assert_eq!((*raised).ob_type, &raw mut PyExc_SystemError);
        drop(raised_owner);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn instance_dictionary_descriptor_requires_an_explicit_declaration() {
    use molt_cpython_abi::api::{errors, object, refcount::OwnedPyObject};
    let _transaction = init();
    let mut declarations = [
        getset_def(
            b"__dict__\0",
            Some(object::PyObject_GenericGetDict),
            Some(object::PyObject_GenericSetDict),
        ),
        getset_sentinel(),
    ];
    for declared in [false, true] {
        let mut class =
            NativeType::subtype(&raw mut PyBaseObject_Type, c"annotation.NativeDictionary");
        class.tp_basicsize =
            (std::mem::size_of::<PyObject>() + std::mem::size_of::<*mut PyObject>()) as Py_ssize_t;
        class.tp_dictoffset = std::mem::size_of::<PyObject>() as Py_ssize_t;
        if declared {
            class.tp_getset = declarations.as_mut_ptr();
        }
        unsafe {
            assert_eq!(class.ready(), 0);
            assert_eq!(
                !mapping::_PyDict_GetItemStringWithError(class.tp_dict, c"__dict__".as_ptr())
                    .is_null(),
                declared
            );
            let instance = OwnedPyObject::from_owned(typeobj::PyType_GenericNew(
                &raw mut *class,
                ptr::null_mut(),
                ptr::null_mut(),
            ));
            assert!(!instance.as_ptr().is_null());
            let public = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                instance.as_ptr(),
                c"__dict__".as_ptr(),
            ));
            if declared {
                assert!(!public.as_ptr().is_null());
                assert_ne!(mapping::PyDict_Check(public.as_ptr()), 0);
            } else {
                assert!(public.as_ptr().is_null());
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                    0
                );
                errors::PyErr_Clear();
                // Offset-backed attribute storage remains valid without a
                // Python-visible dictionary declaration.
                let physical = OwnedPyObject::from_owned(object::PyObject_GenericGetDict(
                    instance.as_ptr(),
                    ptr::null_mut(),
                ));
                assert!(!physical.as_ptr().is_null());
            }
        }
    }
}

// Each descriptor owns only its observation state, never itself or its owner.
// The test retains the state after deallocation without masking descriptor loss.
struct AnnotationDescriptorState {
    owner: *mut PyObject,
    dictionary: *mut PyObject,
    key: &'static std::ffi::CStr,
    result: *mut PyObject,
    failure: *mut PyObject,
    replace: bool,
    calls: std::cell::Cell<usize>,
    active: std::cell::Cell<bool>,
    drops: std::cell::Cell<usize>,
    dropped_in_callback: std::cell::Cell<bool>,
}

#[repr(C)]
struct AnnotationDescriptorPayload {
    ob_base: PyObject,
    state: *const AnnotationDescriptorState,
}

unsafe extern "C" fn annotation_descriptor_drop(descriptor: *mut PyObject) {
    unsafe {
        let state =
            std::rc::Rc::from_raw((*descriptor.cast::<AnnotationDescriptorPayload>()).state);
        state.drops.set(state.drops.get() + 1);
        state.dropped_in_callback.set(state.active.get());
        molt_cpython_abi::api::memory::PyObject_Free(descriptor.cast());
        if state.replace && !state.failure.is_null() {
            // Releasing the callback's last owned edge must retain its exact
            // pending exception even when native destruction raises another.
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast(),
                c"annotation descriptor cleanup sentinel".as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn annotation_descriptor_get(
    descriptor: *mut PyObject,
    instance: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let state = &*(*descriptor.cast::<AnnotationDescriptorPayload>()).state;
        if !instance.is_null() || owner != state.owner {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_AssertionError).cast(),
                c"annotation descriptor receiver/owner mismatch".as_ptr(),
            );
            return ptr::null_mut();
        }
        state.calls.set(state.calls.get() + 1);
        state.active.set(true);
        if state.replace {
            let status =
                mapping::PyDict_SetItemString(state.dictionary, state.key.as_ptr(), state.result);
            typeobj::PyType_Modified(owner.cast());
            if status < 0 {
                state.active.set(false);
                return ptr::null_mut();
            }
        }
        state.active.set(false);
        if !state.failure.is_null() {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_ValueError).cast(),
                state.failure,
            );
            ptr::null_mut()
        } else {
            object::Py_NewRef(state.result)
        }
    }
}

#[test]
fn native_annotation_reads_bind_owned_descriptors_and_reject_static_types() {
    use molt_cpython_abi::api::typeobj::TypeAttributeField as Field;
    use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, sequences};
    use std::cell::Cell;
    use std::rc::Rc;
    let _transaction = init();
    let mut descriptor_type =
        NativeType::subtype(&raw mut PyBaseObject_Type, c"annotation.Descriptor");
    descriptor_type.tp_basicsize = std::mem::size_of::<AnnotationDescriptorPayload>() as Py_ssize_t;
    descriptor_type.tp_descr_get = Some(annotation_descriptor_get);
    descriptor_type.tp_dealloc = Some(annotation_descriptor_drop);
    let mut static_class = NativeType::subtype(&raw mut PyBaseObject_Type, c"annotation.Static");
    static_class.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    unsafe {
        assert_eq!(descriptor_type.ready(), 0);
        assert_eq!(static_class.ready(), 0);
        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"annotation.Heap".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as c_int,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let heap = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&raw mut spec));
        assert!(!heap.as_ptr().is_null());
        let dictionary_owner =
            OwnedPyObject::from_owned(typeobj::PyType_GetDict(heap.as_ptr().cast()));
        let dictionary = dictionary_owner.as_ptr();
        assert!(!dictionary.is_null());
        let result = OwnedPyObject::from_owned(sequences::PyList_New(0));
        let failure = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(
            (&raw mut PyExc_ValueError).cast(),
        ));
        let ordinary = OwnedPyObject::from_owned(mapping::PyDict_New());
        assert!(!result.as_ptr().is_null());
        assert!(!failure.as_ptr().is_null());
        assert!(!ordinary.as_ptr().is_null());
        for (field, key, deferred) in [
            (Field::Annotations, c"__annotations__", false),
            (Field::Annotations, c"__annotations__", true),
            (Field::Annotate, c"__annotate__", true),
        ] {
            // A non-descriptor returns the exact ordinary dictionary each time.
            assert_eq!(
                mapping::PyDict_SetItemString(dictionary, key.as_ptr(), ordinary.as_ptr()),
                0
            );
            for _ in 0..2 {
                let value = OwnedPyObject::from_owned(typeobj::native_type_attribute_get(
                    heap.as_ptr(),
                    field,
                    deferred,
                ));
                assert_eq!(value.as_ptr(), ordinary.as_ptr());
                assert!(errors::PyErr_Occurred().is_null());
            }
            assert_eq!(mapping::PyDict_DelItemString(dictionary, key.as_ptr()), 0);

            for (replace, fail) in [(false, false), (false, true), (true, false), (true, true)] {
                let state = Rc::new(AnnotationDescriptorState {
                    owner: heap.as_ptr(),
                    dictionary,
                    key,
                    result: result.as_ptr(),
                    failure: if fail {
                        failure.as_ptr()
                    } else {
                        ptr::null_mut()
                    },
                    replace,
                    calls: Cell::new(0),
                    active: Cell::new(false),
                    drops: Cell::new(0),
                    dropped_in_callback: Cell::new(false),
                });
                let descriptor = OwnedPyObject::from_owned(typeobj::PyType_GenericNew(
                    &raw mut *descriptor_type,
                    ptr::null_mut(),
                    ptr::null_mut(),
                ));
                assert!(!descriptor.as_ptr().is_null());
                (*descriptor.as_ptr().cast::<AnnotationDescriptorPayload>()).state =
                    Rc::into_raw(Rc::clone(&state));
                assert_eq!(
                    mapping::PyDict_SetItemString(dictionary, key.as_ptr(), descriptor.as_ptr()),
                    0
                );
                drop(descriptor); // Only the class namespace owns the descriptor now.
                assert_eq!(state.drops.get(), 0);
                let reads = if replace { 1 } else { 2 };
                for call in 1..=reads {
                    let value = OwnedPyObject::from_owned(typeobj::native_type_attribute_get(
                        heap.as_ptr(),
                        field,
                        deferred,
                    ));
                    if fail {
                        assert!(value.as_ptr().is_null());
                        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(raised.as_ptr(), failure.as_ptr());
                    } else {
                        assert_eq!(value.as_ptr(), result.as_ptr());
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    assert_eq!(state.calls.get(), call);
                    assert!(!state.dropped_in_callback.get());
                }
                if replace {
                    assert_eq!(state.drops.get(), 1);
                    // The mutation remains visible even if the callback raised.
                    let value = OwnedPyObject::from_owned(typeobj::native_type_attribute_get(
                        heap.as_ptr(),
                        field,
                        deferred,
                    ));
                    assert_eq!(value.as_ptr(), result.as_ptr());
                    assert_eq!(state.calls.get(), 1);
                } else {
                    assert_eq!(state.drops.get(), 0);
                    // A static class still rejects an explicit descriptor entry.
                    let borrowed =
                        mapping::_PyDict_GetItemStringWithError(dictionary, key.as_ptr());
                    assert!(!borrowed.is_null());
                    assert_eq!(
                        mapping::PyDict_SetItemString(static_class.tp_dict, key.as_ptr(), borrowed),
                        0
                    );
                    assert!(
                        typeobj::native_type_attribute_get(
                            (&raw mut *static_class).cast(),
                            field,
                            deferred,
                        )
                        .is_null()
                    );
                    assert_ne!(
                        errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                        0
                    );
                    errors::PyErr_Clear();
                    assert_eq!(state.calls.get(), reads);
                    assert_eq!(
                        mapping::PyDict_DelItemString(static_class.tp_dict, key.as_ptr()),
                        0
                    );
                }
                assert_eq!(mapping::PyDict_DelItemString(dictionary, key.as_ptr()), 0);
                assert_eq!(state.drops.get(), 1);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    }
}
