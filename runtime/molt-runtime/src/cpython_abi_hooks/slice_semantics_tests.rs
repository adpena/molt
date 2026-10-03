//! Real runtime slices through both the C and Python construction paths.
#![allow(non_snake_case)]
use crate::MoltObject;
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::abi_types::{Py_None, PySliceObject};
use molt_cpython_abi::api::{errors, numbers, object, refcount, sequences, slice};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::cell::RefCell;

unsafe fn assert_list(list: *mut PyObject, expected: &[i64]) {
    assert!(!list.is_null());
    assert_eq!(
        unsafe { sequences::PyList_Size(list) },
        expected.len() as isize
    );
    for (index, value) in expected.iter().copied().enumerate() {
        let item = unsafe { sequences::PyList_GetItem(list, index as isize) };
        assert!(!item.is_null());
        assert_eq!(unsafe { numbers::PyLong_AsLongLong(item) }, value);
    }
}

#[test]
fn c_and_runtime_slices_share_list_get_set_delete_and_physical_fields() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let values = [0, 1, 2, 3].map(|value| MoltObject::from_int(value).bits());
        let list = MoltObject::from_ptr(crate::alloc_list(&py, &values)).bits();
        let list_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(list);
        let reverse = numbers::PyLong_FromLong(-1);
        let key = slice::PySlice_New(std::ptr::null_mut(), std::ptr::null_mut(), reverse);
        refcount::Py_DECREF(reverse);
        assert!(!key.is_null());
        let key_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(key).unwrap().bits();
        assert_eq!(
            crate::object_type_id(crate::obj_from_bits(key_bits).as_ptr().unwrap()),
            crate::TYPE_ID_SLICE
        );
        let result = object::PyObject_GetItem(list_view, key);
        assert_list(result, &[3, 2, 1, 0]);
        // Extended self-assignment must snapshot the replacement before writes.
        assert_eq!(object::PyObject_SetItem(list_view, key, list_view), 0);
        assert_list(list_view, &[3, 2, 1, 0]);
        refcount::Py_DECREF(result);
        assert_eq!(object::PyObject_DelItem(list_view, key), 0);
        assert_list(list_view, &[]);
        refcount::Py_DECREF(key);
        refcount::Py_DECREF(list_view);
        crate::dec_ref_bits(&py, list);

        // The opposite crossing must expose the same actual slice, not Other.
        let key_bits = crate::molt_slice_new(
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            MoltObject::from_int(-2).bits(),
        );
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(key_bits);
        assert_eq!(slice::PySlice_Check(view), 1);
        assert_eq!(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(view).unwrap().bits(),
            key_bits
        );
        let physical = view.cast::<PySliceObject>();
        assert_eq!((*physical).start, &raw mut Py_None);
        assert_eq!((*physical).stop, &raw mut Py_None);
        assert_eq!(numbers::PyLong_AsLong((*physical).step), -2);
        let (mut start, mut stop, mut step, mut length) = (0, 0, 0, 0);
        assert_eq!(
            slice::PySlice_GetIndicesEx(view, 5, &mut start, &mut stop, &mut step, &mut length),
            0
        );
        assert_eq!((start, stop, step, length), (4, -1, -2, 3));
        refcount::Py_DECREF(view);
        crate::dec_ref_bits(&py, key_bits);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

thread_local! {
    static OBSERVED_KEYS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static INDEX_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INDEX_ERROR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

extern "C" fn echo_key(_receiver: u64, key: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        OBSERVED_KEYS.with(|keys| keys.borrow_mut().push(key));
        crate::inc_ref_bits(py, key);
        key
    })
}
extern "C" fn set_key(receiver: u64, key: u64, _value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = echo_key(receiver, key);
        crate::dec_ref_bits(py, result);
        MoltObject::none().bits()
    })
}
extern "C" fn index_error(_receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        INDEX_CALLS.with(|calls| calls.set(calls.get() + 1));
        let error = INDEX_ERROR.with(|error| error.get());
        crate::record_exception(py, crate::obj_from_bits(error).as_ptr().unwrap());
        MoltObject::none().bits()
    })
}

