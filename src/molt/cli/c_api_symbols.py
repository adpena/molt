from __future__ import annotations

import re

C_API_TOKEN = (
    r"(?:_?Py[A-Za-z_][A-Za-z0-9_]*|Npy[A-Za-z_][A-Za-z0-9_]*|"
    r"npy[A-Za-z_][A-Za-z0-9_]*|NPY_[A-Za-z0-9_]+)"
)
C_API_TOKEN_RE = re.compile(rf"\b(?P<symbol>{C_API_TOKEN})\b")


def is_c_api_symbol(symbol: str) -> bool:
    return C_API_TOKEN_RE.fullmatch(symbol) is not None


def c_api_primitive_class(symbol: str) -> str:
    if symbol.startswith(
        (
            "PyArray_",
            "PyArray",
            "PyDataType_",
            "PyDimMem_",
            "PyTypeNum_",
            "Npy",
            "npy",
            "NPY_",
        )
    ):
        return "numpy_c_api"
    if symbol.startswith("PyCapsule"):
        return "capsules"
    if symbol.startswith(("PyModule", "PyState", "PyThreadState", "PyGILState")):
        return "module_state"
    if symbol.startswith(("PyErr", "PyExc")):
        return "exceptions"
    if symbol in {"Py_INCREF", "Py_DECREF", "Py_XINCREF", "Py_XDECREF"}:
        return "refcount"
    if symbol.startswith(("PyType", "PyObject", "Py_NewRef", "Py_XNewRef")):
        return "object_type_lifecycle"
    if symbol.startswith(("PyBuffer", "PyMemoryView")) or symbol in {
        "PyObject_GetBuffer",
        "PyBuffer_Release",
    }:
        return "buffer_protocol"
    if symbol.startswith(
        (
            "PyIter",
            "PyMapping",
            "PySequence",
            "PyDict",
            "PyList",
            "PyTuple",
            "PySet",
        )
    ):
        return "iterator_mapping_helpers"
    if symbol.startswith(
        ("PyLong", "PyFloat", "PyNumber", "PyBool", "PyComplex", "PyOS_string_to_double")
    ):
        return "numeric_scalars"
    return "python_c_api"
