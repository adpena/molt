#ifdef _MSC_VER
#define _Thread_local __declspec(thread)
#endif
#ifdef MOLT_PUBLIC_HEADER_PROBE
#include <molt/Python.h>
#else
#include <Python.h>
#endif

/* Runtime supplies the independent expected class. This exercises the actual
 * compiled Py_TYPE/Py_IS_TYPE/TypeCheck macros, never a Rust proxy for them. */
int MOLT_TYPE_IDENTITY_PROBE(PyObject *value, PyTypeObject *expected,
                             int family, int expected_exact) {
    if (Py_TYPE(value) != expected) return 1;
    if (value == NULL) return expected == NULL ? 0 : 2;
    if (!Py_IS_TYPE(value, expected)) return 3;
    if (!PyObject_TypeCheck(value, expected)) return 4;
    if (family == 1) {
        if (!PyList_Check(value)) return 5;
        if (PyList_CheckExact(value) != expected_exact) return 6;
    } else if (family == 2) {
        if (!PyTuple_Check(value)) return 7;
        if (PyTuple_CheckExact(value) != expected_exact) return 8;
    }
    return 0;
}

#define MOLT_PROBE_JOIN_INNER(left, right) left##right
#define MOLT_PROBE_JOIN(left, right) MOLT_PROBE_JOIN_INNER(left, right)
#define MOLT_GC_PROBE(suffix) MOLT_PROBE_JOIN(MOLT_TYPE_IDENTITY_PROBE, suffix)

/* Evaluate every builtin symbol while a custom BufferError subclass instance
 * is pending. The runtime supplies independent singleton addresses each epoch. */
int MOLT_GC_PROBE(_exception_symbols)(PyObject *const *expected, size_t count,
                                     PyObject *pending) {
    PyObject *actual[] = {
        PyExc_BaseException,
        PyExc_Exception,
        PyExc_BaseExceptionGroup,
        PyExc_StopAsyncIteration,
        PyExc_StopIteration,
        PyExc_GeneratorExit,
        PyExc_ArithmeticError,
        PyExc_LookupError,
        PyExc_AssertionError,
        PyExc_AttributeError,
        PyExc_BufferError,
        PyExc_EOFError,
        PyExc_FloatingPointError,
        PyExc_OSError,
        PyExc_ImportError,
        PyExc_ModuleNotFoundError,
        PyExc_IndexError,
        PyExc_KeyError,
        PyExc_KeyboardInterrupt,
        PyExc_MemoryError,
        PyExc_NameError,
        PyExc_OverflowError,
        PyExc_RuntimeError,
        PyExc_RecursionError,
        PyExc_NotImplementedError,
        PyExc_SyntaxError,
        PyExc_IndentationError,
        PyExc_TabError,
        PyExc_ReferenceError,
        PyExc_SystemError,
        PyExc_SystemExit,
        PyExc_TypeError,
        PyExc_UnboundLocalError,
        PyExc_ValueError,
        PyExc_UnicodeError,
        PyExc_UnicodeEncodeError,
        PyExc_UnicodeDecodeError,
        PyExc_UnicodeTranslateError,
        PyExc_ZeroDivisionError,
        PyExc_BlockingIOError,
        PyExc_ChildProcessError,
        PyExc_ConnectionError,
        PyExc_BrokenPipeError,
        PyExc_ConnectionAbortedError,
        PyExc_ConnectionRefusedError,
        PyExc_ConnectionResetError,
        PyExc_FileExistsError,
        PyExc_FileNotFoundError,
        PyExc_InterruptedError,
        PyExc_IsADirectoryError,
        PyExc_NotADirectoryError,
        PyExc_PermissionError,
        PyExc_ProcessLookupError,
        PyExc_TimeoutError,
        PyExc_Warning,
        PyExc_UserWarning,
        PyExc_DeprecationWarning,
        PyExc_PendingDeprecationWarning,
        PyExc_SyntaxWarning,
        PyExc_RuntimeWarning,
        PyExc_FutureWarning,
        PyExc_ImportWarning,
        PyExc_UnicodeWarning,
        PyExc_BytesWarning,
        PyExc_EncodingWarning,
        PyExc_ResourceWarning,
    };
    if (count != sizeof(actual) / sizeof(actual[0])) return 1;
    for (size_t i = 0; i < count; ++i) {
        if (actual[i] != expected[i]) return (int)i + 2;
    }
    if (PyExc_EnvironmentError != PyExc_OSError || PyExc_IOError != PyExc_OSError) return 100;
#if defined(_WIN32) || defined(MS_WINDOWS)
    if (PyExc_WindowsError != PyExc_OSError) return 101;
#endif
    if (!PyErr_ExceptionMatches(PyExc_BufferError)) return 102;
    if (!PyErr_ExceptionMatches(PyExc_Exception)) return 103;
    if (PyErr_ExceptionMatches(PyExc_TypeError)) return 104;
    PyObject *observed = PyErr_GetRaisedException();
    int same = observed == pending;
    PyErr_SetRaisedException(observed);
    return same ? 0 : 105;
}

/* Real compiled consumers of both distributed GC control facades. */
int MOLT_GC_PROBE(_gc_control)(PyObject *value, int tracked, int finalized) {
    if (PyObject_GC_IsTracked(value) != tracked) return 1;
    if (PyObject_GC_IsFinalized(value) != finalized) return 2;
    PyObject_GC_UnTrack(value);
    PyObject_GC_UnTrack(value);
    if (PyObject_GC_IsTracked(value) != 0) return 3;
    if (PyObject_GC_IsFinalized(value) != finalized) return 4;
    PyObject_GC_Track(value);
    if (PyObject_GC_IsTracked(value) != 1) return 5;
    if (PyObject_GC_IsFinalized(value) != finalized) return 6;
    if (!tracked) PyObject_GC_UnTrack(value);
    return 0;
}

