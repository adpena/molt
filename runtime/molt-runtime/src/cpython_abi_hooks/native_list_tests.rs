//! Genuine native list storage crossing production runtime identity boundaries.
use crate::object::foreign::NativeObjectOwner;
use molt_cpython_abi::abi_types::{PyExc_ValueError, PyList_Type, PyObject};
use molt_cpython_abi::api::{errors, numbers, object, sequences, strings, typeobj};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

#[test]
fn native_notes_append_preserves_list_and_note_identity_through_managed_exception() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let notes =
            NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyList_Type, 0));
        assert!(!notes.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(notes.as_ptr())
                .is_none()
        );
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"native note list".as_ptr(),
        );
        let exception = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
        assert!(!exception.as_ptr().is_null());
        assert_eq!(
            object::PyObject_SetAttrString(
                exception.as_ptr(),
                c"__notes__".as_ptr(),
                notes.as_ptr()
            ),
            0
        );
        let note = NativeObjectOwner::from_owned(strings::PyUnicode_FromString(
            c"retained native note".as_ptr(),
        ));
        let method = NativeObjectOwner::from_owned(object::PyObject_GetAttrString(
            exception.as_ptr(),
            c"add_note".as_ptr(),
        ));
        assert!(!note.as_ptr().is_null() && !method.as_ptr().is_null());
        let result = NativeObjectOwner::from_owned(object::PyObject_CallOneArg(
            method.as_ptr(),
            note.as_ptr(),
        ));
        assert!(!result.as_ptr().is_null());
        let observed = NativeObjectOwner::from_owned(object::PyObject_GetAttrString(
            exception.as_ptr(),
            c"__notes__".as_ptr(),
        ));
        assert_eq!(observed.as_ptr(), notes.as_ptr());
        assert_eq!(sequences::PyList_Size(notes.as_ptr()), 1);
        assert_eq!(sequences::PyList_GetItem(notes.as_ptr(), 0), note.as_ptr());
        assert!(errors::PyErr_Occurred().is_null());
    });
}

unsafe fn assert_values(list: *mut PyObject, expected: &[i64]) {
    unsafe {
        assert_eq!(sequences::PyList_Size(list), expected.len() as isize);
        for (index, value) in expected.iter().enumerate() {
            assert_eq!(
                numbers::PyLong_AsLongLong(sequences::PyList_GetItem(list, index as isize)),
                *value
            );
        }
    }
}

#[test]
fn native_and_managed_list_mutation_share_exact_element_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let native =
            NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyList_Type, 0));
        let managed = NativeObjectOwner::from_owned(sequences::PyList_New(0));
        assert!(!native.as_ptr().is_null() && !managed.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(native.as_ptr())
                .is_none()
        );
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(managed.as_ptr())
                .is_some()
        );
        let values: Vec<_> = [7001, 7002, 7003]
            .into_iter()
            .map(|value| NativeObjectOwner::from_owned(numbers::PyLong_FromLong(value)))
            .collect();
        let starting_refs: Vec<_> = values
            .iter()
            .map(|value| (*value.as_ptr()).ob_refcnt)
            .collect();
        for value in &values {
            assert_eq!(sequences::PyList_Append(native.as_ptr(), value.as_ptr()), 0);
        }
        assert_eq!(
            sequences::PyList_Extend(managed.as_ptr(), native.as_ptr()),
            0
        );
        assert_eq!(
            sequences::PyList_Extend(native.as_ptr(), managed.as_ptr()),
            0
        );
        assert_values(native.as_ptr(), &[7001, 7002, 7003, 7001, 7002, 7003]);
        assert_values(managed.as_ptr(), &[7001, 7002, 7003]);
        for (index, value) in values.iter().enumerate() {
            assert_eq!(
                sequences::PyList_GetItem(native.as_ptr(), index as isize),
                value.as_ptr()
            );
            assert_eq!(
                sequences::PyList_GetItem(managed.as_ptr(), index as isize),
                value.as_ptr()
            );
        }
        assert_eq!(
            sequences::PyList_SetSlice(native.as_ptr(), 1, 5, managed.as_ptr()),
            0
        );
        assert_values(native.as_ptr(), &[7001, 7001, 7002, 7003, 7003]);
        assert_eq!(
            sequences::PyList_SetSlice(managed.as_ptr(), 1, 2, native.as_ptr()),
            0
        );
        assert_values(
            managed.as_ptr(),
            &[7001, 7001, 7001, 7002, 7003, 7003, 7003],
        );
        assert_eq!(sequences::PyList_Sort(native.as_ptr()), 0);
        assert_eq!(sequences::PyList_Sort(managed.as_ptr()), 0);
        assert_eq!(sequences::PyList_Clear(native.as_ptr()), 0);
        assert_eq!(sequences::PyList_Clear(managed.as_ptr()), 0);
        for (value, initial_refs) in values.iter().zip(starting_refs) {
            assert_eq!(
                (*value.as_ptr()).ob_refcnt,
                initial_refs,
                "both storage owners released every repeated edge"
            );
        }
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[derive(Clone, Copy, Debug)]
enum ListRoute {
    Native,
    Managed,
    ManagedBuiltinSlot,
}

