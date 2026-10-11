//! Tests for PyList_* and PyTuple_* sequence API.

#![allow(non_snake_case)]

mod support;

use std::ptr;

// Real native layouts exercise the public storage boundary independently of
// the managed list hook provider below. Every item array uses PyMem custody.
mod native_lists {
    use super::support;
    use molt_cpython_abi::abi_types::*;
    use molt_cpython_abi::api::{
        abstract_sequence, errors, memory, object, refcount, sequences, typeobj,
    };
    use refcount::OwnedPyObject;
    use std::cell::{Cell, RefCell};
    use std::ffi::{c_int, c_void};
    use std::ptr;

    #[repr(C)]
    struct Item {
        base: PyObject,
        key: i32,
        tag: usize,
    }

    thread_local! {
        static DROPS: RefCell<Vec<(usize, isize, usize)>> = const { RefCell::new(Vec::new()) };
        static DESTINATION: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        static REENTER: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        static ERROR: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
        static SORT_MODE: Cell<u8> = const { Cell::new(0) };
        static SORT_CALLS: RefCell<Vec<(usize, usize, c_int, isize)>> = const { RefCell::new(Vec::new()) };
        static EVENTS: RefCell<Vec<(&'static str, isize)>> = const { RefCell::new(Vec::new()) };
        static HINT: Cell<isize> = const { Cell::new(0) };
        static NEXT: Cell<usize> = const { Cell::new(0) };
        static VALUES: RefCell<Vec<*mut PyObject>> = const { RefCell::new(Vec::new()) };
        static ITER_ERROR: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "C" fn item_drop(op: *mut PyObject) {
        unsafe {
            let selected = errors::PyErr_GetRaisedException();
            let destination = DESTINATION.with(Cell::get);
            let len = if destination.is_null() {
                -1
            } else {
                sequences::PyList_Size(destination)
            };
            DROPS.with(|events| {
                events
                    .borrow_mut()
                    .push(((*op.cast::<Item>()).tag, len, selected as usize))
            });
            errors::PyErr_SetRaisedException(selected);
            let added = REENTER.with(|value| value.replace(ptr::null_mut()));
            if !added.is_null() {
                sequences::PyList_Append(destination, added);
            }
            memory::PyObject_Free(op.cast());
        }
    }

    unsafe extern "C" fn compare(
        left: *mut PyObject,
        right: *mut PyObject,
        op: c_int,
    ) -> *mut PyObject {
        unsafe {
            let list = DESTINATION.with(Cell::get);
            let len = sequences::PyList_Size(list);
            SORT_CALLS.with(|calls| {
                calls.borrow_mut().push((
                    (*left.cast::<Item>()).tag,
                    (*right.cast::<Item>()).tag,
                    op,
                    len,
                ))
            });
            let mode = SORT_MODE.with(|mode| mode.replace(0));
            if mode != 0 {
                let extra = REENTER.with(|value| value.replace(ptr::null_mut()));
                if mode == 3 {
                    let temporary = memory::PyObject_Malloc(size_of::<Item>()).cast::<Item>();
                    if temporary.is_null() {
                        return errors::PyErr_NoMemory();
                    }
                    temporary.write(Item {
                        base: PyObject {
                            ob_refcnt: 1,
                            ob_type: (*left).ob_type,
                        },
                        key: 9,
                        tag: 9,
                    });
                    let status = sequences::PyList_Append(list, temporary.cast());
                    refcount::Py_DECREF(temporary.cast());
                    if status < 0 {
                        return ptr::null_mut();
                    }
                } else if !extra.is_null() {
                    sequences::PyList_Append(list, extra);
                }
                if mode == 2 {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(ERROR.with(Cell::get)));
                    return ptr::null_mut();
                }
            }
            if op != 0 {
                return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
            }
            object::Py_NewRef(
                if (*left.cast::<Item>()).key < (*right.cast::<Item>()).key {
                    (&raw mut _Py_TrueStruct).cast()
                } else {
                    (&raw mut _Py_FalseStruct).cast()
                },
            )
        }
    }

    fn item_type() -> support::StaticType {
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        class.tp_name = c"NativeListItem".as_ptr();
        class.tp_basicsize = size_of::<Item>() as isize;
        class.tp_dealloc = Some(item_drop);
        class.tp_richcompare = Some(compare);
        class
    }

    unsafe fn item(class: *mut PyTypeObject, key: i32, tag: usize) -> OwnedPyObject {
        unsafe {
            let result = memory::PyObject_Malloc(size_of::<Item>()).cast::<Item>();
            assert!(!result.is_null());
            result.write(Item {
                base: PyObject {
                    ob_refcnt: 1,
                    ob_type: class,
                },
                key,
                tag,
            });
            OwnedPyObject::from_owned(result.cast())
        }
    }

    unsafe fn list() -> OwnedPyObject {
        unsafe {
            let value = typeobj::PyType_GenericAlloc(&raw mut PyList_Type, 0);
            let owner = OwnedPyObject::from_owned(value);
            assert!(!value.is_null());
            assert!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(value)
                    .is_none()
            );
            owner
        }
    }

