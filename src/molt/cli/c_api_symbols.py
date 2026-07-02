from __future__ import annotations

import re

C_API_TOKEN = (
    r"(?:_?Py[A-Za-z_][A-Za-z0-9_]*|Npy[A-Za-z_][A-Za-z0-9_]*|"
    r"npy[A-Za-z_][A-Za-z0-9_]*|NPY_[A-Za-z0-9_]+)"
)
C_API_TOKEN_RE = re.compile(rf"\b(?P<symbol>{C_API_TOKEN})\b")

_C_API_DECLARATION_ONLY_SYMBOLS = frozenset(
    {
        "PyMODINIT_FUNC",
        "Python",
        "PyObject",
        "PyVarObject",
        "PyTypeObject",
        "PyHeapTypeObject",
        "PyModuleDef",
        "PyModuleDef_Base",
        "PyMethodDef",
        "PyGetSetDef",
        "PyMemberDef",
        "PyBufferProcs",
        "PyNumberMethods",
        "PySequenceMethods",
        "PyMappingMethods",
        "PyAsyncMethods",
        "PyType_Slot",
        "PyType_Spec",
        "PyThreadState",
        "PyInterpreterState",
        "PyGILState_STATE",
        "Py_buffer",
        "Py_ssize_t",
        "Py_hash_t",
        "Py_UCS1",
        "Py_UCS2",
        "Py_UCS4",
        "Py_UNICODE",
    }
)


_C_API_PRIMITIVE_EXACT: dict[str, str] = {
    "Py_INCREF": "refcount",
    "Py_DECREF": "refcount",
    "Py_XINCREF": "refcount",
    "Py_XDECREF": "refcount",
    "Py_NewRef": "refcount",
    "Py_XNewRef": "refcount",
    "PyObject_GetBuffer": "buffer_protocol",
    "PyBuffer_Release": "buffer_protocol",
}


_C_API_PRIMITIVE_PREFIXES: tuple[tuple[tuple[str, ...], str], ...] = (
    (
        (
            "PyArray_",
            "PyArray",
            "PyDataType_",
            "PyDimMem_",
            "PyTypeNum_",
            "Npy",
            "npy",
            "NPY_",
        ),
        "numpy_c_api",
    ),
    (("_Pyx_",), "cython_runtime_helper"),
    (("PyCapsule",), "capsules"),
    (("PyErr", "PyExc"), "exceptions"),
    (
        ("PyMem", "PyObject_Malloc", "PyObject_Free", "PyObject_Realloc"),
        "memory_allocator",
    ),
    (("PyBuffer", "PyMemoryView"), "buffer_protocol"),
    (("PyImport",), "import_system"),
    (("PyModule", "PyState", "PyThreadState"), "module_state"),
    (("PyGILState", "PyThread"), "gil_threading"),
    (("PyUnicode", "Py_UCS"), "unicode_text"),
    (("PyBytes", "PyByteArray"), "bytes_bytearray"),
    (
        (
            "PyCallable",
            "PyCFunction",
            "PyObject_Call",
            "PyObject_Vectorcall",
            "PyVectorcall",
            "PyFunction",
        ),
        "call_protocol",
    ),
    (("PyDescr", "PyGetSet", "PyMember", "PyMethod"), "descriptor_protocol"),
    (
        (
            "PyIter",
            "PyMapping",
            "PySequence",
            "PyDict",
            "PyList",
            "PyTuple",
            "PySet",
            "PySlice",
        ),
        "iterator_mapping_helpers",
    ),
    (
        (
            "PyLong",
            "PyFloat",
            "PyNumber",
            "PyBool",
            "PyComplex",
            "PyOS_string_to_double",
        ),
        "numeric_scalars",
    ),
    (("PyCode", "PyFrame", "PyTraceBack", "PyEval"), "code_frame_eval"),
    (("PyType", "PyObject", "_PyType", "_PyObject"), "object_type_lifecycle"),
)


def is_c_api_symbol(symbol: str) -> bool:
    return C_API_TOKEN_RE.fullmatch(symbol) is not None


def is_c_api_external_requirement(symbol: str) -> bool:
    """Return whether a source token names an external C/API obligation.

    The broad scanner intentionally recognizes header and typedef spellings so
    the scan surface can classify them. Static-link object custody is stricter:
    declaration-only names must not become provider/link requirements.
    """
    return (
        is_c_api_symbol(symbol)
        and symbol not in _C_API_DECLARATION_ONLY_SYMBOLS
        and not symbol.endswith("_H")
    )


def c_api_primitive_class(symbol: str) -> str:
    exact = _C_API_PRIMITIVE_EXACT.get(symbol)
    if exact is not None:
        return exact
    for prefixes, primitive_class in _C_API_PRIMITIVE_PREFIXES:
        if symbol.startswith(prefixes):
            return primitive_class
    return "python_c_api"


_NON_CPYTHON_ABI_PRIMITIVE_CLASSES = frozenset(
    {
        "numpy_c_api",
        "cython_runtime_helper",
    }
)


def is_cpython_abi_link_symbol(symbol: str) -> bool:
    """Return whether *symbol* is owned by the Molt CPython ABI runtime lane."""
    return (
        is_c_api_external_requirement(symbol)
        and c_api_primitive_class(symbol) not in _NON_CPYTHON_ABI_PRIMITIVE_CLASSES
    )
