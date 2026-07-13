//! Mask-proof tests for the F4 ledger rows in strings.rs, memory.rs,
//! capsule.rs, contextvars.rs, imports.rs, weakref.rs, buffer.rs, datetime.rs.
//! Each test pins the CPython 3.12 semantic the fix implements; none may pass
//! against the old sentinel/theater bodies.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_buffer, PyObject, PyTypeObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{BorrowedHandleResult, RuntimeHooks};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::Mutex;

// ── Fake runtime backend: strings, dicts, modules, imports ───────────────────

static STR_MAP: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static DICT_MAP: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);
static SYS_MODULES: Mutex<u64> = Mutex::new(0);
static NEXT_MODULE: Mutex<u64> = Mutex::new(0);

fn str_map() -> std::sync::MutexGuard<'static, Option<HashMap<u64, &'static [u8]>>> {
    let mut g = STR_MAP.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}
fn dict_map() -> std::sync::MutexGuard<'static, Option<HashMap<u64, HashMap<u64, u64>>>> {
    let mut g = DICT_MAP.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

fn leak_handle(bytes: &[u8]) -> (u64, &'static [u8]) {
    let data: Vec<u8> = if bytes.is_empty() {
        vec![0]
    } else {
        bytes.to_vec()
    };
    let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
    let handle = MoltObject::from_ptr(leaked.as_ptr() as *mut u8).bits();
    let view: &'static [u8] = if bytes.is_empty() {
        &leaked[..0]
    } else {
        leaked
    };
    (handle, view)
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes: &[u8] = if data.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    // Intern by content so equal names get equal handles (dict keys).
    static INTERN: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);
    let mut g = INTERN.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    if let Some(&h) = g.as_ref().unwrap().get(bytes) {
        return h;
    }
    let (handle, view) = leak_handle(bytes);
    str_map().as_mut().unwrap().insert(handle, view);
    g.as_mut().unwrap().insert(bytes.to_vec(), handle);
    handle
}

unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    if let Some(&v) = str_map().as_ref().unwrap().get(&bits) {
        unsafe { *out_len = v.len() };
        return v.as_ptr();
    }
    unsafe { *out_len = 0 };
    ptr::null()
}

unsafe extern "C" fn fake_bytes_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    unsafe { *out_len = 0 };
    ptr::null()
}