impl ListRoute {
    unsafe fn list(self, values: &[i64]) -> NativeObjectOwner {
        unsafe {
            let list = NativeObjectOwner::from_owned(match self {
                Self::Native => typeobj::PyType_GenericAlloc(&raw mut PyList_Type, 0),
                _ => sequences::PyList_New(0),
            });
            assert!(!list.as_ptr().is_null());
            for &value in values {
                let item = NativeObjectOwner::from_owned(numbers::PyLong_FromLongLong(value));
                assert!(!item.as_ptr().is_null());
                assert_eq!(sequences::PyList_Append(list.as_ptr(), item.as_ptr()), 0);
            }
            list
        }
    }

    unsafe fn get(self, list: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
        unsafe {
            if matches!(self, Self::ManagedBuiltinSlot) {
                type Get = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
                let slots = PyList_Type
                    .tp_as_mapping
                    .cast::<molt_cpython_abi::abi_types::PyMappingMethods>();
                let call: Get = std::mem::transmute((*slots).mp_subscript);
                call(list, key)
            } else {
                object::PyObject_GetItem(list, key)
            }
        }
    }

    unsafe fn set(self, list: *mut PyObject, key: *mut PyObject, value: *mut PyObject) -> i32 {
        unsafe {
            if matches!(self, Self::ManagedBuiltinSlot) {
                type Set = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> i32;
                let slots = PyList_Type
                    .tp_as_mapping
                    .cast::<molt_cpython_abi::abi_types::PyMappingMethods>();
                let call: Set = std::mem::transmute((*slots).mp_ass_subscript);
                call(list, key, value)
            } else if value.is_null() {
                object::PyObject_DelItem(list, key)
            } else {
                object::PyObject_SetItem(list, key, value)
            }
        }
    }
}

