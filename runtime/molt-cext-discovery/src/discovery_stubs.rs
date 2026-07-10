//! DISCOVERY INSTRUMENTATION — not shipped runtime.
//!
//! These symbols exist ONLY to punch the native discovery sweep PAST the
//! symbol-resolution wall (macOS `dlopen(RTLD_NOW)` aborts on the first
//! unresolved symbol) and into numpy's actual init-time semantic frontiers.
//!
//! numpy's `_multiarray_umath` links a set of CPython symbols molt's ABI does
//! not export (or exports under a different name). Three kinds live here:
//!   1. Trivially-correct impls molt is simply *missing* (`PyObject_Malloc`
//!      == the C allocator; the `_Py_ascii_whitespace` table). Correct as-is.
//!   2. LOUD instrumentation stubs for genuine ABI gaps — each announces itself
//!      once on stderr with `===MOLT_DISCOVERY_STUB_FIRST_CALL` the moment numpy
//!      first calls it, turning a "missing symbol" into an ORDERED native
//!      frontier record, then returns the least-harmful value so the sweep can
//!      surface the NEXT frontier. A stub firing is a FRONTIER, never a silent
//!      fake — it always announces itself.
//!   3. Data-singleton stubs for CPython's private singleton symbols
//!      (`_Py_EllipsisObject`, `_Py_NotImplementedStruct`). These do NOT share
//!      molt's identity storage, so identity-dependent behavior beyond this
//!      point is approximate — acceptable for "how far does init get".
//!
//! CPython's PUBLIC singletons `_Py_NoneStruct` / `_Py_TrueStruct` /
//! `_Py_FalseStruct` and the `_SizeT` variadic variants are handled elsewhere
//! (identity-correct linker aliases in `build.rs`; a C shim for the variadics).
//!
//! Every export here uses the EXACT CPython C symbol name (note the private
//! leading underscore on `_Py_Dealloc`, `_PyErr_BadInternalCall`, ...).

use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

fn note_first(flag: &AtomicBool, name: &str, detail: &str) {
    if !flag.swap(true, Ordering::SeqCst) {
        eprintln!("===MOLT_DISCOVERY_STUB_FIRST_CALL: {name} — {detail}");
    }
}

macro_rules! once_flag {
    () => {{
        static SEEN: AtomicBool = AtomicBool::new(false);
        &SEEN
    }};
}