unsafe extern "C" fn fake_classify_heap(bits: u64) -> u8 {
    use molt_cpython_abi::abi_types::MoltTypeTag;
    if str_map().as_ref().unwrap().contains_key(&bits) {
        MoltTypeTag::Str as u8
    } else if dict_map().as_ref().unwrap().contains_key(&bits) {
        MoltTypeTag::Dict as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let (handle, _) = leak_handle(b"d");
    dict_map().as_mut().unwrap().insert(handle, HashMap::new());
    handle
}
unsafe extern "C" fn fake_dict_get(dict: u64, key: u64) -> BorrowedHandleResult {
    match dict_map()
        .as_ref()
        .unwrap()
        .get(&dict)
        .and_then(|m| m.get(&key).copied())
    {
        Some(value) => BorrowedHandleResult::ok(value),
        None => BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fake_dict_set(dict: u64, key: u64, val: u64) -> i32 {
    if let Some(m) = dict_map().as_mut().unwrap().get_mut(&dict) {
        m.insert(key, val);
    }
    0
}

unsafe extern "C" fn fake_sys_get_object_borrowed(
    data: *const u8,
    len: usize,
) -> BorrowedHandleResult {
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    if name == b"modules" {
        let mut g = SYS_MODULES.lock().unwrap();
        if *g == 0 {
            *g = unsafe { fake_alloc_dict() };
        }
        return BorrowedHandleResult::ok(*g);
    }
    BorrowedHandleResult::missing()
}

unsafe extern "C" fn fake_alloc_module(_data: *const u8, _len: usize) -> u64 {
    let mut g = NEXT_MODULE.lock().unwrap();
    let (handle, _) = leak_handle(b"m");
    *g = handle;
    handle
}

unsafe extern "C" fn fake_import_add_module_borrowed(
    data: *const u8,
    len: usize,
) -> BorrowedHandleResult {
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    let modules = {
        let mut slot = SYS_MODULES.lock().unwrap();
        if *slot == 0 {
            *slot = unsafe { fake_alloc_dict() };
        }
        *slot
    };
    let key = unsafe { fake_alloc_str(name.as_ptr(), name.len()) };
    if let Some(module) = dict_map()
        .as_ref()
        .unwrap()
        .get(&modules)
        .and_then(|entries| entries.get(&key).copied())
    {
        return BorrowedHandleResult::ok(module);
    }
    let module = unsafe { fake_alloc_module(name.as_ptr(), name.len()) };
    unsafe { fake_dict_set(modules, key, module) };
    BorrowedHandleResult::ok(module)
}

unsafe extern "C" fn fake_import_module_fails(_data: *const u8, _len: usize) -> u64 {
    0 // every import fails — the mirror-error path must fire
}

unsafe extern "C" fn noop_ref(_: u64) {}

fn install() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_str = fake_alloc_str;
    hooks.str_data = fake_str_data;
    hooks.bytes_data = fake_bytes_data;
    hooks.classify_heap = fake_classify_heap;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_get = fake_dict_get;
    hooks.dict_set = fake_dict_set;
    hooks.sys_get_object_borrowed = fake_sys_get_object_borrowed;
    hooks.alloc_module = fake_alloc_module;
    hooks.import_add_module_borrowed = fake_import_add_module_borrowed;
    hooks.import_module = fake_import_module_fails;
    hooks.inc_ref = noop_ref;
    hooks.dec_ref = noop_ref;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

unsafe fn str_obj(text: &str) -> *mut PyObject {
    unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
            text.as_ptr().cast(),
            text.len() as isize,
        )
    }
}

unsafe fn read_str(py: *mut PyObject) -> Vec<u8> {
    let bits = GLOBAL_BRIDGE
        .pyobj_to_handle(py)
        .map(|identity| identity.as_handle())
        .expect("bridge str handle");
    str_map().as_ref().unwrap().get(&bits).unwrap().to_vec()
}

unsafe fn err_clear() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
unsafe fn err_set() -> bool {
    !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
}

// ===========================================================================
// strings.rs
// ===========================================================================

#[test]
fn as_utf8_non_str_is_null_with_typeerror_not_empty_string() {
    install();
    unsafe { err_clear() };
    let n = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(3) };
    let p = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(n) };
    assert!(
        p.is_null(),
        "non-str must yield NULL, not a fabricated \"\""
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn unicode_compare_non_str_sets_typeerror() {
    install();
    unsafe { err_clear() };
    let s = unsafe { str_obj("a") };
    let n = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let r = unsafe { molt_cpython_abi::api::strings::PyUnicode_Compare(s, n) };
    assert_eq!(r, -1);
    assert!(
        unsafe { err_set() },
        "-1 is also a valid ordering; must carry TypeError"
    );
    unsafe { err_clear() };
}

#[test]
fn as_ascii_string_non_ascii_raises_unicode_encode_error() {
    install();
    unsafe { err_clear() };
    let s = unsafe { str_obj("café") };
    let r = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsASCIIString(s) };
    assert!(r.is_null());
    assert!(
        unsafe { err_set() },
        "must raise UnicodeEncodeError, not bare NULL"
    );
    unsafe { err_clear() };
}

#[test]
fn replace_empty_needle_inserts_at_codepoint_boundaries() {
    install();
    let text = unsafe { str_obj("aé") };
    let needle = unsafe { str_obj("") };
    let dash = unsafe { str_obj("-") };
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Replace(text, needle, dash, -1) };
    assert!(!out.is_null());
    let bytes = unsafe { read_str(out) };
    // CPython: '-a-é-' — never a '-' inside the two-byte é sequence.
    assert_eq!(String::from_utf8(bytes).unwrap(), "-a-é-");
}

