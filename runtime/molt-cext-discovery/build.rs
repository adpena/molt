//! Punch-through link setup for the native discovery harness.
//!
//! numpy's `_multiarray_umath` links CPython symbols molt's ABI implements but
//! exports under DIFFERENT names. We alias the canonical CPython names ONTO
//! molt's real symbols at link time — IDENTITY/BEHAVIOR CORRECT (they reuse
//! molt's own storage/impl):
//!
//!   PUBLIC singletons (identity-critical: numpy's `Py_None` == molt's None)
//!     _Py_NoneStruct   ->  Py_None
//!     _Py_TrueStruct   ->  Py_True
//!     _Py_FalseStruct  ->  Py_False
//!
//!   PY_SSIZE_T_CLEAN variadic variants (numpy defines PY_SSIZE_T_CLEAN, so its
//!   `PyArg_ParseTuple` calls link `_PyArg_ParseTuple_SizeT` etc. — the SAME
//!   function; molt is already ssize-clean, so aliasing is behavior-correct)
//!     _PyArg_ParseTuple_SizeT              ->  PyArg_ParseTuple
//!     _PyArg_ParseTupleAndKeywords_SizeT   ->  PyArg_ParseTupleAndKeywords
//!     _PyArg_VaParseTupleAndKeywords_SizeT ->  PyArg_VaParseTupleAndKeywords
//!     _PyObject_CallFunction_SizeT         ->  PyObject_CallFunction
//!     _PyObject_CallMethod_SizeT           ->  PyObject_CallMethod
//!
//! The private data singletons (`_Py_EllipsisObject`, `_Py_NotImplementedStruct`)
//! and private functions (`_Py_Dealloc`, `_PyErr_BadInternalCall`, ...) are
//! provided as loud instrumentation in `discovery_stubs.rs`.
//!
//! `-export_dynamic` forces all globals (incl. the aliases) into the dynamic
//! table so numpy's `dynamic_lookup` binds to them.
//!
//! The REAL fix belongs in molt-cpython-abi (export the canonical CPython names
//! directly). This harness aliases/stubs so the DISCOVERY sweep advances PAST
//! the symbol-resolution wall into numpy's actual init-time semantic frontiers.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // (molt C symbol -> canonical CPython C name). The macho `_` prefix is added
    // below for Darwin; the `to` names carry CPython's own leading underscore.
    let aliases: &[(&str, &str)] = &[
        ("Py_None", "_Py_NoneStruct"),
        ("Py_True", "_Py_TrueStruct"),
        ("Py_False", "_Py_FalseStruct"),
    ];

    if target_os == "macos" || target_os == "ios" {
        for (from, to) in aliases {
            // ld64: -alias <existing> <new>; Darwin prepends one `_` to C names.
            println!("cargo:rustc-link-arg=-Wl,-alias,_{from},_{to}");
        }
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
    } else if !(target_os == "windows" && target_env == "msvc") {
        for (from, to) in aliases {
            // GNU ld / lld: --defsym NEW=OLD.
            println!("cargo:rustc-link-arg=-Wl,--defsym,{to}={from}");
        }
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
    }
}
