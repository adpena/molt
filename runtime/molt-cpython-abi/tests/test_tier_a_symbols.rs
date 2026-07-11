use molt_cpython_abi::abi_types::{
    METH_FASTCALL, METH_KEYWORDS, METH_METHOD, PyMethodDef, PyObject, PyTypeObject,
};
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

#[test]
fn thread_lock_and_tss_have_real_state() {
    unsafe {
        let lock = molt_cpython_abi::api::thread::PyThread_allocate_lock();
        assert!(!lock.is_null());
        assert_eq!(
            molt_cpython_abi::api::thread::PyThread_acquire_lock(lock, 0),
            1
        );
        assert_eq!(
            molt_cpython_abi::api::thread::PyThread_acquire_lock(lock, 0),
            0
        );
        molt_cpython_abi::api::thread::PyThread_release_lock(lock);
        assert_eq!(
            molt_cpython_abi::api::thread::PyThread_acquire_lock(lock, 0),
            1
        );
        molt_cpython_abi::api::thread::PyThread_release_lock(lock);
        molt_cpython_abi::api::thread::PyThread_free_lock(lock);

        let key = molt_cpython_abi::api::thread::PyThread_tss_alloc();
        assert!(!key.is_null());
        assert_eq!(molt_cpython_abi::api::thread::PyThread_tss_create(key), 0);
        assert_eq!(
            molt_cpython_abi::api::thread::PyThread_tss_is_created(key),
            1
        );
        let value = 0x1234usize as *mut c_void;
        assert_eq!(
            molt_cpython_abi::api::thread::PyThread_tss_set(key, value),
            0
        );
        assert_eq!(molt_cpython_abi::api::thread::PyThread_tss_get(key), value);
        molt_cpython_abi::api::thread::PyThread_tss_delete(key);
        assert!(molt_cpython_abi::api::thread::PyThread_tss_get(key).is_null());
        molt_cpython_abi::api::thread::PyThread_tss_free(key);
    }
}

unsafe extern "C" fn method_stub(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[test]
fn pycmethod_new_requires_and_stores_defining_class() {
    unsafe { molt_cpython_abi::abi_types::init_static_types() };
    molt_cpython_abi::bridge::init_tag_table();
    let mut definition = PyMethodDef {
        ml_name: c"method".as_ptr(),
        ml_meth: Some(method_stub),
        ml_flags: METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
        ml_doc: ptr::null(),
    };
    let mut class: PyTypeObject = unsafe { std::mem::zeroed() };
    let method = unsafe {
        molt_cpython_abi::api::object::PyCMethod_New(
            &mut definition,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut class,
        )
    };
    assert!(!method.is_null());
    let method = method.cast::<molt_cpython_abi::abi_types::PyCMethodObject>();
    assert!(std::ptr::eq(unsafe { (*method).mm_class }, &class));
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(method.cast()) };
}

unsafe extern "C" {
    fn _PyArg_ParseTuple_SizeT(args: *mut PyObject, format: *const c_char, ...) -> c_int;
    fn _PyObject_CallFunction_SizeT(
        callable: *mut PyObject,
        format: *const c_char,
        ...
    ) -> *mut PyObject;
}

#[test]
fn size_t_entry_points_are_linked_and_execute() {
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    assert!(!args.is_null());
    assert_eq!(unsafe { _PyArg_ParseTuple_SizeT(args, c"".as_ptr()) }, 1);
    let callable = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    assert!(unsafe { _PyObject_CallFunction_SizeT(callable, c"".as_ptr()) }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(callable) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(args) };
}

#[test]
fn new_exception_rejects_unqualified_name() {
    unsafe { molt_cpython_abi::abi_types::init_static_types() };
    let bad = unsafe {
        molt_cpython_abi::api::errors::PyErr_NewException(
            c"Unqualified".as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert!(bad.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