unsafe fn slice_key(start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> NativeObjectOwner {
    unsafe {
        let fields = [start, stop, step].map(|value| {
            NativeObjectOwner::from_owned(value.map_or(std::ptr::null_mut(), |value| {
                numbers::PyLong_FromLongLong(value)
            }))
        });
        let key = NativeObjectOwner::from_owned(molt_cpython_abi::api::slice::PySlice_New(
            fields[0].as_ptr(),
            fields[1].as_ptr(),
            fields[2].as_ptr(),
        ));
        assert!(!key.as_ptr().is_null());
        key
    }
}

#[test]
fn native_and_managed_mapping_slices_keep_the_public_literals_and_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        for route in [
            ListRoute::Native,
            ListRoute::Managed,
            ListRoute::ManagedBuiltinSlot,
        ] {
            let list = route.list(&[0, 1, 2, 3, 4, 5, 6, 7]);
            let key = slice_key(Some(7), Some(1), Some(-2));
            let read = NativeObjectOwner::from_owned(route.get(list.as_ptr(), key.as_ptr()));
            assert!(!read.as_ptr().is_null(), "{route:?}");
            assert_values(read.as_ptr(), &[7, 5, 3]);
            let replacement = ListRoute::Native.list(&[70, 50, 30]);
            assert_eq!(
                route.set(list.as_ptr(), key.as_ptr(), replacement.as_ptr()),
                0,
                "{route:?}"
            );
            assert_values(list.as_ptr(), &[0, 1, 2, 30, 4, 50, 6, 70]);
            for (destination, source) in [(7, 0), (5, 1), (3, 2)] {
                assert_eq!(
                    sequences::PyList_GetItem(list.as_ptr(), destination),
                    sequences::PyList_GetItem(replacement.as_ptr(), source)
                );
            }
            let mismatch = slice_key(None, None, Some(2));
            let too_short = ListRoute::Native.list(&[99]);
            assert_eq!(
                route.set(list.as_ptr(), mismatch.as_ptr(), too_short.as_ptr()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                0
            );
            let error = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
            assert!(!error.as_ptr().is_null());
            assert_values(list.as_ptr(), &[0, 1, 2, 30, 4, 50, 6, 70]);
            let deletion = slice_key(Some(6), Some(0), Some(-2));
            assert_eq!(
                route.set(list.as_ptr(), deletion.as_ptr(), std::ptr::null_mut()),
                0
            );
            assert_values(list.as_ptr(), &[0, 1, 30, 50, 70]);
            let reverse = slice_key(None, None, Some(-1));
            assert_eq!(route.set(list.as_ptr(), reverse.as_ptr(), list.as_ptr()), 0);
            assert_values(list.as_ptr(), &[70, 50, 30, 1, 0]);
            let index = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(-1));
            let last = NativeObjectOwner::from_owned(route.get(list.as_ptr(), index.as_ptr()));
            assert_eq!(last.as_ptr(), sequences::PyList_GetItem(list.as_ptr(), 4));
            assert_eq!(
                route.set(list.as_ptr(), index.as_ptr(), std::ptr::null_mut()),
                0
            );
            assert_values(list.as_ptr(), &[70, 50, 30, 1]);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[repr(C)]
struct GrowingReplacement {
    header: PyObject,
    destination: *mut PyObject, // Borrowed from the test's retained list owner.
    started: bool,
    next: i64,
}

unsafe extern "C" fn growing_iter(value: *mut PyObject) -> *mut PyObject {
    unsafe {
        let iterator = &mut *value.cast::<GrowingReplacement>();
        if !iterator.started {
            iterator.started = true;
            for number in [6, 7] {
                let item = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(number));
                if item.as_ptr().is_null()
                    || sequences::PyList_Append(iterator.destination, item.as_ptr()) < 0
                {
                    return std::ptr::null_mut();
                }
            }
        }
        object::Py_NewRef(value)
    }
}

unsafe extern "C" fn growing_next(value: *mut PyObject) -> *mut PyObject {
    unsafe {
        let iterator = &mut *value.cast::<GrowingReplacement>();
        if iterator.next == 2 {
            return std::ptr::null_mut();
        }
        let number = 90 + iterator.next;
        iterator.next += 1;
        numbers::PyLong_FromLongLong(number)
    }
}

unsafe extern "C" fn free_fixture(value: *mut PyObject) {
    unsafe { molt_cpython_abi::api::memory::PyObject_Free(value.cast()) };
}

#[test]
fn replacement_iteration_precedes_slice_normalization_for_both_list_storage_owners() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyTypeObject};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"native_list.GrowingReplacement",
        );
        class.tp_basicsize = std::mem::size_of::<GrowingReplacement>() as isize;
        class.tp_dealloc = Some(free_fixture);
        class.tp_iter = Some(growing_iter);
        class.tp_iternext = Some(growing_next);
        assert_eq!(class.ready(), 0);
        for route in [
            ListRoute::Native,
            ListRoute::Managed,
            ListRoute::ManagedBuiltinSlot,
        ] {
            for (start, step, expected) in [
                (-4, 2, [0, 1, 2, 3, 90, 5, 91, 7]),
                (-2, 1, [0, 1, 2, 3, 4, 5, 90, 91]),
            ] {
                let list = route.list(&[0, 1, 2, 3, 4, 5]);
                let key = slice_key(Some(start), None, Some(step));
                let iterator =
                    NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
                assert!(!iterator.as_ptr().is_null());
                (*iterator.as_ptr().cast::<GrowingReplacement>()).destination = list.as_ptr();
                assert_eq!(
                    route.set(list.as_ptr(), key.as_ptr(), iterator.as_ptr()),
                    0,
                    "{route:?}, step={step}"
                );
                assert!((*iterator.as_ptr().cast::<GrowingReplacement>()).started);
                assert_eq!((*iterator.as_ptr().cast::<GrowingReplacement>()).next, 2);
                assert_values(list.as_ptr(), &expected);
                assert_eq!((*iterator.as_ptr()).ob_refcnt, 1);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

thread_local! {
    static REMOVALS: std::cell::RefCell<Vec<(usize, isize, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    static OBSERVE_REMOVALS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static APPEND_ON_REMOVAL: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

struct RemovalObservation;
impl RemovalObservation {
    fn start(append_on: Option<usize>) -> Self {
        REMOVALS.with(|events| events.borrow_mut().clear());
        APPEND_ON_REMOVAL.set(append_on);
        OBSERVE_REMOVALS.set(true);
        Self
    }
}
impl Drop for RemovalObservation {
    fn drop(&mut self) {
        OBSERVE_REMOVALS.set(false);
        APPEND_ON_REMOVAL.set(None);
    }
}

#[repr(C)]
struct RetiringItem {
    header: PyObject,
    destination: *mut PyObject,
    tag: usize,
}

unsafe extern "C" fn retiring_item(value: *mut PyObject) {
    unsafe {
        let item = &*value.cast::<RetiringItem>();
        if OBSERVE_REMOVALS.get() {
            let error = errors::PyErr_GetRaisedException();
            let len = sequences::PyList_Size(item.destination);
            REMOVALS.with(|events| events.borrow_mut().push((item.tag, len, error.addr())));
            errors::PyErr_SetRaisedException(error);
            if APPEND_ON_REMOVAL.get() == Some(item.tag) {
                let extra = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(999));
                if !extra.as_ptr().is_null() {
                    let _ = sequences::PyList_Append(item.destination, extra.as_ptr());
                }
            }
        }
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}

#[test]
fn list_removal_publishes_before_sole_native_elements_retire_in_cpython_order() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyTypeObject};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"native_list.RetiringItem",
        );
        class.tp_basicsize = std::mem::size_of::<RetiringItem>() as isize;
        class.tp_dealloc = Some(retiring_item);
        assert_eq!(class.ready(), 0);
        for route in [
            ListRoute::Native,
            ListRoute::Managed,
            ListRoute::ManagedBuiltinSlot,
        ] {
            for operation in 0..3 {
                let list = route.list(&[]);
                for tag in 0..5 {
                    let item =
                        NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
                    assert!(!item.as_ptr().is_null());
                    let state = &mut *item.as_ptr().cast::<RetiringItem>();
                    state.destination = list.as_ptr();
                    state.tag = tag;
                    assert_eq!(sequences::PyList_Append(list.as_ptr(), item.as_ptr()), 0);
                    // This local is retired now. Only the actual list owners
                    // retain this value when the operation below removes it.
                }
                let observation = RemovalObservation::start((operation == 0).then_some(3));
                let expected = match operation {
                    0 => {
                        assert_eq!(
                            sequences::PyList_SetSlice(list.as_ptr(), 1, 4, std::ptr::null_mut()),
                            0
                        );
                        assert_eq!(
                            numbers::PyLong_AsLong(sequences::PyList_GetItem(list.as_ptr(), 2)),
                            999
                        );
                        vec![(3, 2, 0), (2, 3, 0), (1, 3, 0)]
                    }
                    1 => {
                        let key = slice_key(Some(4), None, Some(-2));
                        let replacement = ListRoute::Native.list(&[90, 91, 92]);
                        assert_eq!(
                            route.set(list.as_ptr(), key.as_ptr(), replacement.as_ptr()),
                            0
                        );
                        vec![(4, 5, 0), (2, 5, 0), (0, 5, 0)]
                    }
                    _ => {
                        let key = slice_key(Some(4), None, Some(-2));
                        assert_eq!(
                            route.set(list.as_ptr(), key.as_ptr(), std::ptr::null_mut()),
                            0
                        );
                        vec![(0, 2, 0), (2, 2, 0), (4, 2, 0)]
                    }
                };
                assert_eq!(
                    REMOVALS.with(|events| events.borrow().clone()),
                    expected,
                    "{route:?}, operation={operation}"
                );
                assert!(errors::PyErr_Occurred().is_null());
                drop(observation);
            }
        }
    });
}

