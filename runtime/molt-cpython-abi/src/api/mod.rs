//! CPython stable ABI function implementations.
//!
//! Each sub-module provides `extern "C"` functions matching the CPython 3.12
//! stable ABI. C extensions dlopen'd by the loader call these functions through
//! the standard PLT/GOT mechanism — they don't know they're talking to Molt.
//!
//! ## Organization
//!
//! - `refcount`           — Py_INCREF / Py_DECREF / Py_XINCREF / Py_XDECREF
//! - `numbers`            — PyLong_*, PyFloat_*, PyBool_*
//! - `sequences`          — PyList_*, PyTuple_*, PySet_*
//! - `mapping`            — PyDict_*
//! - `strings`            — PyUnicode_*, PyBytes_*
//! - `modules`            — PyModule_*, PyModuleDef_Init
//! - `errors`             — PyErr_*, PyArg_ParseTuple, PyArg_ParseTupleAndKeywords
//! - `typeobj`            — PyType_Ready, PyType_GenericAlloc, PyType_IsSubtype
//! - `object`             — PyObject_* generic protocol (attr, item, call, truthiness)
//! - `abstract_number`    — PyNumber_* arithmetic/bitwise/conversion
//! - `abstract_sequence`  — PySequence_* length, getitem, contains, concat
//! - `abstract_mapping`   — PyMapping_* length, keys, values, items
//! - `buffer`             — PyObject_GetBuffer, PyBuffer_Release (stubs)
//! - `capsule`            — PyCapsule_New, PyCapsule_GetPointer

pub mod abstract_mapping;
pub mod abstract_number;
pub mod abstract_sequence;
pub mod buffer;
pub mod capsule;
pub mod errors;
pub mod mapping;
pub mod modules;
pub mod numbers;
pub mod object;
pub mod refcount;
pub mod sequences;
pub mod strings;
pub mod typeobj;