    unsafe fn tags(list: *mut PyObject) -> Vec<usize> {
        unsafe {
            (0..sequences::PyList_Size(list))
                .map(|index| (*sequences::PyList_GetItem(list, index).cast::<Item>()).tag)
                .collect()
        }
    }

    struct CallbackState;
    impl Drop for CallbackState {
        fn drop(&mut self) {
            DESTINATION.with(|v| v.set(ptr::null_mut()));
            REENTER.with(|v| v.set(ptr::null_mut()));
            ERROR.with(|v| v.set(ptr::null_mut()));
            VALUES.with(|v| v.borrow_mut().clear());
        }
    }

    fn reset() {
        DESTINATION.with(|v| v.set(ptr::null_mut()));
        REENTER.with(|v| v.set(ptr::null_mut()));
        ERROR.with(|v| v.set(ptr::null_mut()));
        DROPS.with(|v| v.borrow_mut().clear());
        SORT_CALLS.with(|v| v.borrow_mut().clear());
        EVENTS.with(|v| v.borrow_mut().clear());
        VALUES.with(|v| v.borrow_mut().clear());
        NEXT.with(|v| v.set(0));
        ITER_ERROR.with(|v| v.set(false));
    }

    #[test]
    fn native_list_mutators_publish_identity_and_honor_stolen_references() {
        let _test = super::init();
        reset();
        unsafe {
            let class = item_type();
            let a = item(class.as_ptr(), 3, 1);
            let b = item(class.as_ptr(), 2, 2);
            let c = item(class.as_ptr(), 1, 3);
            let list = list();
            let _callback_state = CallbackState;
            assert_eq!(sequences::PyList_Append(list.as_ptr(), a.as_ptr()), 0);
            assert_eq!((*a.as_ptr()).ob_refcnt, 2);
            assert_eq!(sequences::PyList_Insert(list.as_ptr(), -99, b.as_ptr()), 0);
            assert_eq!(sequences::PyList_Insert(list.as_ptr(), 99, c.as_ptr()), 0);
            assert_eq!(tags(list.as_ptr()), [2, 1, 3]);
            assert_eq!(sequences::PyList_Reverse(list.as_ptr()), 0);
            assert_eq!(tags(list.as_ptr()), [3, 1, 2]);
            assert_eq!(
                sequences::PyList_SetItem(list.as_ptr(), 1, object::Py_NewRef(c.as_ptr())),
                0
            );
            assert_eq!((*a.as_ptr()).ob_refcnt, 1);
            assert_eq!(tags(list.as_ptr()), [3, 3, 2]);
            let previous_refs = (*c.as_ptr()).ob_refcnt;
            sequences::PyList_SET_ITEM(list.as_ptr(), 1, object::Py_NewRef(a.as_ptr()));
            assert_eq!(
                (*c.as_ptr()).ob_refcnt,
                previous_refs,
                "construction macro deliberately does not release its displaced edge"
            );
            refcount::Py_DECREF(c.as_ptr()); // caller reclaims that deliberately displaced edge
            assert_eq!(tags(list.as_ptr()), [3, 1, 2]);
            let doomed = item(class.as_ptr(), 0, 4);
            DESTINATION.with(|v| v.set(list.as_ptr()));
            assert_eq!(
                sequences::PyList_SetItem(list.as_ptr(), 8, doomed.into_ptr()),
                -1
            );
            assert_eq!(
                DROPS.with(|v| v.borrow().clone()),
                [(4, 3, 0)],
                "stolen input retires before IndexError is selected"
            );
            assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_IndexError).cast());
            errors::PyErr_Clear();
            assert_eq!(tags(list.as_ptr()), [3, 1, 2]);
            assert_eq!(sequences::PyList_Clear(list.as_ptr()), 0);
            assert_eq!((*a.as_ptr()).ob_refcnt, 1);
            assert_eq!((*b.as_ptr()).ob_refcnt, 1);
            assert_eq!((*c.as_ptr()).ob_refcnt, 1);
            DESTINATION.with(|v| v.set(ptr::null_mut()));
        }
    }

    #[test]
    fn native_slice_extend_and_repeat_keep_aliases_and_failure_atomicity() {
        let _test = super::init();
        reset();
        unsafe {
            let class = item_type();
            let values: Vec<_> = (1..=4)
                .map(|tag| item(class.as_ptr(), tag as i32, tag))
                .collect();
            let list = list();
            let _callback_state = CallbackState;
            for value in &values {
                assert_eq!(sequences::PyList_Append(list.as_ptr(), value.as_ptr()), 0);
            }
            assert_eq!(
                sequences::PyList_SetSlice(list.as_ptr(), 1, 3, list.as_ptr()),
                0
            );
            assert_eq!(tags(list.as_ptr()), [1, 1, 2, 3, 4, 4]);
            assert_eq!(
                sequences::PyList_SetSlice(list.as_ptr(), -8, 3, ptr::null_mut()),
                0
            );
            assert_eq!(tags(list.as_ptr()), [3, 4, 4]);
            assert_eq!(sequences::PyList_Extend(list.as_ptr(), list.as_ptr()), 0);
            assert_eq!(tags(list.as_ptr()), [3, 4, 4, 3, 4, 4]);
            let pointer = (*list.as_ptr().cast::<PyListObject>()).ob_item;
            let capacity = (*list.as_ptr().cast::<PyListObject>()).allocated;
            assert!(
                abstract_sequence::PySequence_InPlaceRepeat(list.as_ptr(), isize::MAX).is_null()
            );
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut PyExc_MemoryError).cast()
            );
            errors::PyErr_Clear();
            assert_eq!((*list.as_ptr().cast::<PyListObject>()).ob_item, pointer);
            assert_eq!((*list.as_ptr().cast::<PyListObject>()).allocated, capacity);
            assert_eq!(tags(list.as_ptr()), [3, 4, 4, 3, 4, 4]);
            let repeated = OwnedPyObject::from_owned(abstract_sequence::PySequence_InPlaceRepeat(
                list.as_ptr(),
                2,
            ));
            assert_eq!(repeated.as_ptr(), list.as_ptr());
            assert_eq!(tags(list.as_ptr()), [3, 4, 4, 3, 4, 4, 3, 4, 4, 3, 4, 4]);
            drop(repeated);
            assert_eq!(sequences::PyList_Clear(list.as_ptr()), 0);
            assert!(values.iter().all(|value| (*value.as_ptr()).ob_refcnt == 1));
        }
    }

    unsafe extern "C" fn get_iter(op: *mut PyObject) -> *mut PyObject {
        EVENTS.with(|v| v.borrow_mut().push(("iter", 0)));
        unsafe { object::Py_NewRef(op) }
    }
    unsafe extern "C" fn length(_: *mut PyObject) -> isize {
        EVENTS.with(|v| v.borrow_mut().push(("hint", 0)));
        HINT.with(Cell::get)
    }
    unsafe extern "C" fn next(_: *mut PyObject) -> *mut PyObject {
        unsafe {
            let len = sequences::PyList_Size(DESTINATION.with(Cell::get));
            EVENTS.with(|v| v.borrow_mut().push(("next", len)));
            let index = NEXT.with(|v| {
                let n = v.get();
                v.set(n + 1);
                n
            });
            let value = VALUES.with(|v| v.borrow().get(index).copied());
            if let Some(value) = value {
                return object::Py_NewRef(value);
            }
            if ITER_ERROR.with(Cell::get) {
                errors::PyErr_SetRaisedException(object::Py_NewRef(ERROR.with(Cell::get)));
            }
            ptr::null_mut()
        }
    }

    #[test]
    fn native_extend_observes_hint_before_iteration_and_keeps_failing_prefix() {
        let _test = super::init();
        reset();
        unsafe {
            let items = item_type();
            let a = item(items.as_ptr(), 1, 1);
            let b = item(items.as_ptr(), 2, 2);
            let mut slots: PySequenceMethods = std::mem::zeroed();
            slots.sq_length = length as *mut c_void;
            let mut class = support::StaticType::new();
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_name = c"NativeListSource".as_ptr();
            class.tp_iter = Some(get_iter);
            class.tp_iternext = Some(next);
            class.tp_as_sequence = (&raw mut slots).cast();
            let mut iterator = PyObject {
                ob_refcnt: 1,
                ob_type: class.as_ptr(),
            };
            let list = list();
            let _callback_state = CallbackState;
            DESTINATION.with(|v| v.set(list.as_ptr()));
            VALUES.with(|v| v.borrow_mut().extend([a.as_ptr(), b.as_ptr()]));
            HINT.with(|v| v.set(isize::MAX));
            assert_eq!(
                sequences::PyList_Extend(list.as_ptr(), &raw mut iterator),
                -1
            );
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut PyExc_MemoryError).cast()
            );
            errors::PyErr_Clear();
            assert_eq!(
                EVENTS.with(|v| v.borrow().clone()),
                [("iter", 0), ("hint", 0)]
            );
            assert_eq!(sequences::PyList_Size(list.as_ptr()), 0);
            assert_eq!(iterator.ob_refcnt, 1);
            EVENTS.with(|v| v.borrow_mut().clear());
            HINT.with(|v| v.set(8));
            errors::PyErr_SetString(
                (&raw mut PyExc_LookupError).cast(),
                c"exact iterator failure".as_ptr(),
            );
            let failure = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!failure.as_ptr().is_null());
            ERROR.with(|v| v.set(failure.as_ptr()));
            ITER_ERROR.with(|v| v.set(true));
            assert_eq!(
                sequences::PyList_Extend(list.as_ptr(), &raw mut iterator),
                -1
            );
            let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(selected.as_ptr(), failure.as_ptr());
            assert_eq!(tags(list.as_ptr()), [1, 2]);
            assert_eq!(
                EVENTS.with(|v| v.borrow().clone()),
                [
                    ("iter", 0),
                    ("hint", 0),
                    ("next", 0),
                    ("next", 1),
                    ("next", 2)
                ]
            );
            assert_eq!(iterator.ob_refcnt, 1);
            DESTINATION.with(|v| v.set(ptr::null_mut()));
            VALUES.with(|v| v.borrow_mut().clear());
            ERROR.with(|v| v.set(ptr::null_mut()));
        }
    }

    #[test]
    fn native_sort_uses_less_only_restores_permutation_and_prioritizes_comparison_failure() {
        let _test = super::init();
        reset();
        unsafe {
            let class = item_type();
            let values: Vec<_> = [(2, 1), (1, 2), (2, 3), (1, 4)]
                .into_iter()
                .map(|(key, tag)| item(class.as_ptr(), key, tag))
                .collect();
            let extra = item(class.as_ptr(), 9, 9);
            let list = list();
            let _callback_state = CallbackState;
            DESTINATION.with(|v| v.set(list.as_ptr()));
            for value in &values {
                assert_eq!(sequences::PyList_Append(list.as_ptr(), value.as_ptr()), 0);
            }
            assert_eq!(sequences::PyList_Sort(list.as_ptr()), 0);
            assert_eq!(tags(list.as_ptr()), [2, 4, 1, 3]);
            assert!(SORT_CALLS.with(|v| {
                v.borrow()
                    .iter()
                    .all(|&(_, _, op, len)| op == 0 && len == 0)
            }));
            for mode in [1, 2, 3] {
                errors::PyErr_SetString(
                    (&raw mut PyExc_LookupError).cast(),
                    c"sort compare failure".as_ptr(),
                );
                let failure = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!failure.as_ptr().is_null());
                ERROR.with(|v| v.set(failure.as_ptr()));
                SORT_MODE.with(|v| v.set(mode));
                REENTER.with(|v| v.set(extra.as_ptr()));
                assert_eq!(sequences::PyList_Sort(list.as_ptr()), -1);
                if mode == 2 {
                    let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(selected.as_ptr(), failure.as_ptr());
                } else {
                    assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_ValueError).cast());
                    let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    if mode == 3 {
                        assert_eq!(
                            DROPS.with(|v| v.borrow().last().copied()),
                            Some((9, 4, selected.as_ptr() as usize)),
                            "sort restores its original storage before retiring callback additions under ValueError"
                        );
                    }
                }
                let mut actual = tags(list.as_ptr());
                actual.sort_unstable();
                assert_eq!(actual, [1, 2, 3, 4]);
                assert_eq!((*extra.as_ptr()).ob_refcnt, 1);
            }
            DESTINATION.with(|v| v.set(ptr::null_mut()));
            ERROR.with(|v| v.set(ptr::null_mut()));
        }
    }
}