fn callback_instance(py: &crate::PyToken<'_>, methods: &[(&[u8], *const (), u64)]) -> (u64, u64) {
    let name = crate::attr_name_bits_from_bytes(py, b"SliceCallbackProbe").unwrap();
    let class = crate::molt_class_new(name);
    crate::dec_ref_bits(py, name);
    let result = crate::molt_class_set_base(class, crate::builtin_classes(py).object);
    crate::dec_ref_bits(py, result);
    for &(name, target, arity) in methods {
        let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr("slice_callback_probe", target),
            arity,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        let result = crate::molt_set_attr_name(class, name, function);
        for value in [name, function, result] {
            crate::dec_ref_bits(py, value);
        }
    }
    let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
    let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
    assert!(!crate::exception_pending(py));
    (class, instance)
}

#[test]
fn slice_construction_preserves_callback_key_identity_and_original_index_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        OBSERVED_KEYS.with(|keys| keys.borrow_mut().clear());
        INDEX_CALLS.with(|calls| calls.set(0));
        let (bound_class, bound) =
            callback_instance(&py, &[(b"__index__", index_error as *const (), 1)]);
        let (mapping_class, mapping) = callback_instance(
            &py,
            &[
                (b"__getitem__", echo_key as *const (), 2),
                (b"__setitem__", set_key as *const (), 3),
                (b"__delitem__", echo_key as *const (), 2),
            ],
        );
        let error =
            crate::builtins::exceptions::alloc_exception(&py, "ValueError", "slice index sentinel");
        let error = MoltObject::from_ptr(error).bits();
        INDEX_ERROR.with(|slot| slot.set(error));
        let error_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(error);
        let bound_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bound);
        assert_eq!(
            molt_cpython_abi::api::abstract_number::PyIndex_Check(bound_view),
            1
        );
        assert_eq!(
            INDEX_CALLS.with(|calls| calls.get()),
            0,
            "index admission must not execute the descriptor or method"
        );
        let before = GLOBAL_BRIDGE.mirrored_c_refcount(bound_view.addr());
        let key = slice::PySlice_New(bound_view, std::ptr::null_mut(), std::ptr::null_mut());
        assert!(!key.is_null());
        assert_eq!((*key.cast::<PySliceObject>()).start, bound_view);
        assert_eq!(
            GLOBAL_BRIDGE.mirrored_c_refcount(bound_view.addr()),
            before + 1
        );
        assert_eq!(INDEX_CALLS.with(|calls| calls.get()), 0);
        let bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(key).unwrap().bits();
        let mapping_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(mapping);
        let result = object::PyObject_GetItem(mapping_view, key);
        assert_eq!(result, key);
        refcount::Py_DECREF(result);
        assert_eq!(
            object::PyObject_SetItem(mapping_view, key, &raw mut Py_None),
            0
        );
        assert_eq!(object::PyObject_DelItem(mapping_view, key), 0);
        OBSERVED_KEYS.with(|keys| assert_eq!(&*keys.borrow(), &[bits, bits, bits]));
        assert_eq!(INDEX_CALLS.with(|calls| calls.get()), 0);
        let (mut start, mut stop, mut step) = (0, 0, 0);
        assert_eq!(
            slice::PySlice_Unpack(key, &mut start, &mut stop, &mut step),
            -1
        );
        assert_eq!(INDEX_CALLS.with(|calls| calls.get()), 1);
        let raised = errors::take_current_error().expect("original index exception");
        assert_eq!(raised.value, error_view);
        errors::with_preserved_error(|| drop(raised));
        crate::clear_exception(&py);
        refcount::Py_DECREF(key);
        assert_eq!(GLOBAL_BRIDGE.mirrored_c_refcount(bound_view.addr()), before);
        for view in [bound_view, mapping_view] {
            refcount::Py_DECREF(view);
        }
        INDEX_ERROR.with(|slot| slot.set(0));
        for bits in [mapping, mapping_class, bound, bound_class, error] {
            crate::dec_ref_bits(&py, bits);
        }
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn slice_projection_mirrors_do_not_root_a_managed_cycle() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let list = sequences::PyList_New(0);
        assert!(!list.is_null());
        let key = slice::PySlice_New(list, std::ptr::null_mut(), std::ptr::null_mut());
        assert!(!key.is_null());
        assert_eq!(sequences::PyList_Append(list, key), 0);
        let key_address = key;
        let list_address = list;
        refcount::Py_DECREF(key);
        refcount::Py_DECREF(list);
        assert_eq!(
            crate::object::gc::collect_cycles(&py).status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert!(
            GLOBAL_BRIDGE
                .managed_handle_for_pyobj(key_address)
                .is_none()
        );
        assert!(
            GLOBAL_BRIDGE
                .managed_handle_for_pyobj(list_address)
                .is_none()
        );
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn test_slice_new_owns_start_stop_and_normalizes_null_step() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        // Mortal carriers prove that the slice owns start/stop references. Cached
        // small integers are immortal and intentionally ignore INCREF.
        let start = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1002) };
        let stop = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1005) };
        let start_refcnt_before = unsafe { (*start).ob_refcnt };
        let stop_refcnt_before = unsafe { (*stop).ob_refcnt };
        let slice =
            unsafe { molt_cpython_abi::api::slice::PySlice_New(start, stop, std::ptr::null_mut()) };
        assert!(!slice.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::slice::PySlice_Check(slice) },
            1
        );

        let layout = slice.cast::<PySliceObject>();
        assert!(std::ptr::eq(unsafe { (*layout).start }, start));
        assert!(std::ptr::eq(unsafe { (*layout).stop }, stop));
        assert!(std::ptr::eq(unsafe { (*layout).step }, &raw mut Py_None));
        assert_eq!(unsafe { (*start).ob_refcnt }, start_refcnt_before + 1);
        assert_eq!(unsafe { (*stop).ob_refcnt }, stop_refcnt_before + 1);

        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(slice);
            molt_cpython_abi::api::refcount::Py_DECREF(start);
            molt_cpython_abi::api::refcount::Py_DECREF(stop);
        }
    });
}

