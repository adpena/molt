#include <Python.h>

static int pending_runtime_error(void *unused) {
    (void)unused;
    PyErr_SetString(PyExc_RuntimeError, "pending replacement");
    return -1;
}

static PyObject *arm_runtime_error(PyObject *self, PyObject *args) {
    (void)self;
    (void)args;
    /* Called on the interpreter's main thread from the Python finally body.
       Return with work queued: each interpreter's own post-call safepoint
       must deliver it. Do not force Py_MakePendingCalls here; that would test
       synchronous C-error propagation instead of the observer boundary. */
    if (Py_AddPendingCall(pending_runtime_error, NULL) != 0) {
        PyErr_SetString(PyExc_RuntimeError, "pending-call queue full");
        return NULL;
    }
    Py_RETURN_NONE;
}

static PyMethodDef pending_call_probe_methods[] = {
    {"arm_runtime_error", arm_runtime_error, METH_NOARGS, NULL},
    {NULL, NULL, 0, NULL},
};

static PyModuleDef pending_call_probe_module = {
    PyModuleDef_HEAD_INIT,
    "_native",
    NULL,
    -1,
    pending_call_probe_methods,
};

PyMODINIT_FUNC PyInit__native(void) {
    return PyModule_Create(&pending_call_probe_module);
}
