//! CPython 3.12 sequence numeric-fallback authority.
//!
//! User classes with `__getitem__` plus `__add__`/`__mul__` expose `sq_item`
//! and numeric slots, but no `sq_concat`/`sq_repeat`. The four public sequence
//! entry points must share the reflected-aware numeric dispatcher rather than
//! rejecting those valid sequence classes or maintaining local slot logic.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    IMMORTAL_REFCNT, PyNumberMethods, PyObject, PySequenceMethods, PyTypeObject,
};
use molt_cpython_abi::api::{abstract_sequence, errors, numbers, refcount};
use std::os::raw::c_void;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static LAST_SLOT: AtomicUsize = AtomicUsize::new(0);
static LAST_COUNT: AtomicIsize = AtomicIsize::new(isize::MIN);

static mut RESULT: PyObject = PyObject {
    ob_refcnt: IMMORTAL_REFCNT,
    ob_type: ptr::null_mut(),
};

unsafe fn result(slot: usize) -> *mut PyObject {
    LAST_SLOT.store(slot, Ordering::SeqCst);
    let result = &raw mut RESULT;
    unsafe { refcount::Py_INCREF(result) };
    result
}

unsafe extern "C" fn sequence_item(_object: *mut PyObject, _index: isize) -> *mut PyObject {
    ptr::null_mut()
}

unsafe extern "C" fn number_add(_left: *mut PyObject, _right: *mut PyObject) -> *mut PyObject {
    unsafe { result(1) }
}

unsafe extern "C" fn number_inplace_add(
    _left: *mut PyObject,
    _right: *mut PyObject,
) -> *mut PyObject {
    unsafe { result(2) }
}

unsafe extern "C" fn number_multiply(_left: *mut PyObject, count: *mut PyObject) -> *mut PyObject {
    LAST_COUNT.store(
        unsafe { numbers::PyLong_AsLong(count) } as isize,
        Ordering::SeqCst,
    );
    unsafe { result(3) }
}

unsafe extern "C" fn number_inplace_multiply(
    _left: *mut PyObject,
    count: *mut PyObject,
) -> *mut PyObject {
    LAST_COUNT.store(
        unsafe { numbers::PyLong_AsLong(count) } as isize,
        Ordering::SeqCst,
    );
    unsafe { result(4) }
}

struct ForeignSequence {
    object: PyObject,
    _type: Box<PyTypeObject>,
    _sequence: Box<PySequenceMethods>,
    _number: Box<PyNumberMethods>,
}

impl ForeignSequence {
    fn new() -> Self {
        let mut sequence: Box<PySequenceMethods> = Box::new(unsafe { std::mem::zeroed() });
        sequence.sq_item = sequence_item as *mut c_void;

        let mut number: Box<PyNumberMethods> = Box::new(unsafe { std::mem::zeroed() });
        number.nb_add = number_add as *mut c_void;
        number.nb_inplace_add = number_inplace_add as *mut c_void;
        number.nb_multiply = number_multiply as *mut c_void;
        number.nb_inplace_multiply = number_inplace_multiply as *mut c_void;

        let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        ty.tp_name = c"numeric_sequence".as_ptr();
        ty.tp_as_sequence = (&raw mut *sequence).cast::<c_void>();
        ty.tp_as_number = (&raw mut *number).cast::<c_void>();
        let object = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *ty,
        };
        Self {
            object,
            _type: ty,
            _sequence: sequence,
            _number: number,
        }
    }

    fn as_ptr(&mut self) -> *mut PyObject {
        &raw mut self.object
    }
}

fn reset() {
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    unsafe { errors::PyErr_Clear() };
    LAST_SLOT.store(0, Ordering::SeqCst);
    LAST_COUNT.store(isize::MIN, Ordering::SeqCst);
}

#[test]
fn concat_and_repeat_use_the_shared_numeric_slot_authority() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    reset();
    let mut left = ForeignSequence::new();
    let mut right = ForeignSequence::new();

    let concat = unsafe { abstract_sequence::PySequence_Concat(left.as_ptr(), right.as_ptr()) };
    assert_eq!(concat, &raw mut RESULT);
    assert_eq!(LAST_SLOT.load(Ordering::SeqCst), 1);

    let repeat = unsafe { abstract_sequence::PySequence_Repeat(left.as_ptr(), 37) };
    assert_eq!(repeat, &raw mut RESULT);
    assert_eq!(LAST_SLOT.load(Ordering::SeqCst), 3);
    assert_eq!(LAST_COUNT.load(Ordering::SeqCst), 37);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn inplace_sequence_operations_prefer_the_shared_inplace_numeric_slots() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    reset();
    let mut left = ForeignSequence::new();
    let mut right = ForeignSequence::new();

    let concat =
        unsafe { abstract_sequence::PySequence_InPlaceConcat(left.as_ptr(), right.as_ptr()) };
    assert_eq!(concat, &raw mut RESULT);
    assert_eq!(LAST_SLOT.load(Ordering::SeqCst), 2);

    let repeat = unsafe { abstract_sequence::PySequence_InPlaceRepeat(left.as_ptr(), -9) };
    assert_eq!(repeat, &raw mut RESULT);
    assert_eq!(LAST_SLOT.load(Ordering::SeqCst), 4);
    assert_eq!(LAST_COUNT.load(Ordering::SeqCst), -9);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}
