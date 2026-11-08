#include <Python.h>

#if !defined(__wasi__)
#if defined(_WIN32)
#include <windows.h>
#else
#include <pthread.h>
#endif

static int thread_callback_hits = 0;
static int count_thread_callback(void *unused) {
    (void)unused;
    thread_callback_hits++;
    return 0;
}

struct thread_publication { int result; };
#if defined(_WIN32)
static DWORD WINAPI publish_from_thread(LPVOID arg) {
#else
static void *publish_from_thread(void *arg) {
#endif
    struct thread_publication *publication = arg;
    publication->result = Py_AddPendingCall(count_thread_callback, NULL);
    return 0;
}

static PyObject *arm_thread_callback(PyObject *self, PyObject *args) {
    (void)self;
    (void)args;
    struct thread_publication publication = {-1};
#if defined(_WIN32)
    HANDLE thread = CreateThread(NULL, 0, publish_from_thread, &publication, 0, NULL);
    if (thread == NULL) {
        return PyErr_SetFromWindowsErr(0);
    }
    PyThreadState *saved = PyEval_SaveThread();
    DWORD wait = WaitForSingleObject(thread, INFINITE);
    PyEval_RestoreThread(saved);
    CloseHandle(thread);
    if (wait != WAIT_OBJECT_0) {
        Py_FatalError("pending-call publisher join failed");
    }
#else
    pthread_t thread;
    if (pthread_create(&thread, NULL, publish_from_thread, &publication) != 0) {
        PyErr_SetString(PyExc_RuntimeError, "pending-call publisher creation failed");
        return NULL;
    }
    PyThreadState *saved = PyEval_SaveThread();
    int joined = pthread_join(thread, NULL);
    PyEval_RestoreThread(saved);
    if (joined != 0) {
        /* Never return while a native child can still borrow our stack. */
        Py_FatalError("pending-call publisher join failed");
    }
#endif
    if (publication.result != 0) {
        PyErr_SetString(PyExc_RuntimeError, "pending-call queue full");
        return NULL;
    }
    /* The publisher used no GIL and never ran the callback. Release/reacquire
       around the blocking join also lets CPython 3.12 recompute its main-thread
       breaker (its foreign publisher recomputes from the publisher's identity).
       No Py_MakePendingCalls here: the interpreter's safepoint owns dispatch. */
    Py_RETURN_NONE;
}

static PyObject *thread_callback_count(PyObject *self, PyObject *args) {
    (void)self;
    (void)args;
    return PyLong_FromLong(thread_callback_hits);
}
#endif

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
#if !defined(__wasi__)
    {"arm_thread_callback", arm_thread_callback, METH_NOARGS, NULL},
    {"thread_callback_count", thread_callback_count, METH_NOARGS, NULL},
#endif
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
    /* No bridge/API call may make these fields ready for this probe. An
       extension can read exported type data directly on entering PyInit. */
    PyTypeObject *types[] = {
        &PyBaseObject_Type, &PyType_Type, &PyLong_Type, &PyTuple_Type,
        (PyTypeObject *)PyExc_BaseException, (PyTypeObject *)PyExc_NameError,
    };
    for (size_t i = 0; i < sizeof(types) / sizeof(types[0]); i++) {
        /* CPython 3.12 stores static builtin dictionaries per interpreter,
           including static exceptions, leaving physical tp_dict NULL. All
           six expose bases/MRO and READY directly, before any API call. */
        if (types[i]->tp_mro == NULL ||
                types[i]->tp_bases == NULL || !(types[i]->tp_flags & Py_TPFLAGS_READY)) {
            PyErr_Format(PyExc_RuntimeError,
                         "exported C type %s is not ready before PyInit (dict=%d mro=%d bases=%d)",
                         types[i]->tp_name, types[i]->tp_dict != NULL,
                         types[i]->tp_mro != NULL, types[i]->tp_bases != NULL);
            return NULL;
        }
    }
    for (size_t i = 0; i < sizeof(types) / sizeof(types[0]); i++) {
        PyObject *dict = PyType_GetDict(types[i]);
        if (dict == NULL) {
            return NULL;
        }
        if (!PyDict_Check(dict)) {
            PyErr_SetString(PyExc_RuntimeError, "exported C type dictionary is invalid");
            return NULL;
        }
    }
    return PyModule_Create(&pending_call_probe_module);
}