#[test]
fn tailmatch_uses_codepoint_indices_and_cpython_direction() {
    install();
    // "ééx": start=2 in CODE POINTS selects "x" (byte-offset math would slice
    // inside the second é and never match).
    let text = unsafe { str_obj("ééx") };
    let needle = unsafe { str_obj("x") };
    let starts =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_Tailmatch(text, needle, 2, 3, -1) };
    assert_eq!(starts, 1, "code-point window [2,3) must equal \"x\"");
    // direction > 0 is the SUFFIX match in CPython.
    let ends =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_Tailmatch(text, needle, 0, 3, 1) };
    assert_eq!(ends, 1);
    let not_prefix =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_Tailmatch(text, needle, 0, 3, -1) };
    assert_eq!(not_prefix, 0, "direction<=0 is the PREFIX match");
}

#[test]
fn from_encoded_object_rejects_str_input() {
    install();
    unsafe { err_clear() };
    let s = unsafe { str_obj("abc") };
    let r = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromEncodedObject(
            s,
            c"utf-8".as_ptr(),
            ptr::null(),
        )
    };
    assert!(r.is_null(), "CPython: 'decoding str is not supported'");
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn decode_utf8_invalid_bytes_raise_unicode_decode_error() {
    install();
    unsafe { err_clear() };
    let bad = [0xffu8, 0x30];
    let r = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_DecodeUTF8(
            bad.as_ptr().cast(),
            bad.len() as isize,
            ptr::null(),
        )
    };
    assert!(r.is_null(), "malformed UTF-8 must not be silently accepted");
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn decode_dispatches_latin1_not_utf8() {
    install();
    let bytes = [0xe9u8]; // é in latin-1; INVALID as UTF-8
    let r = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_Decode(
            bytes.as_ptr().cast(),
            1,
            c"latin-1".as_ptr(),
            ptr::null(),
        )
    };
    assert!(!r.is_null(), "latin-1 decode of 0xE9 must succeed");
    assert_eq!(String::from_utf8(unsafe { read_str(r) }).unwrap(), "é");
    // Unknown encoding fails closed with LookupError.
    unsafe { err_clear() };
    let unknown = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_Decode(
            bytes.as_ptr().cast(),
            1,
            c"cp437".as_ptr(),
            ptr::null(),
        )
    };
    assert!(unknown.is_null());
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn format_applies_width_precision_and_hex() {
    install();
    let fmt = unsafe { str_obj("[%5s][%-4d][%.2s][%#x]") };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(4) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(args, 0, str_obj("ab"));
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            1,
            molt_cpython_abi::api::numbers::PyLong_FromLong(7),
        );
        molt_cpython_abi::api::sequences::PyTuple_SetItem(args, 2, str_obj("xyz"));
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            3,
            molt_cpython_abi::api::numbers::PyLong_FromLong(255),
        );
    }
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Format(fmt, args) };
    assert!(!out.is_null());
    assert_eq!(
        String::from_utf8(unsafe { read_str(out) }).unwrap(),
        "[   ab][7   ][xy][0xff]",
        "width/precision/flags were previously parsed then DISCARDED"
    );
}

#[test]
#[allow(clippy::approx_constant)] // 3.14159 deliberately exercises %.2f truncation, not math::PI
fn format_float_conversions() {
    install();
    let fmt = unsafe { str_obj("%.2f|%e|%g") };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            0,
            molt_cpython_abi::api::numbers::PyFloat_FromDouble(3.14159),
        );
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            1,
            molt_cpython_abi::api::numbers::PyFloat_FromDouble(100.0),
        );
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            2,
            molt_cpython_abi::api::numbers::PyFloat_FromDouble(0.5),
        );
    }
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Format(fmt, args) };
    assert!(!out.is_null());
    assert_eq!(
        String::from_utf8(unsafe { read_str(out) }).unwrap(),
        "3.14|1.000000e+02|0.5"
    );
}

