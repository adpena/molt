use molt_cpython_abi::abi_types::PyObject;
use molt_lang_obj_model::MoltObject;

unsafe extern "C" fn accept_set_add(_set: u64, _key: u64) -> std::os::raw::c_int {
    0
}

fn register(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

#[test]
fn pyset_add_rejects_shared_frozenset() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.set_add = accept_set_add;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
    let frozen = Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut molt_cpython_abi::abi_types::PyFrozenSet_Type,
    }));
    unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.register_raw_pyobj(frozen);
    }
    let key = register(MoltObject::from_int(9).bits());
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(frozen) };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PySet_Add(frozen, key) },
        -1
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(frozen);
    }
}
