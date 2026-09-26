/*
 * hello.c — Minimal single-phase C extension for the native loader smoke test.
 *
 * Compiled by runtime/test_support/cext_fixture.rs into Cargo-owned test
 * artifacts and linked against the exact molt-runtime `cext_host` example
 * image, which carries the runtime and the CPython-ABI exports together.
 *
 * Both functions and data are imported from the same host. The fixture builder
 * selects shared-image linkage explicitly, including Windows data imports.
 */

#define PY_SSIZE_T_CLEAN
#include <Python.h>

static PyObject *hello_greet(PyObject *self, PyObject *args)
{
    (void)self;
    (void)args;
    return PyUnicode_FromString("hello from C");
}

static PyObject *hello_fail(PyObject *self, PyObject *args)
{
    (void)args;
    PyObject *hello_error = PyObject_GetAttrString(self, "HelloError");
    if (hello_error == NULL) {
        return NULL;
    }
    PyErr_SetString(hello_error, "deliberate hello failure");
    Py_DECREF(hello_error);
    return NULL;
}

static PyMethodDef hello_methods[] = {
    {"greet", hello_greet, METH_NOARGS, "Return a greeting from a C extension."},
    {"fail", hello_fail, METH_NOARGS, "Raise hello.HelloError from C."},
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
    PyObject *module = PyModule_Create(&hello_module);
    if (module == NULL) {
        return NULL;
    }
    if (Py_TYPE(module) != &PyModule_Type) {
        PyErr_SetString(PyExc_SystemError, "module type belongs to a different ABI image");
        Py_DECREF(module);
        return NULL;
    }
    PyObject *hello_error = PyErr_NewException("hello.HelloError", PyExc_ValueError, NULL);
    if (hello_error == NULL
        || PyModule_AddObjectRef(module, "HelloError", hello_error) < 0) {
        Py_CLEAR(hello_error);
        Py_DECREF(module);
        return NULL;
    }
    Py_DECREF(hello_error);
    return module;
}

/*
 * Host probes, not Python-visible. They return addresses of this image's
 * static definitions and of the PyModule_Create2 export its import binding
 * resolved to, so the host can prove it is the image this extension uses.
 */
#if defined(_WIN32)
/* On Windows &PyModule_Create2 names this image's import thunk; the import
 * address table slot holds the resolved export. */
#if defined(_M_IX86) || defined(__i386__)
extern void *_imp__PyModule_Create2;
#define HELLO_BOUND_MODULE_CREATE2 _imp__PyModule_Create2
#else
extern void *__imp_PyModule_Create2;
#define HELLO_BOUND_MODULE_CREATE2 __imp_PyModule_Create2
#endif
#else
#define HELLO_BOUND_MODULE_CREATE2 ((void *)&PyModule_Create2)
#endif

MOLT_MODULE_INIT_EXPORT const void *hello_probe_module_create2(void)
{
    return HELLO_BOUND_MODULE_CREATE2;
}

MOLT_MODULE_INIT_EXPORT const void *hello_probe_methods(void)
{
    return hello_methods;
}

MOLT_MODULE_INIT_EXPORT const void *hello_probe_module_def(void)
{
    return &hello_module;
}