#[test]
fn format_not_enough_and_surplus_args_are_typeerrors() {
    install();
    unsafe { err_clear() };
    let fmt = unsafe { str_obj("%s %s") };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(args, 0, str_obj("a")) };
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Format(fmt, args) };
    assert!(
        out.is_null(),
        "argument exhaustion must raise, not truncate"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };

    let fmt2 = unsafe { str_obj("%s") };
    let args2 = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(2) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(args2, 0, str_obj("a"));
        molt_cpython_abi::api::sequences::PyTuple_SetItem(args2, 1, str_obj("b"));
    }
    let out2 = unsafe { molt_cpython_abi::api::strings::PyUnicode_Format(fmt2, args2) };
    assert!(out2.is_null(), "surplus args must raise TypeError");
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn format_d_of_non_number_is_typeerror_not_minus_one() {
    install();
    unsafe { err_clear() };
    let fmt = unsafe { str_obj("%d") };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(args, 0, str_obj("nope")) };
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Format(fmt, args) };
    assert!(out.is_null(), "old body appended '-1' and continued");
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn join_concatenates_with_separator_and_rejects_non_str_items() {
    install();
    let sep = unsafe { str_obj(", ") };
    let list = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(list, 0, str_obj("a"));
        molt_cpython_abi::api::sequences::PyTuple_SetItem(list, 1, str_obj("b"));
        molt_cpython_abi::api::sequences::PyTuple_SetItem(list, 2, str_obj("c"));
    }
    let out = unsafe { molt_cpython_abi::api::strings::PyUnicode_Join(sep, list) };
    assert!(!out.is_null(), "join was a stub returning ''");
    assert_eq!(
        String::from_utf8(unsafe { read_str(out) }).unwrap(),
        "a, b, c"
    );

    unsafe { err_clear() };
    let bad = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            bad,
            0,
            molt_cpython_abi::api::numbers::PyLong_FromLong(1),
        );
    }
    let out2 = unsafe { molt_cpython_abi::api::strings::PyUnicode_Join(sep, bad) };
    assert!(out2.is_null());
    assert!(unsafe { err_set() }, "non-str item must be a TypeError");
    unsafe { err_clear() };
}

// ===========================================================================
// memory.rs
// ===========================================================================

#[test]
fn zero_size_allocations_return_unique_non_null() {
    install();
    let p = unsafe { molt_cpython_abi::api::memory::PyMem_Malloc(0) };
    assert!(!p.is_null(), "Malloc(0) must be a unique non-NULL pointer");
    let q = unsafe { molt_cpython_abi::api::memory::PyMem_Realloc(p, 0) };
    assert!(!q.is_null(), "Realloc(p, 0) must not free-and-return-NULL");
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(q) };
    let c = unsafe { molt_cpython_abi::api::memory::PyMem_Calloc(0, 8) };
    assert!(!c.is_null());
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(c) };
}

#[test]
fn object_init_null_sets_memory_error() {
    install();
    unsafe { err_clear() };
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    let r = unsafe { molt_cpython_abi::api::memory::PyObject_Init(ptr::null_mut(), &mut ty) };
    assert!(r.is_null());
    assert!(unsafe { err_set() }, "NULL op must observe MemoryError");
    unsafe { err_clear() };
}

#[test]
fn object_init_increfs_heap_type() {
    install();
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_HEAPTYPE;
    ty.ob_base.ob_base.ob_refcnt = 5;
    let mut obj = PyObject {
        ob_refcnt: 0,
        ob_type: ptr::null_mut(),
    };
    unsafe { molt_cpython_abi::api::memory::PyObject_Init(&mut obj, &mut ty) };
    assert_eq!(
        ty.ob_base.ob_base.ob_refcnt, 6,
        "a HEAPTYPE instance owns a reference to its type"
    );
}

#[test]
fn recursive_call_guard_trips_with_recursion_error() {
    install();
    unsafe { err_clear() };
    let mut tripped = false;
    let mut entered = 0usize;
    for _ in 0..2000 {
        if unsafe { molt_cpython_abi::api::memory::Py_EnterRecursiveCall(c" in test".as_ptr()) }
            != 0
        {
            tripped = true;
            break;
        }
        entered += 1;
    }
    assert!(tripped, "the guard must trip (old body never did)");
    assert!(unsafe { err_set() }, "trip must set RecursionError");
    unsafe { err_clear() };
    for _ in 0..entered {
        unsafe { molt_cpython_abi::api::memory::Py_LeaveRecursiveCall() };
    }
}

