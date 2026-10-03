//! Watcher lookup/error semantics use the real runtime, not stub capabilities.
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, mapping, numbers, refcount::OwnedPyObject, strings, typeobj};
use std::cell::Cell;
use std::ptr;

thread_local! {
    static WATCHER_CALLS: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn observe_type(_object: *mut PyObject) -> std::ffi::c_int {
    WATCHER_CALLS.with(|calls| calls.set(calls.get() + 1));
    0
}

unsafe fn subject() -> OwnedPyObject {
    let mut slots = [PyType_Slot {
        slot: 0,
        pfunc: ptr::null_mut(),
    }];
    let mut spec = PyType_Spec {
        name: c"watcher_test.Subject".as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as i32,
        itemsize: 0,
        flags: Py_TPFLAGS_BASETYPE as u32,
        slots: slots.as_mut_ptr(),
    };
    let object = unsafe { typeobj::PyType_FromSpec(&raw mut spec) };
    assert!(!object.is_null());
    unsafe { OwnedPyObject::from_owned(object) }
}

unsafe fn expect_error(class: *mut PyTypeObject) {
    assert_eq!(unsafe { errors::PyErr_ExceptionMatches(class.cast()) }, 1);
    unsafe { errors::PyErr_Clear() };
}

fn expect_no_error(py: &crate::PyToken<'_>) {
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    assert!(!crate::exception_pending(py));
}

#[test]
fn type_lookup_rearms_watchers_after_error_free_hits_and_misses_but_not_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|_py| unsafe {
        assert!(super::register_cpython_hooks());
        let owner = subject();
        let tp = owner.as_ptr().cast::<PyTypeObject>();
        let marker = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"marker".as_ptr()));
        let missing = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"missing".as_ptr()));
        let value = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(42));
        assert!(
            !marker.as_ptr().is_null() && !missing.as_ptr().is_null() && !value.as_ptr().is_null()
        );
        assert_eq!(
            mapping::PyDict_SetItem((*tp).tp_dict, marker.as_ptr(), value.as_ptr()),
            0
        );
        let watcher = typeobj::PyType_AddWatcher(Some(observe_type));
        assert!(watcher >= 0);
        WATCHER_CALLS.with(|calls| calls.set(0));
        assert_eq!(typeobj::PyType_Watch(watcher, owner.as_ptr()), 0);
        for name in [missing.as_ptr(), marker.as_ptr()] {
            typeobj::PyType_Modified(tp);
            let before = WATCHER_CALLS.with(Cell::get);
            let found = typeobj::_PyType_Lookup(tp, name);
            assert_eq!(
                found,
                if name == marker.as_ptr() {
                    value.as_ptr()
                } else {
                    ptr::null_mut()
                }
            );
            expect_no_error(&_py);
            assert_ne!((*tp).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
            assert_ne!((*tp).tp_version_tag, 0);
            typeobj::PyType_Modified(tp);
            assert_eq!(WATCHER_CALLS.with(Cell::get), before + 1);
        }
        // Failure in the physical MRO consumer must leave its original error
        // and must not turn the lookup into a cacheable miss.
        let saved_mro = (*tp).tp_mro;
        (*tp).tp_mro = value.as_ptr();
        let found = typeobj::_PyType_Lookup(tp, missing.as_ptr());
        (*tp).tp_mro = saved_mro;
        assert!(found.is_null());
        assert_eq!((*tp).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        expect_error(&raw mut PyExc_SystemError);
        assert_eq!(typeobj::PyType_Unwatch(watcher, owner.as_ptr()), 0);
        assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
        assert_eq!(typeobj::molt_type_clear(owner.as_ptr()), 0);
    });
}

#[test]
fn watcher_api_uses_canonical_errors_and_allows_unassignable_tags() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|_py| unsafe {
        assert!(super::register_cpython_hooks());
        for id in [-1, 8, 0] {
            assert_eq!(typeobj::PyType_ClearWatcher(id), -1);
            expect_error(&raw mut PyExc_ValueError);
        }
        let ids: Vec<_> = (0..8)
            .map(|_| typeobj::PyType_AddWatcher(Some(observe_type)))
            .collect();
        assert!(ids.iter().all(|&id| id >= 0));
        assert_eq!(typeobj::PyType_AddWatcher(Some(observe_type)), -1);
        expect_error(&raw mut PyExc_RuntimeError);
        let non_type = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(1));
        for object in [ptr::null_mut(), non_type.as_ptr()] {
            for operation in [typeobj::PyType_Watch, typeobj::PyType_Unwatch] {
                assert_eq!(operation(ids[0], object), -1);
                expect_error(&raw mut PyExc_ValueError);
            }
        }
        let mut unready: PyTypeObject = std::mem::zeroed();
        unready.ob_base.ob_base = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyType_Type,
        };
        assert_eq!(typeobj::PyType_Watch(ids[0], (&raw mut unready).cast()), 0);
        expect_no_error(&_py);
        assert_eq!(unready.tp_watched, 1 << ids[0]);
        assert_eq!(unready.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        assert_eq!(
            typeobj::PyType_Unwatch(ids[0], (&raw mut unready).cast()),
            0
        );
        for id in ids {
            assert_eq!(typeobj::PyType_ClearWatcher(id), 0);
        }
    });
}

