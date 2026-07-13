#ifdef _MSC_VER
#define _Thread_local __declspec(thread)
#endif
#include <Python.h>

/* Prebuilt extensions such as NumPy mutate the public object header directly
 * and tail-call _Py_Dealloc at the zero transition. Keep this witness in C:
 * routing through Py_INCREF/Py_DECREF would exercise only Molt's callable
 * surface, not the compiled CPython header contract. */
Py_ssize_t molt_l7_prebuilt_direct_refcnt(PyObject *value) {
    return value == NULL ? 0 : Py_REFCNT(value);
}

Py_ssize_t molt_l7_prebuilt_direct_incref(PyObject *value) {
    if (value == NULL) {
        return 0;
    }
    value->ob_refcnt += 1;
    return Py_REFCNT(value);
}

Py_ssize_t molt_l7_prebuilt_direct_decref(PyObject *value) {
    Py_ssize_t remaining;
    if (value == NULL || value->ob_refcnt <= 0) {
        return 0;
    }
    remaining = --value->ob_refcnt;
    if (remaining == 0) {
        _Py_Dealloc(value);
    }
    return remaining;
}

int molt_l7_overlay_numeric_probe(void) {
    PyObject *left = PyLong_FromLong(40);
    PyObject *right = PyLong_FromLong(2);
    PyObject *sum = NULL;
    PyObject *value = NULL;
    PyObject *complex_value = NULL;

    if (left == NULL || right == NULL || Py_TYPE(left) != &PyLong_Type ||
        Py_TYPE(right) != &PyLong_Type || !PyLong_CheckExact(left) ||
        !PyLong_CheckExact(right) || !PyNumber_Check(left)) {
        return 1;
    }

    sum = PyNumber_Add(left, right);
    if (sum == NULL) {
        return 20;
    }
    if (Py_TYPE(sum) != &PyLong_Type) {
        return 21;
    }
    if (!PyLong_CheckExact(sum)) {
        return 22;
    }
    if (PyLong_AsLong(sum) != 42) {
        return 23;
    }

    value = PyFloat_FromDouble(1.25);
    if (value == NULL || Py_TYPE(value) != &PyFloat_Type ||
        !PyFloat_CheckExact(value) || !PyNumber_Check(value) ||
        PyFloat_AsDouble(value) != 1.25) {
        return 3;
    }

    complex_value = PyComplex_FromDoubles(3.0, 4.0);
    if (complex_value == NULL) return 40;
    if (Py_TYPE(complex_value) != &PyComplex_Type) return 41;
    if (!PyComplex_CheckExact(complex_value)) return 42;
    if (!PyNumber_Check(complex_value)) return 43;
    if (PyComplex_RealAsDouble(complex_value) != 3.0) return 44;
    if (PyComplex_ImagAsDouble(complex_value) != 4.0) return 45;

    if (PyBool_FromLong(1) != Py_True || PyBool_FromLong(0) != Py_False) {
        return 5;
    }

    Py_DECREF(complex_value);
    Py_DECREF(value);
    Py_DECREF(sum);
    Py_DECREF(right);
    Py_DECREF(left);
    return 0;
}

int molt_l7_overlay_tuple_set_get_probe(PyObject *tuple, PyObject *value) {
    if (tuple == NULL || value == NULL || PyTuple_GET_SIZE(tuple) != 1) {
        return 1;
    }
    Py_INCREF(value);
    PyTuple_SET_ITEM(tuple, 0, value);
    return PyTuple_GET_ITEM(tuple, 0) == value ? 0 : 2;
}

long molt_l7_overlay_long_probe(PyObject *value) {
    return PyLong_AsLong(value);
}

double molt_l7_overlay_float_from_string_probe(PyObject *text) {
    PyObject *value = PyFloat_FromString(text);
    double result;
    if (value == NULL) {
        return -1.0;
    }
    result = PyFloat_AsDouble(value);
    Py_DECREF(value);
    return result;
}

double molt_l7_overlay_complex_real_probe(PyObject *value) {
    return PyComplex_RealAsDouble(value);
}