unsafe extern "C" fn resurrecting_finalizer(op: *mut PyObject) {
    // Simulate a finalizer storing a new reference to the dying object.
    unsafe { (*op).ob_refcnt += 1 };
}

#[test]
fn finalizer_resurrection_aborts_the_free() {
    install();
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_finalize = Some(resurrecting_finalizer);
    let mut obj = PyObject {
        ob_refcnt: 0,
        ob_type: &mut ty,
    };
    let rc = unsafe { molt_cpython_abi::api::memory::PyObject_CallFinalizerFromDealloc(&mut obj) };
    assert_eq!(
        rc, -1,
        "resurrection must abort the free (old body returned 0)"
    );
}

// ===========================================================================
// weakref.rs
// ===========================================================================

#[test]
fn weakref_getobject_fails_loud_never_fabricates_none() {
    install();
    unsafe { err_clear() };
    let n = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let r = unsafe { molt_cpython_abi::api::weakref::PyWeakref_GetObject(n) };
    assert!(
        r.is_null(),
        "old body returned a fabricated Py_None referent"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

// ===========================================================================
// contextvars.rs
// ===========================================================================

#[test]
fn contextvar_empty_name_is_legal() {
    install();
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"".as_ptr(), ptr::null_mut())
    };
    assert!(!var.is_null(), "ContextVar('') is legal in CPython");
}

#[test]
fn contextvar_get_caller_default_beats_var_default() {
    install();
    let var_default = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"v1".as_ptr(), var_default)
    };
    let caller_default = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(20) };
    let mut out: *mut PyObject = ptr::null_mut();
    let rc = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(var, caller_default, &mut out)
    };
    assert_eq!(rc, 0);
    assert_eq!(
        out, caller_default,
        "the caller's def argument wins unconditionally over var_default"
    );
}

#[test]
fn contextvar_get_no_value_is_success_with_null() {
    install();
    unsafe { err_clear() };
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"v2".as_ptr(), ptr::null_mut())
    };
    let mut out: *mut PyObject = usize::MAX as *mut PyObject;
    let rc = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(var, ptr::null_mut(), &mut out)
    };
    assert_eq!(
        rc, 0,
        "C API returns SUCCESS for no-value (never LookupError)"
    );
    assert!(out.is_null());
    assert!(!unsafe { err_set() });
}

#[test]
fn contextvar_set_returns_token_and_reset_restores() {
    install();
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"v3".as_ptr(), ptr::null_mut())
    };
    let v1 = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(111) };
    let token = unsafe { molt_cpython_abi::api::contextvars::PyContextVar_Set(var, v1) };
    assert!(!token.is_null());
    assert_ne!(token, v1, "Set returns a TOKEN, never the value");
    assert_ne!(
        token,
        &raw mut molt_cpython_abi::abi_types::Py_None,
        "Set must not fabricate Py_None as a token"
    );
    // The bound value is observable via Get.
    let mut out: *mut PyObject = ptr::null_mut();
    unsafe { molt_cpython_abi::api::contextvars::PyContextVar_Get(var, ptr::null_mut(), &mut out) };
    assert_eq!(out, v1);
    // Reset consumes the token and restores the unbound state.
    let rc = unsafe { molt_cpython_abi::api::contextvars::PyContextVar_Reset(var, token) };
    assert_eq!(rc, 0);
    let mut out2: *mut PyObject = usize::MAX as *mut PyObject;
    unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(var, ptr::null_mut(), &mut out2)
    };
    assert!(out2.is_null(), "Reset(MISSING token) must unbind the var");
    // A second Reset with the same token is a RuntimeError.
    unsafe { err_clear() };
    let rc2 = unsafe { molt_cpython_abi::api::contextvars::PyContextVar_Reset(var, token) };
    assert_eq!(rc2, -1);
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn contextvar_get_rejects_non_contextvar_without_ducktyping() {
    install();
    unsafe { err_clear() };
    let n = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let mut out: *mut PyObject = ptr::null_mut();
    let rc = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(n, ptr::null_mut(), &mut out)
    };
    assert_eq!(
        rc, -1,
        "non-exact ContextVar must be a TypeError (no duck-typing)"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

// ===========================================================================
// imports.rs
// ===========================================================================

#[test]
fn failed_import_always_leaves_a_pending_exception() {
    install();
    unsafe { err_clear() };
    let m = unsafe { molt_cpython_abi::api::imports::PyImport_ImportModule(c"nope".as_ptr()) };
    assert!(m.is_null());
    assert!(
        unsafe { err_set() },
        "NULL from PyImport_ImportModule must carry PyErr_Occurred() != NULL"
    );
    unsafe { err_clear() };
}

#[test]
fn add_module_creates_and_registers_in_sys_modules() {
    install();
    unsafe { err_clear() };
    let first =
        unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"fresh_mod".as_ptr()) };
    assert!(
        !first.is_null(),
        "AddModule was an unconditional ImportError stub"
    );
    let again =
        unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"fresh_mod".as_ptr()) };
    assert_eq!(
        first, again,
        "second AddModule returns the registered module"
    );
}

