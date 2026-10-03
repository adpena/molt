//! Integration gate tests for the item-access protocol that need the runtime
//! hook boundary (a native dict for the `KeyError`-on-miss path; a working
//! `alloc_str` for `PyMapping_GetItemString`'s key). These live in their own
//! test binary so the process-global `RUNTIME_HOOKS` table they install is
//! isolated from the crate's other tests.
//!
//! Companion to the `object::item_access_slot_tests` unit tests, which cover the
//! foreign `mp_subscript` / `sq_item` / `mp_ass_subscript` dispatch on STUB
//! hooks. Here we exercise:
//!   (c) native dict miss  => KeyError with the key (PyObject_GetItem)
//!   (e) foreign mapping    => PyMapping_GetItemString routes through
//!                             PyObject_GetItem (works, and propagates KeyError)

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{MoltTypeTag, PyMappingMethods, PyObject, PyTypeObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::Ordering;

// Managed subscription reaches object_get_item; physical PyDict accessors
// separately use dict_get. This fixture explicitly supplies the former and
// uses shared dictionary/string custody for native exception construction.
unsafe extern "C" fn dict_get_item_miss(
    dict: u64,
    key: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    assert_eq!(
        unsafe { support::fake_runtime::classify_heap(dict) },
        MoltTypeTag::Dict as u8
    );
    let key = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(key) };
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetObject(
            (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast(),
            key,
        );
    }
    molt_cpython_abi::hooks::OwnedHandleResult::error()
}

fn init_hooks() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    hooks.object_get_item = dict_get_item_miss;
    support::prepare_abi_test_thread(hooks);
}

/// (c) A native dict miss must raise `KeyError` with the key as its argument
/// (CPython `dict_subscript`), never the prior bare NULL with no exception.
#[test]
fn get_item_native_dict_miss_raises_keyerror_with_key() {
    init_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let dict_obj = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    assert!(!dict_obj.is_null());
    // A native int key so `PyErr_SetObject(KeyError, key)` can format its value.
    let key = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(MoltObject::from_int(4242).bits()) };

    let result = unsafe { molt_cpython_abi::api::object::PyObject_GetItem(dict_obj, key) };
    assert!(
        result.is_null(),
        "a dict miss must return NULL (the failure sentinel)"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL from a dict miss must leave a pending exception (KeyError)"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>(),
            )
        },
        1,
        "a dict miss must raise KeyError specifically"
    );
    // KeyError's argument is the key: PyErr_SetObject stores the key's str().
    let msg = support::take_current_error_text();
    assert_eq!(
        msg.as_deref(),
        Some("4242"),
        "KeyError must carry the key (4242) as its value, got {msg:?}"
    );

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(dict_obj);
        molt_cpython_abi::api::refcount::Py_DECREF(key);
    }
}

// ── Foreign mapping for (e): its mp_subscript returns FAKE_VALUE unless the
//    MISS toggle is set, in which case it raises KeyError + returns NULL. ──

static mut FAKE_VALUE: PyObject = PyObject {
    ob_refcnt: 1,
    ob_type: ptr::null_mut(),
};

static MISS_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn foreign_map_subscript(_o: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    if MISS_MODE.load(Ordering::SeqCst) {
        // Model a real mapping's missing-key path: KeyError with the key.
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>(),
                key,
            );
        }
        return ptr::null_mut();
    }
    unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut FAKE_VALUE) }
}

/// (e) `PyMapping_GetItemString` on a FOREIGN mapping must route through
/// `PyObject_GetItem` (dispatching `mp_subscript`), so it works for any mapping;
/// and a missing key must surface the mapping's `KeyError`. The prior route
/// through `PyDict_GetItem` returned a bare NULL for a non-dict mapping, never
/// invoking the slot.
#[test]
fn mapping_getitemstring_routes_foreign_mapping_through_getitem() {
    init_hooks();

    let mut mapping: PyMappingMethods = unsafe { std::mem::zeroed() };
    mapping.mp_subscript = foreign_map_subscript as *mut c_void;
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_as_mapping = (&raw mut mapping).cast::<c_void>();
    ty.tp_name = c"foreign_mapping".as_ptr();
    let mut map = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut ty,
    };
    let map_ptr = &raw mut map;
    let name = c"anykey";

    // Works: present key -> the mapping's own value via mp_subscript.
    MISS_MODE.store(false, Ordering::SeqCst);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let got = unsafe {
        molt_cpython_abi::api::abstract_mapping::PyMapping_GetItemString(map_ptr, name.as_ptr())
    };
    assert_eq!(
        got, &raw mut FAKE_VALUE,
        "PyMapping_GetItemString must dispatch the foreign mapping's mp_subscript"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(got) };
    assert_eq!(unsafe { FAKE_VALUE.ob_refcnt }, 1);

    // Missing key: the mapping's KeyError must propagate (not a silent NULL).
    MISS_MODE.store(true, Ordering::SeqCst);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let miss = unsafe {
        molt_cpython_abi::api::abstract_mapping::PyMapping_GetItemString(map_ptr, name.as_ptr())
    };
    assert!(
        miss.is_null(),
        "a missing key must return NULL (the failure sentinel)"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>(),
            )
        },
        1,
        "a missing key on a foreign mapping must raise KeyError"
    );
    let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
    unsafe {
        let args = molt_cpython_abi::api::errors::PyException_GetArgs(error.value);
        assert!(!args.is_null());
        assert_eq!(molt_cpython_abi::api::sequences::PyTuple_Size(args), 1);
        let key = molt_cpython_abi::api::sequences::PyTuple_GetItem(args, 0);
        let text = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(key);
        assert!(!text.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(text),
            name,
            "KeyError retains the string key"
        );
        molt_cpython_abi::api::refcount::Py_DECREF(args);
    }
    drop(error);
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}
