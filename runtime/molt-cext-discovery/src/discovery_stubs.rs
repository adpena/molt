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
//!
//! CPython's PUBLIC singletons `_Py_NoneStruct` / `_Py_TrueStruct` /
//! `_Py_FalseStruct` and the `_SizeT` variadic variants are handled elsewhere
//! (identity-correct linker aliases in `build.rs`; a C shim for the variadics).

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
