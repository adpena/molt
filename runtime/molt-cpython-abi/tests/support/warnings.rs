//! One foreign warnings provider for numeric protocol and member-write tests.
//! Real PyErr_WarnEx still owns import, lookup, argument construction and call.

use molt_cpython_abi::abi_types::{Py_None, PyObject};
use molt_cpython_abi::api::{errors, numbers, object, sequences, strings};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char};
use std::ptr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emission {
    pub message: String,
    pub category: usize,
    pub stacklevel: isize,
}

thread_local! {
    static MODULE: Cell<usize> = const { Cell::new(0) };
    static CALLABLE: Cell<usize> = const { Cell::new(0) };
    static EMISSIONS: RefCell<Vec<Emission>> = const { RefCell::new(Vec::new()) };
    static AS_ERROR: Cell<bool> = const { Cell::new(false) };
    static OBSERVER: Cell<Option<fn()>> = const { Cell::new(None) };
}

pub unsafe extern "C" fn import_module(data: *const u8, len: usize) -> u64 {
    if unsafe { std::slice::from_raw_parts(data, len) } == b"warnings" {
        let module = MODULE.with(Cell::get) as *mut PyObject;
        if !module.is_null() {
            return unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(module) }.unwrap_or(0);
        }
    }
    unsafe {
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ImportError).cast(),
            c"test module unavailable".as_ptr(),
        )
    };
    0
}

unsafe extern "C" fn getattr(_object: *mut PyObject, name: *const c_char) -> *mut PyObject {
    if unsafe { CStr::from_ptr(name) }.to_bytes() == b"warn" {
        return unsafe { object::Py_NewRef(CALLABLE.with(Cell::get) as *mut PyObject) };
    }
    ptr::null_mut()
}

unsafe extern "C" fn call(
    _object: *mut PyObject,
    args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    let message = unsafe { sequences::PyTuple_GetItem(args, 0) };
    let text = unsafe { strings::PyUnicode_AsUTF8(message) };
    if text.is_null() {
        return ptr::null_mut();
    }
    let category = unsafe { sequences::PyTuple_GetItem(args, 1) };
    let stacklevel = unsafe { numbers::PyLong_AsSsize_t(sequences::PyTuple_GetItem(args, 2)) };
    EMISSIONS.with(|stored| {
        stored.borrow_mut().push(Emission {
            message: unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned(),
            category: category as usize,
            stacklevel,
        });
    });
    if let Some(observer) = OBSERVER.with(Cell::get) {
        observer();
    }
    if AS_ERROR.with(Cell::get) {
        unsafe { errors::PyErr_SetString(category, text) };
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef(&raw mut Py_None) }
    }
}

pub fn set_as_error(enabled: bool) {
    AS_ERROR.with(|value| value.set(enabled));
}

pub fn set_observer(observer: Option<fn()>) {
    OBSERVER.with(|value| value.set(observer));
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
            MODULE.with(|value| value.set(0));
            CALLABLE.with(|value| value.set(0));
            set_as_error(false);
            set_observer(None);
        }
    }
    let mut module_type = super::StaticType::new();
    module_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    module_type.tp_name = c"WarningModule".as_ptr();
    module_type.tp_getattr = Some(getattr);
    let mut callable_type = super::StaticType::new();
    callable_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    callable_type.tp_name = c"WarningCallable".as_ptr();
    callable_type.tp_call = Some(call);
    let mut module = PyObject {
        ob_refcnt: 1,
        ob_type: module_type.as_ptr(),
    };
    let mut callable = PyObject {
        ob_refcnt: 1,
        ob_type: callable_type.as_ptr(),
    };
    MODULE.with(|value| value.set((&raw mut module) as usize));
    CALLABLE.with(|value| value.set((&raw mut callable) as usize));
    clear();
    set_as_error(false);
    set_observer(None);
    let reset = Reset;
    run();
    drop(reset);
    assert_eq!(module.ob_refcnt, 1);
    assert_eq!(callable.ob_refcnt, 1);
}
