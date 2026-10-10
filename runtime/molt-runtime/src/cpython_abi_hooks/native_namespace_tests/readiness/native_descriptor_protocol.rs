//! Public constructor/binding oracle. Callback definitions deliberately use
//! ordinary names; physical invocation cannot depend on a dunder classifier.
//! Text, semantic class identity and native GC use the real runtime.

use super::{NativeType, init};
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, memory, numbers, refcount, sequences, typeobj};
use std::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::ptr;

thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }

unsafe extern "C" fn echo(object: *mut PyObject) -> *mut PyObject {
    CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { refcount::Py_INCREF(object) };
    object
}

unsafe extern "C" fn adapter(
    object: *mut PyObject,
    args: *mut PyObject,
    target: *mut c_void,
) -> *mut PyObject {
    assert_eq!(unsafe { sequences::PyTuple_Size(args) }, 0);
    let callback: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(target) };
    unsafe { callback(object) }
}

fn declaration() -> PyWrapperBase {
    PyWrapperBase {
        name: c"plain_callback".as_ptr(),
        offset: 0,
        function: ptr::null_mut(),
        wrapper: Some(adapter),
        doc: c"plain_callback($self, /)\n--\n\nReturn self.".as_ptr(),
        flags: 0,
        name_strobj: ptr::null_mut(),
    }
}

unsafe extern "C" fn visit(edge: *mut PyObject, context: *mut c_void) -> c_int {
    unsafe { (*context.cast::<Vec<usize>>()).push(edge.addr()) };
    0
}

unsafe fn invoke(callable: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    unsafe { ((*(*callable).ob_type).tp_call.unwrap())(callable, args, ptr::null_mut()) }
}

#[test]
fn public_wrapper_constructors_bind_physical_receiver_and_retain_owners() {
    let _thread = init();
    CALLS.with(|calls| calls.set(0));
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    owner.tp_base = &raw mut PyBaseObject_Type;
    owner.tp_name = c"Receiver".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let owner_baseline = owner.ob_base.ob_base.ob_refcnt;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *owner,
    };
    let mut base = declaration();
    let descr = unsafe {
        typeobj::PyDescr_NewWrapper(&raw mut *owner, &raw mut base, echo as *const () as _)
    };
    assert!(!descr.is_null());
    assert_eq!(owner.ob_base.ob_base.ob_refcnt, owner_baseline + 1);
    assert_eq!(
        unsafe { (*descr.cast::<PyWrapperDescrObject>()).d_base },
        &raw mut base
    );
    assert_eq!(unsafe { memory::PyObject_GC_IsTracked(descr) }, 1);
    let get = unsafe { (*(*descr).ob_type).tp_descr_get.unwrap() };
    let unbound = unsafe { get(descr, ptr::null_mut(), (&raw mut *owner).cast()) };
    assert_eq!(unbound, descr);
    unsafe { refcount::Py_DECREF(unbound) };
    let bound = unsafe { get(descr, &raw mut receiver, (&raw mut *owner).cast()) };
    assert!(!bound.is_null());
    assert_eq!(unsafe { (*bound).ob_type }, &raw mut _PyMethodWrapper_Type);
    assert_eq!((receiver.ob_refcnt, unsafe { (*descr).ob_refcnt }), (2, 2));
    let empty = unsafe { sequences::PyTuple_New(0) };
    let result = unsafe { invoke(bound, empty) };
    assert_eq!(result, &raw mut receiver);
    unsafe { refcount::Py_DECREF(result) };
    let positional = [&raw mut receiver];
    let args = unsafe { sequences::PyTuple_FromArray(positional.as_ptr(), 1) };
    let result = unsafe { invoke(descr, args) };
    assert_eq!(result, &raw mut receiver);
    unsafe { refcount::Py_DECREF(result) };
    assert_eq!(CALLS.with(Cell::get), 2);

    let mut wrong = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut PyBaseObject_Type,
    };
    assert!(unsafe { typeobj::PyWrapper_New(descr, &raw mut wrong) }.is_null());
    assert_eq!(CALLS.with(Cell::get), 2);
    assert_eq!(
        unsafe { errors::PyErr_Occurred() },
        (&raw mut PyExc_TypeError).cast()
    );
    unsafe { errors::PyErr_Clear() };

    let header = descr.cast::<PyDescrObject>();
    let mut edges: Vec<usize> = Vec::new();
    unsafe {
        ((*(*descr).ob_type).tp_traverse.unwrap())(
            descr,
            visit as *const () as _,
            (&raw mut edges).cast(),
        )
    };
    assert_eq!(
        edges,
        [(&raw mut *owner).addr(), unsafe { (*header).d_name }.addr()]
    );
    unsafe {
        refcount::Py_DECREF(bound);
        refcount::Py_DECREF(args);
        refcount::Py_DECREF(empty);
        refcount::Py_DECREF(descr);
    }
    assert_eq!(
        (owner.ob_base.ob_base.ob_refcnt, receiver.ob_refcnt),
        (owner_baseline, 1)
    );
    assert!(!crate::object::gc::native_gc_is_enrolled(descr.addr()));
    assert!(!crate::object::gc::native_gc_is_enrolled(bound.addr()));
}

