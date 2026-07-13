#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{
    MoltTypeTag, Py_TPFLAGS_BYTES_SUBCLASS, Py_TPFLAGS_UNICODE_SUBCLASS, PyBytes_Type,
    PyExc_UnicodeDecodeError, PyObject, PyTypeObject, PyUnicode_Type,
};
use molt_cpython_abi::hooks::RuntimeHooks;
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

static STRINGS: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

unsafe extern "C" fn alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let stored = Box::leak(bytes.to_vec().into_boxed_slice());
    let handle = MoltObject::from_ptr(stored.as_ptr().cast_mut()).bits();
    STRINGS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(handle, stored);
    ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    handle
}

unsafe extern "C" fn str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let strings = STRINGS.lock().unwrap();
    let Some(bytes) = strings.as_ref().and_then(|values| values.get(&bits)) else {
        return std::ptr::null();
    };
    if !out_len.is_null() {
        unsafe { *out_len = bytes.len() };
    }
    bytes.as_ptr()
}

unsafe extern "C" fn classify_heap(bits: u64) -> u8 {
    if STRINGS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|strings| strings.contains_key(&bits))
    {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn noop_ref(_bits: u64) {}

fn init() {
    INIT.call_once(|| {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
        hooks.alloc_str = alloc_str;
        hooks.str_data = str_data;
        hooks.classify_heap = classify_heap;
        hooks.inc_ref = noop_ref;
        hooks.dec_ref = noop_ref;
        assert!(unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) });
    });
}

#[test]
fn as_utf8_is_cached_and_nul_terminated() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let input = "na?ve".as_bytes();
    let unicode = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
            input.as_ptr().cast(),
            input.len() as isize,
        )
    };
    assert!(!unicode.is_null());

    let mut len = -1;
    let first =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(unicode, &raw mut len) };
    let second = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(unicode) };
    assert_eq!(
        first, second,
        "the object must retain one stable UTF-8 cache"
    );
    assert_eq!(len as usize, input.len());
    assert_eq!(
        unsafe { std::slice::from_raw_parts(first.cast::<u8>(), input.len()) },
        input
    );
    assert_eq!(
        unsafe { *first.add(len as usize) },
        0,
        "buffer[len] must be the C terminator"
    );

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(unicode) };
}

#[test]
fn from_string_and_size_rejects_invalid_utf8_before_allocation() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let invalid = [0xff_u8];
    let unicode = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
            invalid.as_ptr().cast(),
            invalid.len() as isize,
        )
    };
    assert!(unicode.is_null());
    assert!(
        STRINGS
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|strings| strings.values().all(|bytes| *bytes != invalid)),
        "invalid input bytes must never reach the runtime string allocator"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut PyExc_UnicodeDecodeError).cast::<PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn check_accepts_subclasses_while_exact_identity_rejects_them() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let mut unicode_subtype: PyTypeObject = unsafe { std::mem::zeroed() };
    unicode_subtype.tp_flags = Py_TPFLAGS_UNICODE_SUBCLASS;
    unicode_subtype.tp_base = &raw mut PyUnicode_Type;
    let mut unicode = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut unicode_subtype,
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::strings::PyUnicode_Check(&raw mut unicode) },
        1
    );
    assert_ne!(unicode.ob_type, &raw mut PyUnicode_Type);

    let mut bytes_subtype: PyTypeObject = unsafe { std::mem::zeroed() };
    bytes_subtype.tp_flags = Py_TPFLAGS_BYTES_SUBCLASS;
    bytes_subtype.tp_base = &raw mut PyBytes_Type;
    let mut bytes = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut bytes_subtype,
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::strings::PyBytes_Check(&raw mut bytes) },
        1
    );
    assert_ne!(bytes.ob_type, &raw mut PyBytes_Type);
}
