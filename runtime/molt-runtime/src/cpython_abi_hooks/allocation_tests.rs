use crate::concurrency::gil::with_gil;
use molt_cpython_abi::abi_types::{
    Py_TPFLAGS_HEAPTYPE, Py_ssize_t, PyObject, PyTypeObject, PyVarObject,
};
use molt_cpython_abi::api::errors;
use std::os::raw::c_int;

type AllocationProbe = unsafe extern "C" fn(
    *mut PyTypeObject,
    *mut PyTypeObject,
    *mut PyTypeObject,
    Py_ssize_t,
) -> c_int;
type AllocationSlot = unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject;

unsafe extern "C" {
    fn molt_linked_allocation_probe(
        fixed: *mut PyTypeObject,
        variable: *mut PyTypeObject,
        custom: *mut PyTypeObject,
        fixed_size: Py_ssize_t,
    ) -> c_int;
    fn molt_public_allocation_probe(
        fixed: *mut PyTypeObject,
        variable: *mut PyTypeObject,
        custom: *mut PyTypeObject,
        fixed_size: Py_ssize_t,
    ) -> c_int;
    fn molt_linked_allocation_probe_allocator(
        ty: *mut PyTypeObject,
        items: Py_ssize_t,
    ) -> *mut PyObject;
    fn molt_public_allocation_probe_allocator(
        ty: *mut PyTypeObject,
        items: Py_ssize_t,
    ) -> *mut PyObject;
}

fn native_heap_type(basicsize: usize, itemsize: usize) -> PyTypeObject {
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.ob_base.ob_base.ob_refcnt = 16;
    ty.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    ty.tp_name = c"NativeAllocationProbe".as_ptr();
    ty.tp_basicsize = basicsize as Py_ssize_t;
    ty.tp_itemsize = itemsize as Py_ssize_t;
    ty.tp_flags = Py_TPFLAGS_HEAPTYPE;
    ty
}

#[test]
fn both_c_headers_share_allocation_initialization_and_custom_alloc_dispatch() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let fixed_size = std::mem::size_of::<PyObject>() + 48;
        for (probe, slot) in [
            (
                molt_linked_allocation_probe as AllocationProbe,
                molt_linked_allocation_probe_allocator as AllocationSlot,
            ),
            (
                molt_public_allocation_probe as AllocationProbe,
                molt_public_allocation_probe_allocator as AllocationSlot,
            ),
        ] {
            let mut fixed = native_heap_type(fixed_size, 0);
            let mut variable = native_heap_type(
                std::mem::size_of::<PyVarObject>(),
                std::mem::size_of::<*mut PyObject>(),
            );
            let mut custom = native_heap_type(fixed_size, 0);
            custom.tp_alloc = Some(slot);
            assert_eq!(
                probe(
                    &raw mut fixed,
                    &raw mut variable,
                    &raw mut custom,
                    fixed_size as Py_ssize_t
                ),
                0
            );
            for ty in [&fixed, &variable, &custom] {
                assert_eq!(ty.ob_base.ob_base.ob_refcnt, 16);
            }
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
    });
}
