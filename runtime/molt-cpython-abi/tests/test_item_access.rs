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

use molt_cpython_abi::abi_types::{
    MoltTypeTag, PyMappingMethods, PyObject, PyTypeObject,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

// A hook table whose `classify_heap` reports Dict (so an is_ptr handle takes the
// native dict lane in PyObject_GetItem), `dict_get` always MISSES (returns 0),
// and `alloc_str` succeeds (so PyUnicode_FromString mints a real key). Every
// other hook stays at STUB. classify_heap->Dict only ever fires for a bridged
// is_ptr `o`; the foreign-mapping test's receiver is unbridged, so it is
// unaffected.
static STR_HANDLE: AtomicU64 = AtomicU64::new(0x6900_0000);

unsafe extern "C" fn dict_classify(_bits: u64) -> u8 {
    MoltTypeTag::Dict as u8
}

unsafe extern "C" fn dict_get_miss(_d: u64, _k: u64) -> u64 {
    0
}

unsafe extern "C" fn fake_alloc_str(_data: *const u8, _len: usize) -> u64 {
    STR_HANDLE.fetch_add(0x10, Ordering::Relaxed)
}

fn init_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = dict_classify;
    hooks.dict_get = dict_get_miss;
    hooks.alloc_str = fake_alloc_str;
    // Idempotent: the first test to run installs the shared table; the rest
    // observe it. Both tests need exactly this table.
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// (c) A native dict miss must raise `KeyError` with the key as its argument
/// (CPython `dict_subscript`), never the prior bare NULL with no exception.
#[test]
fn get_item_native_dict_miss_raises_keyerror_with_key() {
    init_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // A genuine is_ptr handle -> classify_heap reports Dict for it.
    let backing: Box<u8> = Box::new(0);
    let dict_ptr = Box::into_raw(backing);
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let dict_obj = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(dict_bits) };
    // A native int key so `PyErr_SetObject(KeyError, key)` can format its value.
    let key = unsafe {
        GLOBAL_BRIDGE
            .lock()
            .handle_to_pyobj(MoltObject::from_int(4242).bits())
    };

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
                &raw mut molt_cpython_abi::abi_types::PyExc_KeyError,
            )
        },
        1,
        "a dict miss must raise KeyError specifically"
    );
    // KeyError's argument is the key: PyErr_SetObject stores the key's str().
    let msg = molt_cpython_abi::api::errors::take_current_error_message();
    assert_eq!(
        msg.as_deref(),
        Some("4242"),
        "KeyError must carry the key (4242) as its value, got {msg:?}"
    );

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { drop(Box::from_raw(dict_ptr)) };
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
                &raw mut molt_cpython_abi::abi_types::PyExc_KeyError,
                key,
            );
        }
        return ptr::null_mut();
    }
    &raw mut FAKE_VALUE
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
        got,
        &raw mut FAKE_VALUE,
        "PyMapping_GetItemString must dispatch the foreign mapping's mp_subscript"
    );

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
                &raw mut molt_cpython_abi::abi_types::PyExc_KeyError,
            )
        },
        1,
        "a missing key on a foreign mapping must raise KeyError"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

/// The `PyObject_Bytes` fabricated-empty-`b''` fake is gone: a non-bytes object
/// with no `__bytes__` must raise an honest `TypeError` and record the site,
/// never a silently-wrong empty bytes value (M05 poison). Needs the wired
/// `alloc_str` so the `__bytes__` lookup reaches the honest fallback.
#[test]
fn object_bytes_non_bytes_without_dunder_raises_typeerror() {
    init_hooks();
    let _ = molt_cpython_abi::capi_trace::take_last_silent_failure();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_name = c"opaque".as_ptr();
    let mut obj = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut ty,
    };
    let result = unsafe { molt_cpython_abi::api::object::PyObject_Bytes(&raw mut obj) };
    assert!(
        result.is_null(),
        "PyObject_Bytes must NOT fabricate an empty b'' for a non-bytes object"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                &raw mut molt_cpython_abi::abi_types::PyExc_TypeError,
            )
        },
        1,
        "PyObject_Bytes on a non-bytes object without __bytes__ must raise TypeError"
    );
    let recorded = molt_cpython_abi::capi_trace::take_last_silent_failure();
    assert!(
        recorded.as_deref().unwrap_or("").contains("PyObject_Bytes"),
        "expected PyObject_Bytes on the silent-failure surface, got {recorded:?}"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
