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
#![allow(non_snake_case, non_camel_case_types, clippy::missing_safety_doc)]
#![cfg_attr(target_arch = "wasm32", allow(unused))]

pub mod abi_types;
pub mod api;
pub mod bridge;
pub mod hooks;

#[cfg(all(feature = "extension-loader", not(target_arch = "wasm32")))]
pub mod loader;

pub use abi_types::{PyObject, PyTypeObject, Py_ssize_t};
pub use bridge::{AbiHandle, ObjectBridge};
pub use hooks::{RuntimeHooks, set_runtime_hooks, hooks, hooks_or_stubs};