// ─── Missing allocators (trivially correct — belong in molt-cpython-abi) ──────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Malloc(size: usize) -> *mut c_void {
    unsafe { libc::malloc(size.max(1)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { libc::calloc(nelem.max(1), elsize.max(1)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    unsafe { libc::realloc(ptr, size.max(1)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Free(ptr: *mut c_void) {
    unsafe { libc::free(ptr) }
}

/// `PyObject_New(type)` / `PyObject_GC_New(type)` — allocate a zeroed block and
/// stamp the CPython `PyObject` header (`ob_refcnt = 1`, `ob_type = type`).
unsafe fn new_object(type_ptr: *mut c_void) -> *mut c_void {
    const SLAB: usize = 4096;
    let p = unsafe { libc::calloc(1, SLAB) } as *mut usize;
    if !p.is_null() {
        unsafe {
            *p = 1;
            *p.add(1) = type_ptr as usize;
        }
    }
    p as *mut c_void
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_New(type_ptr: *mut c_void) -> *mut c_void {
    unsafe { new_object(type_ptr) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_New(type_ptr: *mut c_void) -> *mut c_void {
    note_first(once_flag!(), "PyObject_GC_New", "GC allocation (no tracked GC in molt ABI)");
    unsafe { new_object(type_ptr) }
}

// ─── `_Py_ascii_whitespace[128]` classification table (correct) ───────────────
#[unsafe(export_name = "_Py_ascii_whitespace")]
pub static PY_ASCII_WHITESPACE: [u8; 128] = {
    let mut t = [0u8; 128];
    t[0x09] = 1;
    t[0x0a] = 1;
    t[0x0b] = 1;
    t[0x0c] = 1;
    t[0x0d] = 1;
    t[0x1c] = 1;
    t[0x1d] = 1;
    t[0x1e] = 1;
    t[0x1f] = 1;
    t[0x20] = 1;
    t
};

// ─── Data-singleton stubs for CPython's PRIVATE singletons ────────────────────
// numpy links `_Py_EllipsisObject` and `_Py_NotImplementedStruct` as data. molt
// exports them under different names (`Py_EllipsisObject`,
// `Py_NotImplementedSentinel`); expose the CPython names here (identity not
// shared — a documented discovery approximation). Immortal refcount, null type.
#[repr(C)]
pub struct StubSingleton {
    pub ob_refcnt: isize,
    pub ob_type: usize,
}
#[unsafe(export_name = "_Py_EllipsisObject")]
pub static DISCOVERY_ELLIPSIS: StubSingleton = StubSingleton { ob_refcnt: 1 << 30, ob_type: 0 };
#[unsafe(export_name = "_Py_NotImplementedStruct")]
pub static DISCOVERY_NOTIMPLEMENTED: StubSingleton =
    StubSingleton { ob_refcnt: 1 << 30, ob_type: 0 };

// ─── LOUD instrumentation stubs for genuine ABI function gaps ─────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_IsWhitespace(ch: u32) -> c_int {
    matches!(ch,
        0x09..=0x0d | 0x1c..=0x1f | 0x20 | 0x85 | 0xa0 | 0x1680
        | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000
    ) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_HashDouble(_inst: *mut c_void, v: f64) -> isize {
    note_first(once_flag!(), "Py_HashDouble", "float/complex hashing (non-CPython hash algorithm)");
    (v.to_bits() as isize) ^ (v.to_bits() >> 32) as isize
}

unsafe extern "C" {
    fn PyDict_GetItemString(dp: *mut c_void, key: *const c_char) -> *mut c_void;
}
// CPython's PRIVATE `_PyDict_GetItemStringWithError` (leading underscore).
#[unsafe(export_name = "_PyDict_GetItemStringWithError")]
pub unsafe extern "C" fn priv_dict_getitemstring_witherror(
    dp: *mut c_void,
    key: *const c_char,
) -> *mut c_void {
    note_first(once_flag!(), "_PyDict_GetItemStringWithError",
        "forwarded to PyDict_GetItemString (error-signalling nuance not modelled)");
    unsafe { PyDict_GetItemString(dp, key) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetDict() -> *mut c_void {
    note_first(once_flag!(), "PyThreadState_GetDict", "thread-state dict (returns NULL)");
    std::ptr::null_mut()
}

// CPython's PRIVATE `_Py_Dealloc` (leading underscore).
#[unsafe(export_name = "_Py_Dealloc")]
pub unsafe extern "C" fn priv_py_dealloc(_obj: *mut c_void) {
    note_first(once_flag!(), "_Py_Dealloc", "object finalization (no-op; leaks during discovery)");
}

// CPython's PRIVATE `_PyErr_BadInternalCall` (leading underscore).
#[unsafe(export_name = "_PyErr_BadInternalCall")]
pub unsafe extern "C" fn priv_err_badinternalcall(_file: *const c_char, _line: c_int) {
    note_first(once_flag!(), "_PyErr_BadInternalCall",
        "SystemError signalling (no-op; molt exports the public PyErr_BadInternalCall only)");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_setsig(_sig: c_int, handler: *mut c_void) -> *mut c_void {
    note_first(once_flag!(), "PyOS_setsig", "signal handler install (no-op)");
    handler
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyStructSequence_InitType2(
    _type: *mut c_void,
    _desc: *mut c_void,
) -> c_int {
    note_first(once_flag!(), "PyStructSequence_InitType2",
        "struct-sequence type init (UNIMPLEMENTED in molt ABI)");
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyStructSequence_New(_type: *mut c_void) -> *mut c_void {
    note_first(once_flag!(), "PyStructSequence_New",
        "struct-sequence instance (UNIMPLEMENTED in molt ABI; returns NULL)");
    std::ptr::null_mut()
}
