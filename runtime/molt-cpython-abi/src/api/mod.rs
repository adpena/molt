//! CPython stable ABI function implementations.
//!
//! Each sub-module provides `extern "C"` functions matching the CPython 3.12
//! stable ABI. C extensions dlopen'd by the loader call these functions through
//! the standard PLT/GOT mechanism — they don't know they're talking to Molt.
//!
//! ## Organization
//!
//! - `refcount`  — Py_INCREF / Py_DECREF / Py_XINCREF / Py_XDECREF
//! - `numbers`   — PyLong_*, PyFloat_*, PyBool_*
//! - `sequences` — PyList_*, PyTuple_*
//! - `mapping`   — PyDict_*
//! - `strings`   — PyUnicode_*, PyBytes_*
//! - `modules`   — PyModule_*, PyModuleDef_Init
//! - `errors`    — PyErr_*, PyArg_ParseTuple, PyArg_ParseTupleAndKeywords
//! - `typeobj`   — PyType_Ready, PyType_GenericAlloc

pub mod errors;
pub mod mapping;
pub mod modules;
pub mod numbers;
pub mod refcount;
pub mod sequences;
pub mod strings;
pub mod typeobj;