thread_local! {
    static SORT_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SORT_FAILURE: std::cell::Cell<*mut PyObject> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

struct SortMutationObservation;
impl Drop for SortMutationObservation {
    fn drop(&mut self) {
        SORT_APPEND.set(false);
        SORT_FAILURE.set(std::ptr::null_mut());
    }
}

unsafe extern "C" fn mutate_during_comparison(
    left: *mut PyObject,
    right: *mut PyObject,
    operation: i32,
) -> *mut PyObject {
    unsafe {
        if operation != 0 {
            return object::Py_NewRef(
                &raw mut molt_cpython_abi::abi_types::Py_NotImplementedSentinel,
            );
        }
        if SORT_APPEND.replace(false) {
            let added =
                NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc((*left).ob_type, 0));
            if added.as_ptr().is_null() {
                return std::ptr::null_mut();
            }
            let destination = (*left.cast::<RetiringItem>()).destination;
            let item = added.as_ptr().cast::<RetiringItem>();
            (*item).destination = destination;
            (*item).tag = 99;
            if sequences::PyList_Append(destination, added.as_ptr()) < 0 {
                return std::ptr::null_mut();
            }
            drop(added);
            let failure = SORT_FAILURE.get();
            if !failure.is_null() {
                errors::PyErr_SetRaisedException(object::Py_NewRef(failure));
                return std::ptr::null_mut();
            }
        }
        object::Py_NewRef(
            if (*left.cast::<RetiringItem>()).tag < (*right.cast::<RetiringItem>()).tag {
                (&raw mut molt_cpython_abi::abi_types::_Py_TrueStruct).cast()
            } else {
                (&raw mut molt_cpython_abi::abi_types::_Py_FalseStruct).cast()
            },
        )
    }
}

#[test]
fn both_list_sorts_select_failure_before_retiring_sole_callback_added_values() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyExc_LookupError, PyTypeObject};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"native_list.SortRetirement",
        );
        class.tp_basicsize = std::mem::size_of::<RetiringItem>() as isize;
        class.tp_dealloc = Some(retiring_item);
        class.tp_richcompare = Some(mutate_during_comparison);
        assert_eq!(class.ready(), 0);
        for route in [ListRoute::Native, ListRoute::Managed] {
            for comparison_fails in [false, true] {
                let list = route.list(&[]);
                for tag in [3, 1, 2] {
                    let item =
                        NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
                    assert!(!item.as_ptr().is_null());
                    let state = item.as_ptr().cast::<RetiringItem>();
                    (*state).destination = list.as_ptr();
                    (*state).tag = tag;
                    assert_eq!(sequences::PyList_Append(list.as_ptr(), item.as_ptr()), 0);
                }
                errors::PyErr_SetString(
                    (&raw mut PyExc_LookupError).cast(),
                    c"comparison remains selected".as_ptr(),
                );
                let comparison_error =
                    NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!comparison_error.as_ptr().is_null());
                let _sort_observation = SortMutationObservation;
                let _removals = RemovalObservation::start(None);
                SORT_APPEND.set(true);
                SORT_FAILURE.set(if comparison_fails {
                    comparison_error.as_ptr()
                } else {
                    std::ptr::null_mut()
                });
                assert_eq!(sequences::PyList_Sort(list.as_ptr()), -1, "{route:?}");
                if !comparison_fails {
                    assert_ne!(
                        errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                        0
                    );
                }
                let selected = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!selected.as_ptr().is_null());
                if comparison_fails {
                    assert_eq!(selected.as_ptr(), comparison_error.as_ptr());
                }
                assert_eq!(
                    REMOVALS.with(|events| events.borrow().clone()),
                    [(99, 3, selected.as_ptr().addr())],
                    "{route:?}, comparison_fails={comparison_fails}"
                );
                let mut tags: Vec<_> = (0..3)
                    .map(|index| {
                        (*sequences::PyList_GetItem(list.as_ptr(), index).cast::<RetiringItem>())
                            .tag
                    })
                    .collect();
                if !comparison_fails {
                    assert_eq!(tags, [1, 2, 3]);
                }
                tags.sort_unstable();
                assert_eq!(tags, [1, 2, 3]);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

