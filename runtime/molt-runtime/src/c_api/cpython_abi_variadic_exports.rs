//! WASM export anchors for the CPython ABI C variadic shim family.
//!
//! The symbols listed here are implemented by `molt-cpython-abi`'s
//! `shims/pyarg_variadic.c`.  Source-recompiled native extensions import them
//! from the split runtime, so the runtime must retain the C shim archive even
//! when no Rust code calls the functions directly.  Keep this as an anchor over
//! the shared ABI implementation rather than reimplementing variadic CPython
//! behavior in Rust.

#![allow(dead_code, improper_ctypes)]

#[link(
    name = "molt_pyarg_shims",
    kind = "static",
    modifiers = "+whole-archive"
)]
unsafe extern "C" {
    fn PyArg_ParseTuple();
    fn PyArg_ParseTupleAndKeywords();
    fn PyArg_VaParseTupleAndKeywords();
    fn PyArg_UnpackTuple();
    fn PyTuple_Pack();
    fn PyObject_CallFunction();
    fn PyObject_CallFunctionObjArgs();
    fn PyObject_CallMethodObjArgs();
    fn PyObject_CallMethod();
    fn Py_BuildValue();
    fn _Py_BuildValue_SizeT();
    fn Py_VaBuildValue();
    fn PyUnicode_FromFormat();
    fn PyUnicode_FromFormatV();
    fn PyOS_snprintf();
    fn PyOS_vsnprintf();
    fn PyOS_string_to_double();
    fn PyOS_strtol();
    fn PyOS_strtoul();
    fn PyErr_WarnFormat();
    fn PyErr_Format();
    fn PyErr_FormatV();
    fn PyErr_FormatUnraisable();
    fn PySys_WriteStderr();
}

#[used]
static MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS: [unsafe extern "C" fn(); 24] = [
    PyArg_ParseTuple,
    PyArg_ParseTupleAndKeywords,
    PyArg_VaParseTupleAndKeywords,
    PyArg_UnpackTuple,
    PyTuple_Pack,
    PyObject_CallFunction,
    PyObject_CallFunctionObjArgs,
    PyObject_CallMethodObjArgs,
    PyObject_CallMethod,
    Py_BuildValue,
    _Py_BuildValue_SizeT,
    Py_VaBuildValue,
    PyUnicode_FromFormat,
    PyUnicode_FromFormatV,
    PyOS_snprintf,
    PyOS_vsnprintf,
    PyOS_string_to_double,
    PyOS_strtol,
    PyOS_strtoul,
    PyErr_WarnFormat,
    PyErr_Format,
    PyErr_FormatV,
    PyErr_FormatUnraisable,
    PySys_WriteStderr,
];

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_variadic_export_anchor_count() -> usize {
    core::hint::black_box(MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS.as_ptr());
    MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS.len()
}