fn init() -> support::AbiTestThreadStateTransaction {
    support::enter_abi_test(support::stub_runtime_hooks())
}

// ---------------------------------------------------------------------------
// PyList_New
// ---------------------------------------------------------------------------

#[test]
fn test_list_new_fails_closed_on_alloc_failure() {
    // F4 teeth: with stub hooks, alloc_list returns 0 (allocation failure).
    // PyList_New MUST fail closed with NULL + a set MemoryError, NOT return a
    // non-NULL Py_None placeholder (which would defeat the caller's
    // `if (list == NULL)` guard and let it operate on None as a list).
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    assert!(
        py.is_null(),
        "PyList_New must return NULL on alloc failure, not a placeholder"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyList_New must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_list_new_with_size_fails_closed_on_alloc_failure() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(5) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_new_fails_closed() {
    // F5 teeth: PySet_New must fail closed (NULL + NotImplementedError) rather
    // than return a *list* with list semantics (no dedup, no membership).
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PySet_New(ptr::null_mut()) };
    assert!(
        py.is_null(),
        "PySet_New must fail closed, not return a list"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_membership_ops_fail_closed() {
    // F5 teeth: PySet_Contains/Add/Discard must return the error sentinel (-1)
    // with an exception set, NOT fake success/absence (0).
    let _abi_test = init();
    for result in [
        unsafe {
            molt_cpython_abi::api::sequences::PySet_Contains(ptr::null_mut(), ptr::null_mut())
        },
        unsafe { molt_cpython_abi::api::sequences::PySet_Add(ptr::null_mut(), ptr::null_mut()) },
        unsafe {
            molt_cpython_abi::api::sequences::PySet_Discard(ptr::null_mut(), ptr::null_mut())
        },
    ] {
        assert_eq!(result, -1, "set op must fail closed with -1, not fake 0");
    }
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PySet_Size(ptr::null_mut()) },
        -1,
        "PySet_Size must fail closed with -1, not fake 0"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyList_Append — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_append_null_list_returns_error() {
    let _abi_test = init();
    let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Append(ptr::null_mut(), item) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(item) };
}

#[test]
fn test_list_append_null_item_returns_error() {
    let _abi_test = init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Append(list, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn test_list_append_both_null_returns_error() {
    let _abi_test = init();
    let result = unsafe {
        molt_cpython_abi::api::sequences::PyList_Append(ptr::null_mut(), ptr::null_mut())
    };
    assert_eq!(result, -1);
}

#[test]
fn test_list_append_rejects_non_list() {
    let _abi_test = init();
    let tuple = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyList_Append(tuple, item) },
        -1
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(item);
        molt_cpython_abi::api::refcount::Py_DECREF(tuple);
    }
}

// ---------------------------------------------------------------------------
// PyList_GetItem / PyList_GET_ITEM — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_getitem_null_returns_null() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GetItem(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_list_get_item_negative_index_returns_null() {
    let _abi_test = init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GET_ITEM(list, -1) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn test_list_get_item_ref_null_returns_null() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GetItemRef(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyList_SetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_setitem_null_list_returns_error() {
    let _abi_test = init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyList_SetItem(ptr::null_mut(), 0, val) };
    assert_eq!(result, -1);
    // `PyList_SetItem` steals `val` even when the container is not a list (it
    // `Py_XDECREF`s the item before `PyErr_BadInternalCall`, matching CPython);
    // decref'ing `val` again here would be a double-free.
}

#[test]
fn test_list_setitem_null_container_and_value_returns_error() {
    let _abi_test = init();
    // A NULL container is invalid even when the replacement is NULL. Do not
    // mistake the stub's allocation failure for rejection of a NULL item.
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result = unsafe {
        molt_cpython_abi::api::sequences::PyList_SetItem(ptr::null_mut(), 0, ptr::null_mut())
    };
    assert_eq!(result, -1);
    assert_eq!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
        (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_list_setitem_negative_index_returns_error() {
    let _abi_test = init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_SetItem(list, -1, val) };
    assert_eq!(result, -1);
    // `PyList_SetItem` STEALS the item reference even on the out-of-range error
    // path (it `Py_XDECREF`s `val` before returning -1, matching CPython's
    // `listobject.c`). The caller therefore must NOT decref `val` again — doing
    // so is a double-free (a use-after-free Miri catches at `refcount.rs`).
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(list);
    }
}

// ---------------------------------------------------------------------------
// PyList_Size / PyList_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_size_null_sets_error_and_returns_minus_one() {
    // CPython: PyList_Size(non-list/NULL) → PyErr_BadInternalCall() + -1, not a
    // fabricated 0 (sentinel sweep; same class as the PyDict_Size fix).
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let size = unsafe { molt_cpython_abi::api::sequences::PyList_Size(ptr::null_mut()) };
    assert_eq!(size, -1, "PyList_Size(NULL) must be -1, not a fabricated 0");
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a -1 return from PyList_Size must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_list_get_size_null_returns_zero() {
    let _abi_test = init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyList_GET_SIZE(ptr::null_mut()) };
    assert_eq!(size, 0);
}

// ---------------------------------------------------------------------------
// PyList_Check
// ---------------------------------------------------------------------------

#[test]
fn test_list_check_null_returns_zero() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_list_check_on_int_returns_zero() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyTuple_New
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_new_returns_non_null() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_tuple_new_with_size() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_tuple_new_negative_size_rejects_with_system_error() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(-5) };
    assert!(py.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError)
                    .cast::<molt_cpython_abi::abi_types::PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyTuple_GetItem / PyTuple_GET_ITEM — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_getitem_null_returns_null() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_GetItem(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_tuple_get_item_negative_index_returns_null() {
    let _abi_test = init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_ITEM(tup, -1) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tup) };
}