#[test]
fn test_slice_get_indices_ex_positive_step() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let start = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
        let stop = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(6) };
        let step = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
        let slice = unsafe { molt_cpython_abi::api::slice::PySlice_New(start, stop, step) };
        let mut out_start = 0;
        let mut out_stop = 0;
        let mut out_step = 0;
        let mut out_len = 0;

        assert_eq!(
            unsafe {
                molt_cpython_abi::api::slice::PySlice_GetIndicesEx(
                    slice,
                    10,
                    &raw mut out_start,
                    &raw mut out_stop,
                    &raw mut out_step,
                    &raw mut out_len,
                )
            },
            0
        );
        assert_eq!((out_start, out_stop, out_step, out_len), (1, 6, 2, 3));

        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(slice);
            molt_cpython_abi::api::refcount::Py_DECREF(start);
            molt_cpython_abi::api::refcount::Py_DECREF(stop);
            molt_cpython_abi::api::refcount::Py_DECREF(step);
        }
    });
}

#[test]
fn test_slice_get_indices_ex_negative_step_defaults() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let step = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
        let slice = unsafe {
            molt_cpython_abi::api::slice::PySlice_New(&raw mut Py_None, &raw mut Py_None, step)
        };
        let mut out_start = 0;
        let mut out_stop = 0;
        let mut out_step = 0;
        let mut out_len = 0;

        assert_eq!(
            unsafe {
                molt_cpython_abi::api::slice::PySlice_GetIndicesEx(
                    slice,
                    4,
                    &raw mut out_start,
                    &raw mut out_stop,
                    &raw mut out_step,
                    &raw mut out_len,
                )
            },
            0
        );
        assert_eq!((out_start, out_stop, out_step, out_len), (3, -1, -1, 4));

        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(slice);
            molt_cpython_abi::api::refcount::Py_DECREF(step);
        }
    });
}