unsafe fn call_method(
    receiver: *mut PyObject,
    name: &std::ffi::CStr,
    args: &mut [*mut PyObject],
) -> NativeObjectOwner {
    unsafe {
        let method =
            NativeObjectOwner::from_owned(object::PyObject_GetAttrString(receiver, name.as_ptr()));
        assert!(!method.as_ptr().is_null(), "missing {name:?}");
        NativeObjectOwner::from_owned(object::PyObject_Vectorcall(
            method.as_ptr(),
            args.as_mut_ptr(),
            args.len(),
            std::ptr::null_mut(),
        ))
    }
}

unsafe fn assert_text(value: *mut PyObject, expected: &str) {
    unsafe {
        assert!(!value.is_null());
        let mut len = 0;
        let bytes = strings::PyUnicode_AsUTF8AndSize(value, &mut len);
        assert!(!bytes.is_null());
        assert_eq!(
            std::slice::from_raw_parts(bytes.cast::<u8>(), len as usize),
            expected.as_bytes()
        );
    }
}

#[test]
fn genuine_native_lists_reach_inherited_descriptors_without_a_managed_proxy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        for route in [ListRoute::Native, ListRoute::Managed] {
            let list = route.list(&[3, 1, 2]);
            let seven = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(7));
            let zero = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(0));
            let two = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(2));
            assert!(
                !call_method(list.as_ptr(), c"append", &mut [seven.as_ptr()])
                    .as_ptr()
                    .is_null()
            );
            assert!(
                !call_method(
                    list.as_ptr(),
                    c"insert",
                    &mut [zero.as_ptr(), seven.as_ptr()]
                )
                .as_ptr()
                .is_null()
            );
            assert_values(list.as_ptr(), &[7, 3, 1, 2, 7]);
            let count = call_method(list.as_ptr(), c"count", &mut [seven.as_ptr()]);
            assert_eq!(numbers::PyLong_AsLong(count.as_ptr()), 2);
            let index = call_method(list.as_ptr(), c"index", &mut [seven.as_ptr()]);
            assert_eq!(numbers::PyLong_AsLong(index.as_ptr()), 0);
            let popped = call_method(list.as_ptr(), c"pop", &mut []);
            assert_eq!(popped.as_ptr(), seven.as_ptr());
            assert!(
                !call_method(list.as_ptr(), c"remove", &mut [seven.as_ptr()])
                    .as_ptr()
                    .is_null()
            );
            let native_tail = ListRoute::Native.list(&[9, 8]);
            assert!(
                !call_method(list.as_ptr(), c"extend", &mut [native_tail.as_ptr()])
                    .as_ptr()
                    .is_null()
            );
            assert_values(list.as_ptr(), &[3, 1, 2, 9, 8]);
            let copy = call_method(list.as_ptr(), c"copy", &mut []);
            assert!(!copy.as_ptr().is_null());
            assert_values(copy.as_ptr(), &[3, 1, 2, 9, 8]);
            assert!(
                !call_method(list.as_ptr(), c"reverse", &mut [])
                    .as_ptr()
                    .is_null()
            );
            assert_values(list.as_ptr(), &[8, 9, 2, 1, 3]);
            assert!(
                !call_method(list.as_ptr(), c"sort", &mut [])
                    .as_ptr()
                    .is_null()
            );
            assert_values(list.as_ptr(), &[1, 2, 3, 8, 9]);
            let repr = call_method(list.as_ptr(), c"__repr__", &mut []);
            assert_text(repr.as_ptr(), "[1, 2, 3, 8, 9]");
            let concat = call_method(list.as_ptr(), c"__add__", &mut [native_tail.as_ptr()]);
            assert_values(concat.as_ptr(), &[1, 2, 3, 8, 9, 9, 8]);
            let repeat = call_method(native_tail.as_ptr(), c"__mul__", &mut [two.as_ptr()]);
            assert_values(repeat.as_ptr(), &[9, 8, 9, 8]);
            assert_eq!(
                typeobj::PyObject_RichCompareBool(native_tail.as_ptr(), native_tail.as_ptr(), 2),
                1
            );
            let native_equal = ListRoute::Native.list(&[1, 2, 3, 8, 9]);
            let equal = call_method(list.as_ptr(), c"__eq__", &mut [native_equal.as_ptr()]);
            assert_eq!(object::PyObject_IsTrue(equal.as_ptr()), 1);
            let iter = call_method(list.as_ptr(), c"__iter__", &mut []);
            for expected in [1, 2, 3, 8, 9] {
                let item = NativeObjectOwner::from_owned(object::PyIter_Next(iter.as_ptr()));
                assert_eq!(numbers::PyLong_AsLong(item.as_ptr()), expected);
            }
            assert!(object::PyIter_Next(iter.as_ptr()).is_null());
            assert!(errors::PyErr_Occurred().is_null());
            let reverse = call_method(list.as_ptr(), c"__reversed__", &mut []);
            assert_eq!(
                sequences::PyList_SetSlice(list.as_ptr(), 1, 5, std::ptr::null_mut()),
                0
            );
            assert!(
                object::PyIter_Next(reverse.as_ptr()).is_null(),
                "shrink invalidates saved reverse cursor"
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert!(
                !call_method(list.as_ptr(), c"__init__", &mut [native_tail.as_ptr()])
                    .as_ptr()
                    .is_null()
            );
            assert_values(list.as_ptr(), &[9, 8]);
            assert!(
                !call_method(list.as_ptr(), c"clear", &mut [])
                    .as_ptr()
                    .is_null()
            );
            assert_values(list.as_ptr(), &[]);
            if matches!(route, ListRoute::Native) {
                assert!(
                    GLOBAL_BRIDGE.molt_handle_for_pyobj(list.as_ptr()).is_none(),
                    "no managed list proxy appeared"
                );
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    static KEY_TARGET: std::cell::Cell<*mut PyObject> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    static KEY_CALLS: std::cell::RefCell<Vec<(i64, isize)>> = const { std::cell::RefCell::new(Vec::new()) };
}
struct KeyObservation;
impl Drop for KeyObservation {
    fn drop(&mut self) {
        KEY_TARGET.set(std::ptr::null_mut());
    }
}
unsafe extern "C" fn last_digit_key(
    _: *mut PyObject,
    args: *mut PyObject,
    _: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let value = numbers::PyLong_AsLongLong(sequences::PyTuple_GetItem(args, 0));
        if errors::raised_error_pending() {
            return std::ptr::null_mut();
        }
        let len = sequences::PyList_Size(KEY_TARGET.get());
        KEY_CALLS.with(|calls| calls.borrow_mut().push((value, len)));
        numbers::PyLong_FromLongLong(value % 10)
    }
}