#[test]
fn checked_type_observers_reject_terminal_managed_pins_without_resurrection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        assert!(super::register_cpython_hooks());
        let name = crate::attr_name_bits_from_bytes(&py, b"TerminalWatcherSubject").unwrap();
        let namespace = crate::alloc_dict_with_pairs(&py, &[]);
        assert!(!namespace.is_null());
        let namespace = crate::bits_from_ptr(namespace);
        let none = crate::MoltObject::none().bits();
        let class = crate::builtins::types::molt_type_new(
            crate::builtin_classes(&py).type_obj,
            name,
            none,
            namespace,
            none,
        );
        crate::dec_ref_bits(&py, namespace);
        crate::dec_ref_bits(&py, name);
        assert!(!crate::exception_pending(&py));
        let pointer = crate::obj_from_bits(class).as_ptr().expect("sealed class");
        let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class);
        assert!(!view.is_null());
        let tp = view.cast::<PyTypeObject>();
        let watcher = typeobj::PyType_AddWatcher(Some(observe_type));
        assert!(watcher >= 0);
        assert_eq!(typeobj::PyType_Watch(watcher, view), 0);
        assert_ne!((*tp).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        WATCHER_CALLS.with(|calls| calls.set(0));
        let lookup_name =
            OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"__init__".as_ptr()));
        assert!(!typeobj::_PyType_Lookup(tp, lookup_name.as_ptr()).is_null());
        let header = crate::header_from_obj_ptr(pointer);
        let c_refs = (*view).ob_refcnt;
        let runtime_refs = (*header).ref_count_snapshot();
        let flags = (*tp).tp_flags;
        let version_tag = (*tp).tp_version_tag;
        // Exercise the exact terminal admission state deterministically, while
        // the fixture owns both allocations. A positive teardown pin cannot
        // reopen a committed runtime death.
        (*header).fetch_or_flags(crate::object::HEADER_FLAG_DEALLOCATING);
        let refused = OwnedPyObject::try_from_borrowed(view).is_none();
        typeobj::PyType_Modified(tp);
        let result = typeobj::_PyType_Lookup(tp, lookup_name.as_ptr());
        let slot = (*(*view).ob_type)
            .tp_getattro
            .expect("metatype getattr slot");
        let slot_result = slot(view, lookup_name.as_ptr());
        (*header).fetch_and_flags(!crate::object::HEADER_FLAG_DEALLOCATING);
        assert!(refused);
        assert!(result.is_null());
        assert!(slot_result.is_null());
        expect_error(&raw mut PyExc_SystemError);
        assert_eq!(WATCHER_CALLS.with(Cell::get), 0);
        assert_eq!((*tp).tp_flags, flags);
        assert_eq!((*tp).tp_version_tag, version_tag);
        assert_eq!((*view).ob_refcnt, c_refs);
        assert_eq!((*header).ref_count_snapshot(), runtime_refs);
        expect_no_error(&py);
        assert_eq!(typeobj::PyType_Unwatch(watcher, view), 0);
        assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
        crate::dec_ref_bits(&py, class);
    });
}

#[test]
fn best_effort_version_tags_preserve_errors_for_malformed_physical_bases() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        assert!(super::register_cpython_hooks());
        let owner = subject();
        let tp = owner.as_ptr().cast::<PyTypeObject>();
        let invalid = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(42));
        let saved = (*tp).tp_bases;
        typeobj::PyType_Modified(tp);
        (*tp).tp_bases = invalid.as_ptr();
        let watcher = typeobj::PyType_AddWatcher(Some(observe_type));
        assert!(watcher >= 0);
        assert_eq!(typeobj::PyType_Watch(watcher, owner.as_ptr()), 0);
        expect_no_error(&py);
        assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(tp), 0);
        expect_no_error(&py);
        let runtime =
            crate::builtins::exceptions::alloc_exception(&py, "LookupError", "runtime original");
        assert!(!runtime.is_null());
        let runtime = crate::bits_from_ptr(runtime);
        errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"original".as_ptr());
        let original = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!original.as_ptr().is_null());
        molt_cpython_abi::api::refcount::Py_INCREF(original.as_ptr());
        errors::PyErr_SetRaisedException(original.as_ptr());
        crate::builtins::exceptions::molt_exception_set_last(runtime);
        assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(tp), 0);
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(runtime));
        let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(observed.as_ptr(), original.as_ptr());
        assert!(!crate::exception_pending(&py));
        crate::dec_ref_bits(&py, runtime);
        (*tp).tp_bases = saved;
        assert_eq!(typeobj::PyType_Unwatch(watcher, owner.as_ptr()), 0);
        assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
        assert_eq!(typeobj::molt_type_clear(owner.as_ptr()), 0);
    });
}

#[test]
fn watcher_slots_are_reset_for_each_runtime_lifetime() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        for cycle in 0..3 {
            let execution = crate::concurrency::execution::RuntimeExecutionGuard::enter();
            assert!(super::register_cpython_hooks());
            for expected in 0..8 {
                assert_eq!(
                    unsafe { typeobj::PyType_AddWatcher(Some(observe_type)) },
                    expected
                );
            }
            drop(execution);
            assert_eq!(crate::state::runtime_state::molt_runtime_shutdown(), 1);
            if cycle != 2 {
                crate::state::runtime_state::molt_runtime_reset_for_testing();
                assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            }
        }
    });
}
