/*
 * hello_ext.c -- Minimal C extension for testing Molt's dlopen-based loader.
 *
 * Exports:
 *   hello()     -> "hello from C!"
 *   add(a, b)   -> a + b
 *
 * Compile against the Molt CPython ABI:
 *   Use the Cargo-owned C-extension fixture builder in
 *   runtime/test_support/cext_fixture.rs for runtime-backed tests.
 */

#include <Python.h>

static PyObject *
hello_ext_hello(PyObject *self, PyObject *args)
{
    (void)self;
    (void)args;
    return PyUnicode_FromString("hello from C!");
}

static PyObject *
hello_ext_add(PyObject *self, PyObject *args)
{
    (void)self;
    int a = 0, b = 0;
    if (!PyArg_ParseTuple(args, "ii", &a, &b))
        return NULL;
    return PyLong_FromLong((long)(a + b));
}

static PyMethodDef hello_ext_methods[] = {
    {"hello", hello_ext_hello, METH_NOARGS,  "Say hello from C."},
    {"add",   hello_ext_add,   METH_VARARGS, "Add two integers."},
    PY_METHODDEF_SENTINEL,
};

static PyModuleDef hello_ext_module = {
    PyModuleDef_HEAD_INIT,
    "hello_ext",                    /* m_name    */
    "Test C extension for Molt",    /* m_doc     */
    -1,                             /* m_size    */
    hello_ext_methods,              /* m_methods */
    NULL, NULL, NULL, NULL,         /* slots / traverse / clear / free */
};

PyMODINIT_FUNC
PyInit_hello_ext(void)
{
    return PyModule_Create(&hello_ext_module);
}
