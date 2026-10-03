//! Native bootstrap type predicates must not require managed class hooks.
mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, sequences, typeobj};

#[repr(C)]
struct NativeMro {
    header: PyVarObject,
    items: [*mut PyObject; 2],
}

#[test]
fn exact_native_tuple_predicate_terminates_with_a_materialized_tuple_mro() {
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
    unsafe {
        let tuple_type = &raw mut PyTuple_Type;
        let object_type = &raw mut PyBaseObject_Type;
        let mut mro = NativeMro {
            header: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: tuple_type,
                },
                ob_size: 2,
            },
            items: [tuple_type.cast(), object_type.cast()],
        };
        struct RestoreMro(*mut PyTypeObject, *mut PyObject);
        impl Drop for RestoreMro {
            fn drop(&mut self) {
                unsafe { (*self.0).tp_mro = self.1 };
            }
        }
        let _restore = RestoreMro(tuple_type, (*tuple_type).tp_mro);
        (*tuple_type).tp_mro = (&raw mut mro).cast();
        let mut value = PyTupleObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: tuple_type,
                },
                ob_size: 0,
            },
            ob_item: [std::ptr::null_mut()],
        };
        let value = (&raw mut value).cast();
        assert_eq!(sequences::PyTuple_Check(value), 1);
        assert_eq!(sequences::PyTuple_CheckExact(value), 1);
        assert_eq!(sequences::PyTuple_Size(value), 0);
        assert_eq!(typeobj::PyType_IsSubtype(tuple_type, object_type), 1);
        assert!(errors::PyErr_Occurred().is_null());
    }
}