#[test]
fn native_keyed_sort_uses_physical_keys_and_stable_reverse_order() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyTypeObject};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"native_list.LastDigitKey",
        );
        class.tp_call = Some(last_digit_key);
        class.tp_dealloc = Some(free_fixture);
        assert_eq!(class.ready(), 0);
        let key = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
        assert!(!key.as_ptr().is_null());
        let names = NativeObjectOwner::from_owned(sequences::PyTuple_New(2));
        assert_eq!(
            sequences::PyTuple_SetItem(
                names.as_ptr(),
                0,
                strings::PyUnicode_FromString(c"key".as_ptr())
            ),
            0
        );
        assert_eq!(
            sequences::PyTuple_SetItem(
                names.as_ptr(),
                1,
                strings::PyUnicode_FromString(c"reverse".as_ptr())
            ),
            0
        );
        for route in [ListRoute::Native, ListRoute::Managed] {
            for reverse in [false, true] {
                let list = route.list(&[32, 11, 31, 12]);
                let source: Vec<_> = (0..4)
                    .map(|i| sequences::PyList_GetItem(list.as_ptr(), i))
                    .collect();
                let method = NativeObjectOwner::from_owned(object::PyObject_GetAttrString(
                    list.as_ptr(),
                    c"sort".as_ptr(),
                ));
                assert!(!method.as_ptr().is_null());
                let _observation = KeyObservation;
                KEY_TARGET.set(list.as_ptr());
                KEY_CALLS.with(|calls| calls.borrow_mut().clear());
                let mut args = [
                    key.as_ptr(),
                    if reverse {
                        (&raw mut molt_cpython_abi::abi_types::_Py_TrueStruct).cast()
                    } else {
                        (&raw mut molt_cpython_abi::abi_types::_Py_FalseStruct).cast()
                    },
                ];
                let result = NativeObjectOwner::from_owned(object::PyObject_Vectorcall(
                    method.as_ptr(),
                    args.as_mut_ptr(),
                    0,
                    names.as_ptr(),
                ));
                assert!(!result.as_ptr().is_null(), "{route:?}, reverse={reverse}");
                assert_eq!(
                    KEY_CALLS.with(|calls| calls.borrow().clone()),
                    vec![(32, 0), (11, 0), (31, 0), (12, 0)]
                );
                let order = if reverse { [0, 3, 1, 2] } else { [1, 2, 0, 3] };
                for (index, original) in order.into_iter().enumerate() {
                    assert_eq!(
                        sequences::PyList_GetItem(list.as_ptr(), index as isize),
                        source[original]
                    );
                }
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

#[repr(C)]
struct AppendingIndex {
    header: PyObject,
    destination: *mut PyObject,
}
unsafe extern "C" fn append_before_index(value: *mut PyObject) -> *mut PyObject {
    unsafe {
        let destination = (*value.cast::<AppendingIndex>()).destination;
        let item = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(44));
        if item.as_ptr().is_null() || sequences::PyList_Append(destination, item.as_ptr()) < 0 {
            return std::ptr::null_mut();
        }
        numbers::PyLong_FromLong(-1)
    }
}

#[test]
fn list_pop_distinguishes_none_from_omission_and_converts_before_reading_length() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{
        PyBaseObject_Type, PyExc_OverflowError, PyExc_TypeError, PyNumberMethods, PyTypeObject,
    };
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"native_list.AppendingIndex",
        );
        let mut number: PyNumberMethods = std::mem::zeroed();
        number.nb_index = append_before_index as *mut std::ffi::c_void;
        class.tp_as_number = (&raw mut number).cast();
        class.tp_basicsize = std::mem::size_of::<AppendingIndex>() as isize;
        class.tp_dealloc = Some(free_fixture);
        assert_eq!(class.ready(), 0);
        for route in [ListRoute::Native, ListRoute::Managed] {
            let list = route.list(&[7]);
            let rejected = call_method(
                list.as_ptr(),
                c"pop",
                &mut [&raw mut molt_cpython_abi::abi_types::Py_None],
            );
            assert!(rejected.as_ptr().is_null());
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            let error = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
            let message = NativeObjectOwner::from_owned(typeobj::PyObject_Str(error.as_ptr()));
            assert_text(
                message.as_ptr(),
                "'NoneType' object cannot be interpreted as an integer",
            );
            assert_values(list.as_ptr(), &[7]);
            let default = call_method(list.as_ptr(), c"pop", &mut []);
            assert_eq!(numbers::PyLong_AsLong(default.as_ptr()), 7);
            let index = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
            assert!(!index.as_ptr().is_null());
            (*index.as_ptr().cast::<AppendingIndex>()).destination = list.as_ptr();
            let converted = call_method(list.as_ptr(), c"pop", &mut [index.as_ptr()]);
            assert_eq!(numbers::PyLong_AsLong(converted.as_ptr()), 44, "{route:?}");
            assert_values(list.as_ptr(), &[]);
            let too_large = NativeObjectOwner::from_owned(numbers::PyLong_FromUnsignedLongLong(
                isize::MAX as u64 + 1,
            ));
            for (name, mut arguments) in [
                (c"pop", vec![too_large.as_ptr()]),
                (c"insert", vec![too_large.as_ptr(), default.as_ptr()]),
            ] {
                let rejected = call_method(list.as_ptr(), name, &mut arguments);
                assert!(rejected.as_ptr().is_null());
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_OverflowError).cast()),
                    0
                );
                let failure = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!failure.as_ptr().is_null());
                assert_values(list.as_ptr(), &[]);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn list_iterator_hints_identity_and_target_retirement_follow_live_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        for route in [ListRoute::Native, ListRoute::Managed] {
            for reverse in [false, true] {
                let list = route.list(&[1, 2, 3]);
                let initial_refs = (*list.as_ptr()).ob_refcnt;
                let iter = call_method(
                    list.as_ptr(),
                    if reverse {
                        c"__reversed__"
                    } else {
                        c"__iter__"
                    },
                    &mut [],
                );
                assert!(!iter.as_ptr().is_null());
                let class = object::Py_TYPE(iter.as_ptr());
                let hint = call_method(iter.as_ptr(), c"__length_hint__", &mut []);
                assert_eq!(numbers::PyLong_AsLong(hint.as_ptr()), 3);
                let first = NativeObjectOwner::from_owned(object::PyIter_Next(iter.as_ptr()));
                assert_eq!(
                    numbers::PyLong_AsLong(first.as_ptr()),
                    if reverse { 3 } else { 1 }
                );
                let hint = call_method(iter.as_ptr(), c"__length_hint__", &mut []);
                assert_eq!(numbers::PyLong_AsLong(hint.as_ptr()), 2);
                assert_eq!(sequences::PyList_Clear(list.as_ptr()), 0);
                let hint = call_method(iter.as_ptr(), c"__length_hint__", &mut []);
                assert_eq!(numbers::PyLong_AsLong(hint.as_ptr()), 0);
                assert!(object::PyIter_Next(iter.as_ptr()).is_null());
                assert!(errors::PyErr_Occurred().is_null());
                assert_eq!(
                    object::Py_TYPE(iter.as_ptr()),
                    class,
                    "public iterator identity survives target retirement"
                );
                let hint = call_method(iter.as_ptr(), c"__length_hint__", &mut []);
                assert_eq!(numbers::PyLong_AsLong(hint.as_ptr()), 0);
                if matches!(route, ListRoute::Native) {
                    assert_eq!(
                        (*list.as_ptr()).ob_refcnt,
                        initial_refs,
                        "exhausted iterator released its native target"
                    );
                }
                let extra = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(9));
                assert_eq!(sequences::PyList_Append(list.as_ptr(), extra.as_ptr()), 0);
                assert!(
                    object::PyIter_Next(iter.as_ptr()).is_null(),
                    "exhaustion cannot revive after append"
                );
            }
        }
    });
}

