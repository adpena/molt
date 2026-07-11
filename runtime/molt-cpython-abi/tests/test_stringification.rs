//! Mask-proof tests for `PyObject_Str` / `PyObject_Repr` (ledger F4 priority row,
//! `typeobj.rs:1907/1915`). Before the fix both returned the literal
//! `"<molt object>"` for EVERY object (zero-dispatch theater), corrupting `%S`
//! `PyErr_Format`, `PyUnicode_FromFormat`, and every dtype/array string path.
//!
//! These tests install a small but faithful runtime backend (real `alloc_str` /
//! `str_data` / `classify_heap`) so a native `int`/`str` round-trips through the
//! runtime str primitive, and construct genuine *foreign* type objects with
//! `tp_str` / `tp_repr` slots to prove slot dispatch. No result may ever be
//! `"<molt object>"`.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::RuntimeHooks;
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

// ── Faithful mini runtime backend: real native strings ───────────────────────
// Each interned str handle is a genuine `TAG_PTR` `MoltObject` over a leaked
// byte buffer, so `classify_heap` -> Str, `str_data` -> the bytes, and
// `handle_to_pyobj` stamps `ob_type == &PyUnicode_Type`.

static STR_MAP: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);

fn str_map() -> std::sync::MutexGuard<'static, Option<HashMap<u64, &'static [u8]>>> {
    let mut g = STR_MAP.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    // Leak a stable buffer so `str_data` can hand back a valid pointer forever.
    let bytes: Vec<u8> = if data.is_null() || len == 0 {
        Vec::from(&b"\0"[..]) // 1-byte backing so the ptr is non-dangling; len tracked separately
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let handle = MoltObject::from_ptr(leaked.as_ptr() as *mut u8).bits();
    // Store the logically-empty case as an empty slice view.
    let view: &'static [u8] = if len == 0 { &leaked[..0] } else { leaked };
    str_map().as_mut().unwrap().insert(handle, view);
    handle
}

unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    if let Some(&view) = str_map().as_ref().unwrap().get(&bits) {
        if !out_len.is_null() {
            unsafe { *out_len = view.len() };
        }
        return view.as_ptr();
    }
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    ptr::null()
}

unsafe extern "C" fn fake_classify_heap(bits: u64) -> u8 {
    use molt_cpython_abi::abi_types::MoltTypeTag;
    if str_map().as_ref().unwrap().contains_key(&bits) {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

fn install() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_str = fake_alloc_str;
    hooks.str_data = fake_str_data;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// Read the UTF-8 bytes backing a Molt-native `str` result.
unsafe fn read_native_str(py: *mut PyObject) -> Vec<u8> {
    let bits = GLOBAL_BRIDGE
        .lock()
        .pyobj_to_handle(py)
        .map(|identity| identity.as_handle())
        .expect("result must be a bridge-managed native str");
    str_map()
        .as_ref()
        .unwrap()
        .get(&bits)
        .map(|v| v.to_vec())
        .expect("result handle must be a known str")
}

// ── Foreign type-object scaffolding ──────────────────────────────────────────

fn make_type(
    name: *const c_char,
    tp_str: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    tp_repr: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
) -> *mut PyTypeObject {
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = name;
    ty.tp_str = tp_str;
    ty.tp_repr = tp_repr;
    Box::into_raw(ty)
}

/// A genuine foreign instance: a real dereferenceable `PyObject` whose `ob_type`
/// is a foreign type. NOT bridge-registered, so `pyobj_to_handle` -> None (the
/// foreign path).
fn make_instance(ty: *mut PyTypeObject) -> *mut PyObject {
    let obj = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    });
    Box::into_raw(obj)
}

unsafe extern "C" fn foreign_str(_o: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"FOREIGN_STR".as_ptr()) }
}

unsafe extern "C" fn foreign_repr(_o: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"foreign_repr()".as_ptr()) }
}

unsafe extern "C" fn foreign_str_returns_int(_o: *mut PyObject) -> *mut PyObject {
    // A slot that lies and returns a non-str — CPython raises TypeError.
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) }
}

// ===========================================================================
// Native scalars route through the runtime str/repr primitive (no theater).
// ===========================================================================

#[test]
fn native_int_str_is_the_decimal_digits() {
    install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(!s.is_null(), "str(42) must not be NULL");
    assert_eq!(unsafe { read_native_str(s) }, b"42");
}

#[test]
fn native_int_repr_is_the_decimal_digits() {
    install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-17) };
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(py) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"-17");
}

#[test]
fn native_str_str_is_identity_passthrough() {
    install();
    let s = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hello".as_ptr()) };
    assert!(!s.is_null());
    // str(s) is s — same object (CPython PyUnicode_CheckExact fast path).
    let out = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(s) };
    assert_eq!(out, s, "str(str) must return the same object");
}

#[test]
fn native_str_repr_is_quoted() {
    install();
    let s = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hi".as_ptr()) };
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(s) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"'hi'");
}

// ===========================================================================
// Foreign objects dispatch their OWN tp_str / tp_repr slots.
// ===========================================================================

#[test]
fn foreign_object_str_dispatches_tp_str() {
    install();
    let ty = make_type(c"Widget".as_ptr(), Some(foreign_str), Some(foreign_repr));
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(!s.is_null());
    let bytes = unsafe { read_native_str(s) };
    assert_eq!(bytes, b"FOREIGN_STR");
    assert_ne!(bytes, b"<molt object>", "must not be the old theater constant");
}

#[test]
fn foreign_object_repr_dispatches_tp_repr() {
    install();
    let ty = make_type(c"Widget".as_ptr(), Some(foreign_str), Some(foreign_repr));
    let inst = make_instance(ty);
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(inst) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"foreign_repr()");
}

#[test]
fn foreign_str_falls_back_to_repr_when_tp_str_null() {
    install();
    // tp_str == NULL: CPython PyObject_Str falls back to PyObject_Repr -> tp_repr.
    let ty = make_type(c"Widget".as_ptr(), None, Some(foreign_repr));
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(!s.is_null());
    assert_eq!(unsafe { read_native_str(s) }, b"foreign_repr()");
}

#[test]
fn foreign_repr_default_is_type_name_and_address() {
    install();
    // tp_repr == NULL: CPython default "<%s object at %p>".
    let ty = make_type(c"gadget".as_ptr(), None, None);
    let inst = make_instance(ty);
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(inst) };
    assert!(!r.is_null());
    let bytes = unsafe { read_native_str(r) };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("<gadget object at 0x"), "got {text:?}");
    assert!(text.ends_with('>'), "got {text:?}");
    assert_ne!(text, "<molt object>");
}

#[test]
fn foreign_str_slot_returning_non_string_raises_typeerror() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let ty = make_type(c"Liar".as_ptr(), Some(foreign_str_returns_int), None);
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(s.is_null(), "a non-str tp_str result must fail, not pass through");
    let err = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!err.is_null(), "must set TypeError on non-string __str__ result");
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn null_object_str_is_angle_null() {
    install();
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(ptr::null_mut()) };
    assert!(!s.is_null());
    assert_eq!(unsafe { read_native_str(s) }, b"<NULL>");
}