// ---------------------------------------------------------------------------
// PyTuple_SetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_setitem_null_tuple_returns_error() {
    let _abi_test = init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(ptr::null_mut(), 0, val) };
    assert_eq!(result, -1);
    // `PyTuple_SetItem` steals `val` even when the container is not a tuple (it
    // `Py_XDECREF`s the item before `PyErr_BadInternalCall`, matching CPython);
    // decref'ing `val` again here would be a double-free.
}

#[test]
fn test_tuple_setitem_null_value_clears_slot_and_releases_displaced_reference() {
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyObject};
    use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, sequences};
    let _abi_test = init();
    // CPython v3.12.13 Objects/tupleobject.c:110-127 uses Py_XSETREF:
    // NULL is a valid replacement, and the displaced reference is released.
    // The stack sentinel retains one owner; the second is stolen by the tuple.
    let mut item = PyObject {
        ob_refcnt: 2,
        ob_type: &raw mut PyBaseObject_Type,
    };
    let tup = unsafe { OwnedPyObject::from_owned(sequences::PyTuple_New(1)) };
    assert!(!tup.as_ptr().is_null());
    unsafe { errors::PyErr_Clear() };
    assert_eq!(
        unsafe { sequences::PyTuple_SetItem(tup.as_ptr(), 0, ptr::null_mut()) },
        0
    );
    assert!(unsafe { sequences::PyTuple_GetItem(tup.as_ptr(), 0) }.is_null());
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    assert_eq!(
        unsafe { sequences::PyTuple_SetItem(tup.as_ptr(), 0, &raw mut item) },
        0
    );
    assert_eq!(
        unsafe { sequences::PyTuple_GetItem(tup.as_ptr(), 0) },
        &raw mut item
    );
    assert_eq!(
        item.ob_refcnt, 2,
        "SetItem steals without adding a reference"
    );
    assert_eq!(
        unsafe { sequences::PyTuple_SetItem(tup.as_ptr(), 0, ptr::null_mut()) },
        0
    );
    assert!(unsafe { sequences::PyTuple_GetItem(tup.as_ptr(), 0) }.is_null());
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    assert_eq!(
        item.ob_refcnt, 1,
        "NULL replacement releases the old slot exactly once"
    );
    drop(tup);
    assert_eq!(
        item.ob_refcnt, 1,
        "tuple destruction must not release the cleared slot again"
    );
}