#[repr(C)]
struct Payload {
    header: PyObject,
    value: c_int,
}

unsafe extern "C" fn getter(object: *mut PyObject, _: *mut c_void) -> *mut PyObject {
    CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { numbers::PyLong_FromLong(c_long::from((*object.cast::<Payload>()).value)) }
}
unsafe extern "C" fn setter(_: *mut PyObject, value: *mut PyObject, _: *mut c_void) -> c_int {
    CALLS.with(|calls| calls.set(calls.get() + 1));
    assert!(
        value.is_null(),
        "C deletion must retain NULL rather than Py_None"
    );
    0
}

#[test]
fn getset_and_member_share_receiver_admission_and_gc_header_ownership() {
    let _thread = init();
    CALLS.with(|calls| calls.set(0));
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    owner.tp_base = &raw mut PyBaseObject_Type;
    owner.tp_name = c"Payload".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<Payload>() as isize;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let owner_baseline = owner.ob_base.ob_base.ob_refcnt;
    let mut receiver = Payload {
        header: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *owner,
        },
        value: 42,
    };
    let mut wrong = Payload {
        header: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyBaseObject_Type,
        },
        value: 99,
    };
    let mut getset = PyGetSetDef {
        name: c"value".as_ptr(),
        get: Some(getter),
        set: Some(setter),
        doc: ptr::null(),
        closure: ptr::null_mut(),
    };
    let mut member = PyMemberDef {
        name: c"value".as_ptr(),
        type_: 1,
        offset: std::mem::offset_of!(Payload, value) as isize,
        flags: 0,
        doc: ptr::null(),
    };
    let descriptors = unsafe {
        [
            typeobj::PyDescr_NewGetSet(&raw mut *owner, &raw mut getset),
            typeobj::PyDescr_NewMember(&raw mut *owner, &raw mut member),
        ]
    };
    for descr in descriptors {
        assert!(!descr.is_null());
        assert_eq!(unsafe { memory::PyObject_GC_IsTracked(descr) }, 1);
        assert_eq!(unsafe { typeobj::PyDescr_IsData(descr) }, 1);
        let ty = unsafe { (*descr).ob_type };
        let get = unsafe { (*ty).tp_descr_get.unwrap() };
        let set = unsafe { (*ty).tp_descr_set.unwrap() };
        let result = unsafe { get(descr, (&raw mut receiver).cast(), (&raw mut *owner).cast()) };
        assert_eq!(unsafe { numbers::PyLong_AsLong(result) }, 42);
        unsafe { refcount::Py_DECREF(result) };
        let before = CALLS.with(Cell::get);
        assert!(unsafe { get(descr, (&raw mut wrong).cast(), (&raw mut *owner).cast()) }.is_null());
        assert_eq!(
            unsafe { errors::PyErr_Occurred() },
            (&raw mut PyExc_TypeError).cast()
        );
        unsafe { errors::PyErr_Clear() };
        assert_eq!(
            unsafe { set(descr, (&raw mut wrong).cast(), ptr::null_mut()) },
            -1
        );
        assert_eq!(
            CALLS.with(Cell::get),
            before,
            "wrong receiver must not reach a callback or memory offset"
        );
        unsafe { errors::PyErr_Clear() };
        let mut edges: Vec<usize> = Vec::new();
        unsafe {
            ((*ty).tp_traverse.unwrap())(descr, visit as *const () as _, (&raw mut edges).cast())
        };
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0], (&raw mut *owner).addr());
    }
    let getset_descr = descriptors[0];
    assert_eq!(
        unsafe {
            ((*(*getset_descr).ob_type).tp_descr_set.unwrap())(
                getset_descr,
                (&raw mut receiver).cast(),
                ptr::null_mut(),
            )
        },
        0
    );
    for descr in descriptors {
        unsafe { refcount::Py_DECREF(descr) };
        assert!(!crate::object::gc::native_gc_is_enrolled(descr.addr()));
    }
    assert_eq!(owner.ob_base.ob_base.ob_refcnt, owner_baseline);
}

