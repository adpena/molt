//! Compile the same type-identity observer against each distributed C header,
//! then supply real runtime projections whose physical carrier hides the class.
use super::*;
use molt_cpython_abi::abi_types;
use std::os::raw::c_int;

#[test]
fn optimize_flag_c_consumers_share_the_runtime_owner() {
    molt_cpython_abi_test_support::link();
    unsafe extern "C" {
        fn molt_public_type_identity_probe_optimize_address() -> *mut c_int;
        fn molt_linked_type_identity_probe_optimize_address() -> *mut c_int;
    }
    // Address identity does not read or mutate the process-global flag. The
    // separately executed C fixture owns its own data and proves write visibility.
    let owner = std::ptr::addr_of_mut!(abi_types::Py_OptimizeFlag);
    unsafe {
        assert_eq!(molt_public_type_identity_probe_optimize_address(), owner);
        assert_eq!(molt_linked_type_identity_probe_optimize_address(), owner);
    }
}

type Probe = unsafe extern "C" fn(*mut PyObject, *mut PyTypeObject, c_int, c_int) -> c_int;
unsafe extern "C" {
    fn molt_public_type_identity_probe(
        value: *mut PyObject,
        expected: *mut PyTypeObject,
        family: c_int,
        exact: c_int,
    ) -> c_int;
    fn molt_linked_type_identity_probe(
        value: *mut PyObject,
        expected: *mut PyTypeObject,
        family: c_int,
        exact: c_int,
    ) -> c_int;
}

unsafe fn check_headers(
    value: *mut PyObject,
    expected: *mut PyTypeObject,
    family: c_int,
    exact: c_int,
) {
    for (header, probe) in [
        (
            "public source header",
            molt_public_type_identity_probe as Probe,
        ),
        (
            "linked ABI header",
            molt_linked_type_identity_probe as Probe,
        ),
    ] {
        assert_eq!(
            unsafe { probe(value, expected, family, exact) },
            0,
            "{header}"
        );
    }
}

unsafe fn identity_subclass(py: &PyToken<'_>, base: u64, name: &[u8]) -> u64 {
    let name = attr_name_bits_from_bytes(py, name).unwrap();
    let class = crate::molt_class_new(name);
    let status = crate::molt_class_set_base(class, base);
    dec_ref_bits(py, status);
    unsafe {
        crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()).unwrap()
    };
    dec_ref_bits(py, name);
    assert!(!exception_pending(py));
    class
}

#[test]
fn compiled_headers_follow_runtime_class_for_list_tuple_and_type_projections() {
    molt_cpython_abi_test_support::link();
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let classes = builtin_classes(&py);
        for (base, family, physical, name) in [
            (
                classes.list,
                1,
                &raw mut abi_types::PyList_Type,
                b"HeaderListChild".as_slice(),
            ),
            (
                classes.tuple,
                2,
                &raw mut abi_types::PyTuple_Type,
                b"HeaderTupleChild".as_slice(),
            ),
        ] {
            let child = identity_subclass(&py, base, name);
            for (constructor, exact) in [(base, 1), (child, 0)] {
                let instance = crate::call_callable0(&py, constructor);
                assert!(!exception_pending(&py));
                assert_eq!(type_of_bits(&py, instance), constructor);
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance);
                let expected = GLOBAL_BRIDGE
                    .handle_to_borrowed_pyobj(constructor)
                    .cast::<PyTypeObject>();
                assert!(!view.is_null() && !expected.is_null());
                assert_eq!(
                    (*view).ob_type,
                    physical,
                    "witness uses the actual static carrier"
                );
                if exact == 0 {
                    assert_ne!(physical, expected);
                }
                check_headers(view, expected, family, exact);
                refcount::Py_DECREF(view);
                dec_ref_bits(&py, instance);
            }
            dec_ref_bits(&py, child);
        }

        // A class object with a custom metaclass is physically PyType_Type.
        // Source code must still see that metaclass via Py_TYPE and TypeCheck.
        let meta = identity_subclass(&py, classes.type_obj, b"HeaderMeta");
        let name = attr_name_bits_from_bytes(&py, b"HeaderClassWithMeta").unwrap();
        let bases = MoltObject::from_ptr(alloc_tuple(&py, &[classes.object])).bits();
        let namespace = MoltObject::from_ptr(alloc_dict_with_pairs(&py, &[])).bits();
        let keywords = MoltObject::from_ptr(alloc_dict_with_pairs(&py, &[])).bits();
        let class = crate::molt_type_new(meta, name, bases, namespace, keywords);
        assert!(!exception_pending(&py));
        assert_eq!(type_of_bits(&py, class), meta);
        let class_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class);
        let meta_view = GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(meta)
            .cast::<PyTypeObject>();
        assert!(!class_view.is_null() && !meta_view.is_null());
        assert_eq!((*class_view).ob_type, &raw mut abi_types::PyType_Type);
        assert_ne!(meta_view, &raw mut abi_types::PyType_Type);
        check_headers(class_view, meta_view, 0, 0);
        refcount::Py_DECREF(class_view);
        for bits in [class, keywords, namespace, bases, name, meta] {
            dec_ref_bits(&py, bits);
        }

        // Native C objects still expose their physical class. This is selected
        // by the bridge's identity authority, not duplicated in either header.
        let mut native_type: PyTypeObject = std::mem::zeroed();
        native_type.ob_base.ob_base.ob_refcnt = 1;
        native_type.tp_name = c"HeaderNativeReceiver".as_ptr();
        let mut native = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut native_type,
        };
        check_headers(&raw mut native, &raw mut native_type, 0, 0);
        check_headers(ptr::null_mut(), ptr::null_mut(), 0, 0);
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}
