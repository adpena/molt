//! One warning-emission hook provider for numeric protocol and member tests.
//! Public PyErr_WarnEx owns default category, string conversion and borrowed
//! crossings; this provider observes that exact hook contract without imports.

use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::{errors, object, refcount, strings};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::ptr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emission {
    pub message: String,
    pub category: usize,
    pub stacklevel: isize,
}

thread_local! {
    static EMISSIONS: RefCell<Vec<Emission>> = const { RefCell::new(Vec::new()) };
    static AS_ERROR: Cell<bool> = const { Cell::new(false) };
    static OBSERVER: Cell<Option<fn()>> = const { Cell::new(None) };
    static ERROR_INSTANCE: RefCell<Option<refcount::ProtectedPyObject>> = const { RefCell::new(None) };
}

pub unsafe extern "C" fn emit(message: u64, category: u64, stacklevel: i64) -> i32 {
    let message = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(message) };
    if message.is_null() {
        return -1;
    }
    let message = unsafe { refcount::OwnedPyObject::from_owned(message) };
    let category = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(category) };
    if category.is_null() {
        return -1;
    }
    let category = unsafe { refcount::OwnedPyObject::from_owned(category) };
    let text = unsafe { strings::PyUnicode_AsUTF8(message.as_ptr()) };
    if text.is_null() {
        return -1;
    }
    EMISSIONS.with(|stored| {
        stored.borrow_mut().push(Emission {
            message: unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned(),
            category: category.as_ptr() as usize,
            stacklevel: stacklevel as isize,
        });
    });
    if let Some(observer) = OBSERVER.with(Cell::get) {
        observer();
    }
    if AS_ERROR.with(Cell::get) {
        let error = ERROR_INSTANCE.with(|value| {
            value
                .borrow()
                .as_ref()
                .map_or(ptr::null_mut(), |owner| owner.as_ptr())
        });
        if error.is_null() {
            unsafe { errors::PyErr_SetString(category.as_ptr(), text) };
        } else {
            unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
        }
        -1
    } else {
        0
    }
}

pub fn set_as_error(enabled: bool) {
    AS_ERROR.with(|value| value.set(enabled));
}

pub fn set_observer(observer: Option<fn()>) {
    OBSERVER.with(|value| value.set(observer));
}

unsafe fn set_error_instance(error: *mut PyObject) {
    let owner = if error.is_null() {
        None
    } else {
        Some(unsafe { refcount::ProtectedPyObject::from_borrowed(error) })
    };
    let previous = ERROR_INSTANCE.with(|value| value.replace(owner));
    drop(previous);
}

/// Raise the supplied exception through the real warning-call boundary while
/// warning-as-error is enabled. The provider owns a reference for this scope;
/// the caller's instance and traceback remain independent identity witnesses.
///
/// # Safety
/// `error` must be a live exception instance for the duration of `run`.
pub unsafe fn with_error_instance(error: *mut PyObject, run: impl FnOnce()) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            unsafe { set_error_instance(ptr::null_mut()) };
        }
    }
    unsafe { set_error_instance(error) };
    let _reset = Reset;
    run();
}

pub fn emissions() -> Vec<Emission> {
    EMISSIONS.with(|value| value.borrow().clone())
}

pub fn last_message() -> Option<String> {
    EMISSIONS.with(|value| value.borrow().last().map(|item| item.message.clone()))
}

pub fn clear() {
    EMISSIONS.with(|value| value.borrow_mut().clear());
}

pub fn with_provider(run: impl FnOnce()) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_as_error(false);
            set_observer(None);
            unsafe { set_error_instance(ptr::null_mut()) };
        }
    }
    clear();
    set_as_error(false);
    set_observer(None);
    unsafe { set_error_instance(ptr::null_mut()) };
    let _reset = Reset;
    run();
}
