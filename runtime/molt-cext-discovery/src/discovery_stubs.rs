//! DISCOVERY INSTRUMENTATION — not shipped runtime.
//!
//! These symbols exist ONLY to punch the native discovery sweep PAST the
//! symbol-resolution wall (macOS `dlopen(RTLD_NOW)` aborts on the first
//! unresolved symbol) and into numpy's actual init-time semantic frontiers.
//!
//! As `molt-cpython-abi` grows the real symbols, the corresponding stubs here
//! are DELETED (the harness sheds its aliases). Already real in the ABI and
//! removed from here: the allocators (`PyObject_Malloc`/`Calloc`/`Realloc`/
//! `Free`), `PyThreadState_GetDict`, `PyOS_setsig`, `_Py_ascii_whitespace`,
//! `_Py_Dealloc`, `_PyErr_BadInternalCall`, `_PyDict_GetItemStringWithError`.
//! (numpy links the PRIVATE `_PyObject_New`/`_PyObject_GC_New`/`_Py_HashDouble`/
//! `_PyUnicode_IsWhitespace`, which the ABI already exports, so no public-name
//! stubs are needed for those either.)
//!
//! What REMAINS here is what the ABI does not yet provide natively:
//!   * Data-singleton stubs for CPython's PRIVATE singletons
//!     (`_Py_EllipsisObject`, `_Py_NotImplementedStruct`). These do NOT share
//!     molt's identity storage, so identity-dependent behavior beyond this
//!     point is approximate — acceptable for "how far does init get". A true
//!     same-storage global alias to molt's `Py_EllipsisObject` /
//!     `Py_NotImplementedSentinel` needs a linker `-alias` at the FINAL link
//!     (an in-crate `.set` alias is emitted LOCAL on Mach-O), which a library
//!     crate cannot itself request — see build.rs for the public-singleton
//!     aliases done at this cdylib's own link.
//!   * `PyStructSequence_New` / `PyStructSequence_InitType2` — a genuine feature
//!     the ABI has not implemented yet (numpy does not call them on the path to
//!     the current init frontier; each announces itself if it fires).
//!
//! CPython's PUBLIC singletons `_Py_NoneStruct` / `_Py_TrueStruct` /
//! `_Py_FalseStruct` and the `_SizeT` variadic variants are handled elsewhere
//! (identity-correct linker aliases in `build.rs`; a C shim for the variadics).

use std::os::raw::{c_int, c_void};
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
pub static DISCOVERY_ELLIPSIS: StubSingleton = StubSingleton {
    ob_refcnt: 1 << 30,
    ob_type: 0,
};
#[unsafe(export_name = "_Py_NotImplementedStruct")]
pub static DISCOVERY_NOTIMPLEMENTED: StubSingleton = StubSingleton {
    ob_refcnt: 1 << 30,
    ob_type: 0,
};

// ─── LOUD instrumentation stubs for genuine ABI function gaps ─────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyStructSequence_InitType2(
    _type: *mut c_void,
    _desc: *mut c_void,
) -> c_int {
    note_first(
        once_flag!(),
        "PyStructSequence_InitType2",
        "struct-sequence type init (UNIMPLEMENTED in molt ABI)",
    );
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyStructSequence_New(_type: *mut c_void) -> *mut c_void {
    note_first(
        once_flag!(),
        "PyStructSequence_New",
        "struct-sequence instance (UNIMPLEMENTED in molt ABI; returns NULL)",
    );
    std::ptr::null_mut()
}