thread_local! {
    static OVERRIDE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe extern "C" fn overridden_list_length(_: *mut PyObject) -> isize {
    OVERRIDE_CALLS.set(OVERRIDE_CALLS.get() + 1);
    99
}
unsafe extern "C" fn overridden_list_get(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    OVERRIDE_CALLS.set(OVERRIDE_CALLS.get() + 1);
    unsafe { numbers::PyLong_FromLong(99) }
}
unsafe extern "C" fn overridden_list_set(
    _: *mut PyObject,
    _: *mut PyObject,
    _: *mut PyObject,
) -> i32 {
    OVERRIDE_CALLS.set(OVERRIDE_CALLS.get() + 1);
    0
}

#[test]
fn explicit_list_base_descriptors_bypass_native_subclass_mapping_overrides() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::{PyMappingMethods, PySequenceMethods, PyTypeObject};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut sequence: PySequenceMethods = std::mem::zeroed();
        sequence.sq_length = overridden_list_length as *mut std::ffi::c_void;
        let mut mapping: PyMappingMethods = std::mem::zeroed();
        mapping.mp_length = overridden_list_length as *mut std::ffi::c_void;
        mapping.mp_subscript = overridden_list_get as *mut std::ffi::c_void;
        mapping.mp_ass_subscript = overridden_list_set as *mut std::ffi::c_void;
        let mut class = NativeType::<PyTypeObject>::subtype(
            &raw mut PyList_Type,
            c"native_list.MappingOverrides",
        );
        class.tp_as_sequence = (&raw mut sequence).cast();
        class.tp_as_mapping = (&raw mut mapping).cast();
        assert_eq!(class.ready(), 0);
        let list = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(&mut *class, 0));
        assert!(!list.as_ptr().is_null());
        let zero = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(0));
        let seven = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(7));
        let eight = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(8));
        assert_eq!(sequences::PyList_Append(list.as_ptr(), seven.as_ptr()), 0);
        OVERRIDE_CALLS.set(0);
        let overridden =
            NativeObjectOwner::from_owned(object::PyObject_GetItem(list.as_ptr(), zero.as_ptr()));
        assert_eq!(numbers::PyLong_AsLong(overridden.as_ptr()), 99);
        assert_eq!(object::PyObject_Length(list.as_ptr()), 99);
        assert_eq!(
            object::PyObject_SetItem(list.as_ptr(), zero.as_ptr(), eight.as_ptr()),
            0
        );
        assert_eq!(OVERRIDE_CALLS.get(), 3);
        assert_values(list.as_ptr(), &[7]);

        let base = (&raw mut PyList_Type).cast();
        let item = call_method(base, c"__getitem__", &mut [list.as_ptr(), zero.as_ptr()]);
        assert_eq!(item.as_ptr(), seven.as_ptr());
        let len = call_method(base, c"__len__", &mut [list.as_ptr()]);
        assert_eq!(numbers::PyLong_AsLong(len.as_ptr()), 1);
        let set = call_method(
            base,
            c"__setitem__",
            &mut [list.as_ptr(), zero.as_ptr(), eight.as_ptr()],
        );
        assert!(!set.as_ptr().is_null());
        assert_values(list.as_ptr(), &[8]);
        let deleted = call_method(base, c"__delitem__", &mut [list.as_ptr(), zero.as_ptr()]);
        assert!(!deleted.as_ptr().is_null());
        assert_values(list.as_ptr(), &[]);
        assert_eq!(
            OVERRIDE_CALLS.get(),
            3,
            "explicit base calls never redispatch the subclass slots"
        );
        assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(list.as_ptr()).is_none());
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn list_direct_c_operations_keep_an_admitted_prior_exception_on_success() {
    use molt_cpython_abi::abi_types::{PyExc_LookupError, PyMappingMethods};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        for route in [ListRoute::Native, ListRoute::Managed] {
            for runtime_prior in [false, true] {
                let list = route.list(&[3, 1, 2]);
                let replacement = route.list(&[8, 9]);
                let indexed = route.list(&[3, 1, 2]);
                let key = slice_key(None, None, Some(2));
                let seven = NativeObjectOwner::from_owned(numbers::PyLong_FromLong(7));
                errors::PyErr_SetString(
                    (&raw mut PyExc_LookupError).cast(),
                    c"admitted prior list error".as_ptr(),
                );
                let prior = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!prior.as_ptr().is_null());
                errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                if runtime_prior {
                    assert!(super::transfer_pending_cpython_exception());
                }
                assert_eq!(sequences::PyList_Append(list.as_ptr(), seven.as_ptr()), 0);
                assert_eq!(
                    sequences::PyList_Insert(list.as_ptr(), isize::MIN, seven.as_ptr()),
                    0
                );
                assert_eq!(
                    sequences::PyList_SetSlice(list.as_ptr(), 1, 3, replacement.as_ptr()),
                    0
                );
                // The immutable base slot is a direct storage operation, not
                // the callable boundary that rejects result-plus-error.
                let mapping = PyList_Type.tp_as_mapping.cast::<PyMappingMethods>();
                type Set = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> i32;
                let assign: Set = std::mem::transmute((*mapping).mp_ass_subscript);
                assert_eq!(assign(list.as_ptr(), key.as_ptr(), indexed.as_ptr()), 0);
                assert_eq!(
                    sequences::PyList_Sort(list.as_ptr()),
                    0,
                    "{route:?}, runtime_prior={runtime_prior}"
                );
                assert_eq!(sequences::PyList_Reverse(list.as_ptr()), 0);
                if matches!(route, ListRoute::Managed) {
                    let bits = GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(list.as_ptr())
                        .unwrap()
                        .bits();
                    assert_eq!(
                        crate::c_api::PyList_Insert(
                            bits,
                            isize::MAX,
                            crate::MoltObject::from_int(7).bits()
                        ),
                        0
                    );
                }
                let selected = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(
                    selected.as_ptr(),
                    prior.as_ptr(),
                    "{route:?}, runtime_prior={runtime_prior}"
                );
                assert_values(
                    list.as_ptr(),
                    if matches!(route, ListRoute::Managed) {
                        &[8, 3, 2, 2, 1, 7]
                    } else {
                        &[8, 3, 2, 2, 1]
                    },
                );
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}