#[test]
fn get_module_dict_is_backed_by_sys_modules() {
    install();
    let d1 = unsafe { molt_cpython_abi::api::imports::PyImport_GetModuleDict() };
    assert!(!d1.is_null());
    let bits = GLOBAL_BRIDGE
        .pyobj_to_handle(d1)
        .map(|identity| identity.as_handle())
        .expect("dict handle");
    assert_eq!(
        bits,
        *SYS_MODULES.lock().unwrap(),
        "GetModuleDict must return the runtime's real sys.modules, not a detached dict"
    );
}

// ===========================================================================
// buffer.rs
// ===========================================================================

static GETBUFFER_CALLS: Mutex<usize> = Mutex::new(0);

unsafe extern "C" fn fake_bf_getbuffer(
    obj: *mut PyObject,
    view: *mut Py_buffer,
    _flags: c_int,
) -> c_int {
    *GETBUFFER_CALLS.lock().unwrap() += 1;
    unsafe {
        std::ptr::write_bytes(view, 0, 1);
        (*view).obj = obj;
        (*view).len = 4;
        (*view).itemsize = 1;
        (*view).readonly = 1;
    }
    0
}

fn foreign_buffer_type() -> *mut PyTypeObject {
    let procs = Box::new(molt_cpython_abi::abi_types::PyBufferProcs {
        bf_getbuffer: fake_bf_getbuffer as *mut std::ffi::c_void,
        bf_releasebuffer: ptr::null_mut(),
    });
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"Exporter".as_ptr();
    ty.tp_as_buffer = Box::into_raw(procs).cast();
    Box::into_raw(ty)
}

fn foreign_instance(ty: *mut PyTypeObject) -> *mut PyObject {
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    }))
}

#[test]
fn get_buffer_dispatches_foreign_bf_getbuffer() {
    install();
    let ty = foreign_buffer_type();
    let inst = foreign_instance(ty);
    let before = *GETBUFFER_CALLS.lock().unwrap();
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe { molt_cpython_abi::api::buffer::PyObject_GetBuffer(inst, &mut view, 0) };
    assert_eq!(rc, 0, "the installed bf_getbuffer slot must be dispatched");
    assert_eq!(*GETBUFFER_CALLS.lock().unwrap(), before + 1);
    assert_eq!(view.len, 4);
}

#[test]
fn get_buffer_without_slot_is_typeerror() {
    install();
    unsafe { err_clear() };
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"NoBuffer".as_ptr();
    let inst = foreign_instance(Box::into_raw(ty));
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe { molt_cpython_abi::api::buffer::PyObject_GetBuffer(inst, &mut view, 0) };
    assert_eq!(rc, -1);
    assert!(
        unsafe { err_set() },
        "no-slot failure is TypeError, not BufferError"
    );
    unsafe { err_clear() };
}

