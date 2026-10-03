//! Ordinary extension types inherit object attribute dispatch before publication.
use super::*;
use molt_cpython_abi::api::{errors, object, refcount, strings};

#[repr(C)]
struct Receiver {
    object: PyObject,
    reads: usize,
    writes: usize,
    deletes: usize,
    last_value: *mut PyObject,
}

unsafe extern "C" fn get_field(object: *mut PyObject, _closure: *mut c_void) -> *mut PyObject {
    unsafe {
        (*object.cast::<Receiver>()).reads += 1;
        refcount::Py_INCREF(object);
    }
    object
}

unsafe extern "C" fn set_field(
    object: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    let receiver = unsafe { &mut *object.cast::<Receiver>() };
    if value.is_null() {
        receiver.deletes += 1;
    } else {
        receiver.writes += 1;
    }
    receiver.last_value = value;
    0
}

#[test]
fn ordinary_readied_child_uses_inherited_object_slots_for_its_descriptors() {
    let _transaction = init();
    let mut definitions = [
        PyGetSetDef {
            name: c"field".as_ptr(),
            get: Some(get_field),
            set: Some(set_field),
            doc: ptr::null(),
            closure: ptr::null_mut(),
        },
        PyGetSetDef {
            name: ptr::null(),
            get: None,
            set: None,
            doc: ptr::null(),
            closure: ptr::null_mut(),
        },
    ];
    let mut kind = NativeType::<PyTypeObject>::new();
    kind.ob_base.ob_base.ob_refcnt = 1;
    kind.tp_name = c"OrdinaryDescriptorChild".as_ptr();
    kind.tp_basicsize = std::mem::size_of::<Receiver>() as Py_ssize_t;
    // The declaration table retains its typed pointer through readiness.
    kind.tp_getset = definitions.as_mut_ptr();
    assert!(kind.tp_getattro.is_none() && kind.tp_setattro.is_none());
    assert_eq!(unsafe { ready(&raw mut *kind) }, 0);
    assert_eq!(kind.tp_base, &raw mut PyBaseObject_Type);
    assert!(kind.tp_getattro.is_some() && kind.tp_setattro.is_some());

    let mut receiver = Receiver {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *kind,
        },
        reads: 0,
        writes: 0,
        deletes: 0,
        last_value: ptr::null_mut(),
    };
    let receiver_ptr = &raw mut receiver.object;
    let name = unsafe { strings::PyUnicode_FromString(c"field".as_ptr()) };
    assert!(!name.is_null());
    for c_string in [false, true] {
        let result = if c_string {
            unsafe { object::PyObject_GetAttrString(receiver_ptr, c"field".as_ptr()) }
        } else {
            unsafe { object::PyObject_GetAttr(receiver_ptr, name) }
        };
        assert_eq!(
            result, receiver_ptr,
            "readiness must expose the declared getter"
        );
        unsafe {
            refcount::Py_DECREF(result);
        }
        let assigned = if c_string {
            unsafe { object::PyObject_SetAttrString(receiver_ptr, c"field".as_ptr(), receiver_ptr) }
        } else {
            unsafe { object::PyObject_SetAttr(receiver_ptr, name, receiver_ptr) }
        };
        assert_eq!(assigned, 0);
        assert_eq!(receiver.last_value, receiver_ptr);
        let deleted = if c_string {
            unsafe {
                object::PyObject_SetAttrString(receiver_ptr, c"field".as_ptr(), ptr::null_mut())
            }
        } else {
            unsafe { object::PyObject_SetAttr(receiver_ptr, name, ptr::null_mut()) }
        };
        assert_eq!(deleted, 0);
        assert!(receiver.last_value.is_null());
    }
    unsafe {
        refcount::Py_DECREF(name);
    }
    assert_eq!(
        (receiver.reads, receiver.writes, receiver.deletes),
        (2, 2, 2)
    );
    assert_eq!(receiver.object.ob_refcnt, 1);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}