/* The runtime supplies the real native type layout. The source-compatible
 * facade intentionally exposes only an opaque PyTypeObject shell. */
int MOLT_GC_PROBE(_gc_allocation)(PyTypeObject *type) {
    PyObject *fixed;
    PyVarObject *variable;
    int result = 0;
    fixed = _PyObject_GC_New(type);
    if (fixed == NULL) return 1;
    variable = _PyObject_GC_NewVar(type, 3);
    if (variable == NULL) {
        PyObject_GC_Del(fixed);
        return 2;
    }
    if (PyObject_GC_IsTracked(fixed) || PyObject_GC_IsTracked((PyObject *)variable)) {
        result = 3;
        goto done;
    }
    if (Py_SIZE(variable) != 3) {
        result = 4;
        goto done;
    }
    Py_SET_SIZE(variable, 2);
    if (Py_SIZE(variable) != 2) {
        result = 6;
        goto done;
    }
    Py_SET_SIZE(variable, 3);
    /* Complete payload initialization before the public tracking boundary. */
    for (int index = 0; index < 3; ++index) {
        ((PyObject **)(variable + 1))[index] = NULL;
    }
    if (MOLT_GC_PROBE(_gc_control)(fixed, 0, 0) != 0
            || MOLT_GC_PROBE(_gc_control)((PyObject *)variable, 0, 0) != 0) {
        result = 5;
        goto done;
    }
done:
    PyObject_GC_UnTrack(fixed);
    PyObject_GC_UnTrack(variable);
    PyObject_GC_Del(fixed);
    PyObject_GC_Del(variable);
    return result;
}

Py_ssize_t MOLT_GC_PROBE(_gc_collect)(void) {
    int enabled = PyGC_IsEnabled();
    if (PyGC_Disable() != enabled || PyGC_IsEnabled() != 0) return -1;
    if (PyGC_Enable() != 0 || PyGC_IsEnabled() != 1) return -2;
    if (!enabled) PyGC_Disable();
    /* Explicit collection remains effective while automatic GC is disabled. */
    return PyGC_Collect();
}


/* Count/index and containment deliberately enter through the installed API
 * from each existing header transport, with no header-local implementation. */
Py_ssize_t MOLT_GC_PROBE(_sequence_search)(PyObject *value, PyObject *needle, int operation) {
    if (operation == 0) return PySequence_Contains(value, needle);
    if (operation == 1) return PySequence_Count(value, needle);
    if (operation == 3) return PySet_Contains(value, needle);
    if (operation == 4) return PySet_Discard(value, needle);
    return PySequence_Index(value, needle);
}


int MOLT_GC_PROBE(_sequence_check)(PyObject *value) {
    return PySequence_Check(value);
}

int MOLT_GC_PROBE(_sequence_materialization)(PyObject *value, PyObject *first) {
    int result = 0;
    PyObject *fast = PySequence_Fast(value, "sequence required");
    PyObject *list = NULL, *tuple = NULL;
    if (fast == NULL) { result = 1; goto done; }
    if (fast != value || PySequence_Fast_GET_SIZE(fast) != 2) { result = 2; goto done; }
    if (PySequence_Fast_GET_ITEM(fast, 0) != first) { result = 3; goto done; }
    PyObject **items = PySequence_Fast_ITEMS(fast);
    if (items == NULL || items[0] != first) { result = 4; goto done; }
    list = PySequence_List(value);
    tuple = PySequence_Tuple(value);
    if (list == NULL || tuple == NULL) { result = 5; goto done; }
    if (list == value || !PyList_CheckExact(list) || !PyTuple_CheckExact(tuple)) { result = 6; goto done; }
    if (PySequence_Fast_GET_SIZE(tuple) != 2 || PySequence_Fast_GET_ITEM(tuple, 0) != first) result = 7;
done:
    Py_XDECREF(tuple);
    Py_XDECREF(list);
    Py_XDECREF(fast);
    return result;
}

Py_ssize_t MOLT_GC_PROBE(_length_hint)(PyObject *value, Py_ssize_t defaultvalue) {
    return PyObject_LengthHint(value, defaultvalue);
}


/* Both installed header transports consume the exact same iteration boundary. */
int MOLT_GC_PROBE(_iteration)(PyObject *value, PyObject *completion) {
    int status = 0;
    PyObject *iter = PyObject_GetIter(value);
    PyObject *item = NULL, *result = NULL;
    if (iter == NULL) return 1;
    if (iter != value || !PyIter_Check(iter)) { status = 2; goto done; }
    item = PyIter_Next(iter);
    if (item == NULL || PyFloat_AsDouble(item) != 0.0) { status = 3; goto done; }
    Py_CLEAR(item);
    if (PyIter_Send(iter, Py_None, &result) != PYGEN_RETURN) { status = 4; goto done; }
    if (result != completion) { status = 5; goto done; }
    Py_CLEAR(result);
    item = PyObject_Next(iter);
    if (item != NULL || PyErr_Occurred() != NULL) { status = 6; goto done; }
    item = PyObject_SelfIter(iter);
    if (item != iter) { status = 7; goto done; }
done:
    Py_XDECREF(item);
    Py_XDECREF(result);
    Py_DECREF(iter);
    return status;
}

PyObject *MOLT_GC_PROBE(_sequence_iterator_first)(PyObject *value) {
    PyObject *iter = PySeqIter_New(value);
    PyObject *item;
    if (iter == NULL) return NULL;
    item = PyIter_Next(iter);
    Py_DECREF(iter);
    return item;
}