#[test]
fn check_buffer_is_pure_and_side_effect_free() {
    install();
    unsafe { err_clear() };
    // Foreign with slot -> 1; without -> 0; and a pending exception SURVIVES.
    let with = foreign_instance(foreign_buffer_type());
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"Plain".as_ptr();
    let without = foreign_instance(Box::into_raw(ty));
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
            c"pending".as_ptr(),
        );
    }
    let calls_before = *GETBUFFER_CALLS.lock().unwrap();
    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyObject_CheckBuffer(with) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyObject_CheckBuffer(without) },
        0
    );
    assert_eq!(
        *GETBUFFER_CALLS.lock().unwrap(),
        calls_before,
        "CheckBuffer must not ACQUIRE the buffer"
    );
    assert!(
        unsafe { err_set() },
        "CheckBuffer must not clear the error indicator"
    );
    unsafe { err_clear() };
}

#[test]
fn is_contiguous_suboffsets_and_empty() {
    install();
    let mut suboffsets: [isize; 1] = [0];
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    view.len = 8;
    view.itemsize = 1;
    view.suboffsets = suboffsets.as_mut_ptr();
    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyBuffer_IsContiguous(&view, b'C' as c_char) },
        0,
        "suboffsets => never contiguous"
    );
    let mut empty: Py_buffer = unsafe { std::mem::zeroed() };
    empty.len = 0;
    empty.itemsize = 1;
    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyBuffer_IsContiguous(&empty, b'C' as c_char) },
        1,
        "len == 0 => always contiguous"
    );
}

#[test]
fn fill_info_null_view_sets_buffer_error() {
    install();
    unsafe { err_clear() };
    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            1,
            0,
        )
    };
    assert_eq!(rc, -1);
    assert!(unsafe { err_set() }, "NULL view must carry BufferError");
    unsafe { err_clear() };
}

// ===========================================================================
// datetime.rs
// ===========================================================================

#[test]
fn feb_30_is_rejected_leap_aware() {
    install();
    unsafe { err_clear() };
    let bad = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_date_from_date(
            2023,
            2,
            30,
            ptr::null_mut(),
        )
    };
    assert!(
        bad.is_null(),
        "Feb 30 must be ValueError (day bound was a flat 1..=31)"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
    // Feb 29 in a leap year is fine; in a non-leap year it is not.
    let leap = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_date_from_date(
            2024,
            2,
            29,
            ptr::null_mut(),
        )
    };
    assert!(!leap.is_null());
    unsafe { err_clear() };
    let nonleap = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_date_from_date(
            2023,
            2,
            29,
            ptr::null_mut(),
        )
    };
    assert!(nonleap.is_null());
    unsafe { err_clear() };
}