unsafe extern "C" fn bare_failure(_: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}
unsafe extern "C" fn success_with_error(object: *mut PyObject) -> *mut PyObject {
    unsafe {
        errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast());
        refcount::Py_INCREF(object);
    }
    object
}
unsafe extern "C" fn exact_failure(_: *mut PyObject) -> *mut PyObject {
    unsafe { errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast()) };
    ptr::null_mut()
}

#[test]
fn wrapper_failure_contract_releases_success_result_and_preserves_real_failure() {
    let _thread = init();
    CALLS.with(|calls| calls.set(0));
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    owner.tp_base = &raw mut PyBaseObject_Type;
    owner.tp_name = c"FailureReceiver".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let owner_baseline = owner.ob_base.ob_base.ob_refcnt;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *owner,
    };
    let mut base = declaration();
    let args = unsafe { sequences::PyTuple_New(0) };
    for (callback, error) in [
        (
            bare_failure as *const () as *mut c_void,
            &raw mut PyExc_SystemError,
        ),
        (
            success_with_error as *const () as *mut c_void,
            &raw mut PyExc_SystemError,
        ),
        (
            exact_failure as *const () as *mut c_void,
            &raw mut PyExc_ValueError,
        ),
    ] {
        let descr =
            unsafe { typeobj::PyDescr_NewWrapper(&raw mut *owner, &raw mut base, callback) };
        let bound = unsafe { typeobj::PyWrapper_New(descr, &raw mut receiver) };
        assert!(unsafe { invoke(bound, args) }.is_null());
        assert_eq!(unsafe { errors::PyErr_Occurred() }, error.cast());
        if callback == success_with_error as *const () as *mut c_void {
            let raised = unsafe { errors::PyErr_GetRaisedException() };
            let raised_owner = unsafe { refcount::OwnedPyObject::from_owned(raised) };
            let cause = unsafe { errors::PyException_GetCause(raised) };
            let cause_owner = unsafe { refcount::OwnedPyObject::from_owned(cause) };
            let context = unsafe { errors::PyException_GetContext(raised) };
            let context_owner = unsafe { refcount::OwnedPyObject::from_owned(context) };
            assert!(!cause.is_null());
            assert_eq!(
                cause, context,
                "malformed success chains the same original exception"
            );
            assert_eq!(
                unsafe {
                    errors::PyErr_GivenExceptionMatches(cause, (&raw mut PyExc_ValueError).cast())
                },
                1
            );
            drop(cause_owner);
            drop(context_owner);
            drop(raised_owner);
        }
        unsafe {
            errors::PyErr_Clear();
            refcount::Py_DECREF(bound);
            refcount::Py_DECREF(descr);
        }
        assert_eq!(
            receiver.ob_refcnt, 1,
            "result plus pending error must release the rejected owned result"
        );
        assert_eq!(owner.ob_base.ob_base.ob_refcnt, owner_baseline);
        assert!(!crate::object::gc::native_gc_is_enrolled(descr.addr()));
        assert!(!crate::object::gc::native_gc_is_enrolled(bound.addr()));
    }
    unsafe { refcount::Py_DECREF(args) };
}