#[test]
fn test_tuple_setitem_negative_index_returns_error() {
    let _abi_test = init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tup, -1, val) };
    assert_eq!(result, -1);
    // `PyTuple_SetItem` steals `val` on the out-of-range error path (it
    // `Py_XDECREF`s the item before returning -1, matching CPython); the caller
    // must NOT decref `val` again — that is a double-free.
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
    }
}

#[test]
fn test_tuple_setitem_rejects_shared_tuple() {
    let _abi_test = init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    assert!(!tup.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(tup) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tup, 0, val) },
        -1,
        "published tuple mutation must fail once the tuple is shared"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    assert!(unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_ITEM(tup, 0) }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
    }
}

// ---------------------------------------------------------------------------
// PyTuple_Size / PyTuple_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_size_null_is_bad_internal_call() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let size = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(ptr::null_mut()) };
    assert_eq!(size, -1);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_tuple_get_size_null_returns_zero() {
    let _abi_test = init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_SIZE(ptr::null_mut()) };
    assert_eq!(size, 0);
}

// ---------------------------------------------------------------------------
// PyTuple_Check
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_check_null_returns_zero() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_tuple_check_on_int_returns_zero() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_sequence_fast_items_returns_raw_tuple_storage() {
    let _abi_test = init();
    let tuple = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(2) };
    let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(11) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tuple, 0, first) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tuple, 1, second) },
        0
    );

    let fast =
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast(tuple, ptr::null()) };
    assert_eq!(fast, tuple);
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_GET_SIZE(fast) },
        2
    );
    let items = unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_ITEMS(fast) };
    assert!(!items.is_null());
    assert_eq!(unsafe { *items }, first);
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_GET_ITEM(fast, 1) },
        second
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(fast);
        molt_cpython_abi::api::refcount::Py_DECREF(tuple);
    }
}
