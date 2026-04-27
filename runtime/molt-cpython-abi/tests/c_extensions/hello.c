/*
 * hello.c — Minimal METH_NOARGS C extension for the dlopen-loader smoke test.
 *
 * Compile via tools/scripts/build-cext.sh after building the
 * molt-lang-cpython-abi dylib.  The companion extension_manifest.json
 * provides the metadata Molt's importlib enforcement requires.
 */

#define PY_SSIZE_T_CLEAN
#include <Python.h>

static PyObject *hello_greet(PyObject *self, PyObject *args)
{
    (void)self;
    (void)args;
    return PyUnicode_FromString("hello from C");
}

static PyMethodDef hello_methods[] = {
    {"greet", hello_greet, METH_NOARGS, "Return a greeting from a C extension."},
    PY_METHODDEF_SENTINEL,
};

static struct PyModuleDef hello_module = {
    PyModuleDef_HEAD_INIT,
    "hello",
    "Smoke-test extension for the molt dlopen loader.",
    -1,
    hello_methods,
    NULL, NULL, NULL, NULL,
};

PyMODINIT_FUNC PyInit_hello(void)
{
    return PyModule_Create(&hello_module);
}