#[test]
fn delta_normalizes_and_rejects_overflow() {
    install();
    let delta = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_delta_from_delta(
            0,
            0,
            1_000_000,
            1,
            ptr::null_mut(),
        )
    };
    assert!(!delta.is_null());
    let d = delta.cast::<molt_cpython_abi::abi_types::PyDateTime_Delta>();
    unsafe {
        assert_eq!((*d).days, 0);
        assert_eq!((*d).seconds, 1, "1_000_000us must carry into seconds");
        assert_eq!((*d).microseconds, 0);
    }
    // Negative microseconds normalize with floor semantics: -1us == -1d+86399s+999999us.
    let neg = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_delta_from_delta(
            0,
            0,
            -1,
            1,
            ptr::null_mut(),
        )
    };
    let n = neg.cast::<molt_cpython_abi::abi_types::PyDateTime_Delta>();
    unsafe {
        assert_eq!((*n).days, -1);
        assert_eq!((*n).seconds, 86_399);
        assert_eq!((*n).microseconds, 999_999);
    }
    unsafe { err_clear() };
    let too_big = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_delta_from_delta(
            1_000_000_000,
            0,
            0,
            1,
            ptr::null_mut(),
        )
    };
    assert!(too_big.is_null(), "|days| > 999999999 is OverflowError");
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn timezone_utc_singleton_and_range_check() {
    install();
    // Zero offset + no name -> the UTC singleton.
    let zero = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_delta_from_delta(
            0,
            0,
            0,
            1,
            ptr::null_mut(),
        )
    };
    let utc = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_timezone_from_timezone(
            zero,
            ptr::null_mut(),
        )
    };
    assert!(
        !utc.is_null(),
        "timezone construction was NotImplementedError"
    );
    assert_eq!(
        utc,
        &raw mut molt_cpython_abi::abi_types::PyDateTime_TimeZone_UTC_Object,
        "zero offset with no name must reuse the UTC singleton"
    );
    // A 25-hour offset is out of range.
    unsafe { err_clear() };
    let big = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_delta_from_delta(
            1,
            3600,
            0,
            1,
            ptr::null_mut(),
        )
    };
    let bad = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_timezone_from_timezone(
            big,
            ptr::null_mut(),
        )
    };
    assert!(bad.is_null());
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn datetime_fromtimestamp_epoch() {
    install();
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    unsafe {
        molt_cpython_abi::api::sequences::PyTuple_SetItem(
            args,
            0,
            molt_cpython_abi::api::numbers::PyFloat_FromDouble(86_400.5),
        );
    }
    let dt = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_datetime_from_timestamp(
            ptr::null_mut(),
            args,
            ptr::null_mut(),
        )
    };
    assert!(!dt.is_null(), "fromtimestamp was NotImplementedError");
    let obj = dt.cast::<molt_cpython_abi::abi_types::PyDateTime_DateTime>();
    unsafe {
        let data = &(*obj).data;
        let year = ((data[0] as i32) << 8) | data[1] as i32;
        assert_eq!((year, data[2], data[3]), (1970, 1, 2), "1970-01-02");
        assert_eq!((data[4], data[5], data[6]), (0, 0, 0), "00:00:00");
        let us = ((data[7] as u32) << 16) | ((data[8] as u32) << 8) | data[9] as u32;
        assert_eq!(us, 500_000, "fractional second rounds to microseconds");
    }
}

#[test]
fn datetime_fold_out_of_range_is_valueerror() {
    install();
    unsafe { err_clear() };
    let dt = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_datetime_from_date_and_time_and_fold(
            2020,
            1,
            1,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            2,
            ptr::null_mut(),
        )
    };
    assert!(
        dt.is_null(),
        "fold=2 must be ValueError, not silently coerced to 1"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn non_tzinfo_argument_is_typeerror() {
    install();
    unsafe { err_clear() };
    let not_tz = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(3) };
    let dt = unsafe {
        molt_cpython_abi::api::datetime::molt_cpython_abi_datetime_from_date_and_time(
            2020,
            1,
            1,
            0,
            0,
            0,
            0,
            not_tz,
            ptr::null_mut(),
        )
    };
    assert!(
        dt.is_null(),
        "an int as tzinfo must be TypeError, not stored"
    );
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

// ===========================================================================
// capsule.rs
// ===========================================================================

#[test]
fn capsule_import_miss_sets_import_error() {
    install();
    unsafe { err_clear() };
    let p = unsafe {
        molt_cpython_abi::api::capsule::PyCapsule_Import(c"totally.absent._CAPI".as_ptr(), 0)
    };
    assert!(p.is_null());
    assert!(unsafe { err_set() });
    unsafe { err_clear() };
}

#[test]
fn capsule_import_registry_fast_path_still_resolves() {
    install();
    let mut data = 42u32;
    let capsule = unsafe {
        molt_cpython_abi::api::capsule::PyCapsule_New(
            (&mut data as *mut u32).cast(),
            c"regmod._CAPI".as_ptr(),
            None,
        )
    };
    assert!(!capsule.is_null());
    unsafe { err_clear() };
    let p =
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(c"regmod._CAPI".as_ptr(), 0) };
    assert_eq!(
        p,
        (&mut data as *mut u32).cast(),
        "registry fast path preserved"
    );
}
