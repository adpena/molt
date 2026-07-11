//! `molt-cpython-abi` — CPython binary ABI compatibility shim for Molt.
//!
//! ## Purpose
//!
//! Enables loading pre-built CPython C extensions (`.so` / `.pyd`) directly
//! into a Molt runtime via `dlopen` without recompilation.
//!
//! ## Architecture
//!
//! CPython uses `PyObject*` — a tagged C struct with specific memory layout.
//! Molt uses `MoltHandle = u64` — QNAN-boxed values that encode type tags and
//! pointers inside a 64-bit word.
//!
//! This crate provides:
//!
//! 1. **ABI types** (`abi_types`): `PyObject`, `PyTypeObject`, `PyMethodDef`,
//!    etc. with `repr(C)` layout matching CPython 3.12 stable ABI.
//!
//! 2. **ABI functions** (`api/`): The ~150 CPython C API functions that cover
//!    >95% of real extension code, implemented as thin bridges to Molt internals.
//!
//! 3. **Object bridge** (`bridge`): Bidirectional translation between
//!    `*mut PyObject` and `MoltHandle`, with SIMD-accelerated type-tag lookup.
//!
//! 4. **Extension loader** (`loader`): `dlopen` a real CPython `.so`, call
//!    its `PyInit_<name>()` entry point, and wrap the result as a Molt module.
//!
//! ## SIMD strategy
//!
//! The hot path is `bridge::handle_to_pyobj()` — called on every argument
//! when a C extension function receives args from Molt.
//!
//! - **x86_64 (SSE4.1)**: `_mm_cmpeq_epi8` on 16-byte type-tag cache lines
//! - **ARM64 (NEON)**: `vceqq_u8` equivalent via `std::arch::aarch64`
//! - **Fallback**: scalar binary search in sorted type-tag table
//!
//! Both paths are selected at compile time via `#[cfg(target_arch)]`.
//!
//! ## Stable ABI coverage
//!
//! The following groups are implemented:
//! - Ref-counting: `Py_INCREF`, `Py_DECREF`, `Py_XINCREF`, `Py_XDECREF`
//! - None/True/False singletons
//! - Integers: `PyLong_FromLong`, `PyLong_AsLong`, `PyLong_FromSsize_t`, etc.
//! - Floats: `PyFloat_FromDouble`, `PyFloat_AsDouble`
//! - Bytes/Unicode: `PyBytes_FromStringAndSize`, `PyUnicode_FromString`, etc.
//! - Lists: `PyList_New`, `PyList_Append`, `PyList_GET_ITEM`, `PyList_SET_ITEM`
//! - Tuples: `PyTuple_New`, `PyTuple_GET_ITEM`, `PyTuple_GET_SIZE`
//! - Dicts: `PyDict_New`, `PyDict_SetItem`, `PyDict_GetItem`, etc.
//! - Modules: `PyModule_New`, `PyModule_AddObject`, `PyModuleDef_Init`
//! - Errors: `PyErr_SetString`, `PyErr_Occurred`, `PyErr_Clear`, `PyArg_ParseTuple`
//! - Type checks: `PyLong_Check`, `PyFloat_Check`, `PyList_Check`, `PyTuple_Check`, etc.

#![deny(unsafe_op_in_unsafe_fn)]
// Apparatus for the CPYTHON-ABI-LOCK-SWEEP class: a `match`/`if let`/`while let`
// scrutinee that locks a non-reentrant `Mutex` (e.g. `GLOBAL_BRIDGE.lock()`)
// extends that guard's lifetime across every arm body (Rust's temporary
// lifetime extension for match scrutinees). Any arm that re-locks the same
// mutex — directly, or via a helper that locks it (e.g. `PyErr_SetString`,
// `PyErr_BadInternalCall`, `PyErr_SetObject`, all of which lock `GLOBAL_BRIDGE`
// to resolve the exception type) — self-deadlocks: a hang, not a crash. Two
// P0 hangs of exactly this shape already bit this crate (`PyErr_GivenExceptionMatches`,
// `PyObject_GetBuffer`) before a full sweep fixed all live and latent sites by
// binding the lock's result to a local *before* the match/if-let so the guard
// drops before any arm runs. This lint is the regression gate: it fires on
// exactly this shape (a significant-Drop temporary — a `MutexGuard` — held
// across a match scrutinee), so it must stay denied.
#![deny(clippy::significant_drop_in_scrutinee)]
#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    clippy::missing_safety_doc
)]
#![cfg_attr(target_arch = "wasm32", allow(unused))]

/// Debug-only alignment guard for pointers whose alignment molt *owns* and
/// guarantees at the allocation site.
///
/// Use this to document + enforce an N-byte alignment contract so
/// `MOLT_WITNESS_ITERATION=1` (debug assertions on) keeps catching regressions
/// where a molt-minted allocation drifts out of alignment. It is a no-op in
/// release builds.
///
/// Do **not** use it to guard pointers minted by C — those carry no molt-side
/// alignment guarantee. On `wasm32` a C `PyObject` has struct alignment 4 (its
/// widest member, `Py_ssize_t`/`ob_type`, is 4 bytes), so a statically-declared
/// C object (module / type / exception singleton) sits at a 4-byte-aligned
/// address. An 8-byte scalar read (`u64`/`f64`) or a projection into an
/// 8-aligned struct off such a pointer is a misaligned dereference (UB in Rust
/// semantics; silently tolerated by wasm hardware in release, caught by the
/// debug alignment check). Read those with `core::ptr::read_unaligned` instead.
macro_rules! debug_assert_aligned {
    ($ptr:expr, $ty:ty) => {{
        let __addr = $ptr as *const _ as usize;
        let __align = ::core::mem::align_of::<$ty>();
        ::core::debug_assert!(
            __addr % __align == 0,
            "pointer {:#x} is not aligned to {} bytes for {}",
            __addr,
            __align,
            ::core::stringify!($ty),
        );
    }};
}

pub mod abi_types;
pub mod api;
pub mod bridge;
pub mod capi_trace;
pub mod hooks;

#[cfg(all(feature = "extension-loader", not(target_arch = "wasm32")))]
pub mod loader;

// Profiling + allocation-budget gate for the Py_buffer export hot path.
// Test-only: never compiled into the shipped cdylib/rlib.
#[cfg(test)]
mod buffer_export_bench;

pub use abi_types::{Py_ssize_t, PyObject, PyTypeObject};
pub use bridge::{AbiHandle, ObjectBridge};
pub use hooks::{
    DictOp, MoltBufferView, NumberBinaryOp, NumberUnaryOp, RuntimeHooks, SetOp, hooks,
    hooks_or_stubs, set_runtime_hooks, try_set_runtime_hooks,
};
