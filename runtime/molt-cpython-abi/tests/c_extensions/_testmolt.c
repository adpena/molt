/*
 * _testmolt.c — Minimal Python C extension for validating the Molt CPython ABI.
 *
 * Exports three functions:
 *   add(a: int, b: int) -> int    -- integer addition via PyArg_ParseTuple "ii"
 *   hello() -> str                 -- static string return
 *   reverse(s: str) -> str         -- UTF-8 string reversal
 *
 * Compile:
 *   cc -O2 -shared -fPIC \
 *      -I../../include \
 *      _testmolt.c \
 *      -L<cargo-target>/release -lmolt_cpython_abi \
 *      -o _testmolt.cpython-312-darwin.so
 */

#include <Python.h>
#include <string.h>

/* ── add(a, b) → a + b ──────────────────────────────────────────────────── */

static PyObject *
testmolt_add(PyObject *self, PyObject *args)
{
    int a = 0, b = 0;
    if (!PyArg_ParseTuple(args, "ii", &a, &b))
        return NULL;
    return PyLong_FromLong((long)(a + b));
}

/* ── hello() → "Hello from Molt!" ──────────────────────────────────────── */

static PyObject *
testmolt_hello(PyObject *self, PyObject *args)
{
    return PyUnicode_FromString("Hello from Molt!");
}

/* ── reverse(s) → reversed str ─────────────────────────────────────────── */

static PyObject *
testmolt_reverse(PyObject *self, PyObject *args)
{
    const char *s = NULL;
    Py_ssize_t len = 0;
    if (!PyArg_ParseTuple(args, "s#", &s, &len))
        return NULL;
    if (len == 0)
        return PyUnicode_FromString("");

    char *buf = (char *)malloc((size_t)len + 1);
    if (!buf)
        return NULL;
    for (Py_ssize_t i = 0; i < len; i++)
        buf[i] = s[len - 1 - i];
    buf[len] = '\0';
    PyObject *result = PyUnicode_FromStringAndSize(buf, len);
    free(buf);
    return result;
}

/* ── sum_list(lst) → sum of int items ──────────────────────────────────── */

static PyObject *
testmolt_sum_list(PyObject *self, PyObject *args)
{
    PyObject *lst = NULL;
    if (!PyArg_ParseTuple(args, "O", &lst))
        return NULL;
    if (!PyList_Check(lst)) {
        PyErr_SetString(PyExc_TypeError, "argument must be a list");
        return NULL;
    }
    long total = 0;
    Py_ssize_t n = PyList_Size(lst);
    for (Py_ssize_t i = 0; i < n; i++) {
        PyObject *item = PyList_GET_ITEM(lst, i);
        if (!PyLong_Check(item)) {
            PyErr_SetString(PyExc_TypeError, "list items must be ints");
            return NULL;
        }
        total += PyLong_AsLong(item);
    }
    return PyLong_FromLong(total);
}

/* ── Method table ───────────────────────────────────────────────────────── */

static PyMethodDef testmolt_methods[] = {
    {"add",      testmolt_add,      METH_VARARGS, "Add two integers."},
    {"hello",    testmolt_hello,    METH_VARARGS, "Return a greeting string."},
    {"reverse",  testmolt_reverse,  METH_VARARGS, "Reverse a string."},
    {"sum_list", testmolt_sum_list, METH_VARARGS, "Sum a list of integers."},
    PY_METHODDEF_SENTINEL,
};

/* ── Module definition ──────────────────────────────────────────────────── */

static PyModuleDef testmolt_module = {
    PyModuleDef_HEAD_INIT,
    "_testmolt",                   /* m_name  */
    "Molt CPython ABI test module",/* m_doc   */
    -1,                            /* m_size  */
    testmolt_methods,              /* m_methods */
    NULL, NULL, NULL, NULL,        /* slots / traverse / clear / free */
};

/* ── Module init ─────────────────────────────────────────────────────────── */

PyMODINIT_FUNC
PyInit__testmolt(void)
{
    PyObject *m = PyModule_Create(&testmolt_module);
    if (m == NULL)
        return NULL;
    PyModule_AddIntConstant(m, "VERSION_MAJOR", 3);
    PyModule_AddIntConstant(m, "VERSION_MINOR", 12);
    PyModule_AddStringConstant(m, "RUNTIME", "molt");
    return m;
}
