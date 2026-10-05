/* Independent CPython oracle. Uses only Python's public C API and real slots. */
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <structmember.h>
#include <stddef.h>

typedef struct {
    PyObject_HEAD
    PyObject *dictionary;
    Py_ssize_t sets;
    Py_ssize_t deletes;
    int fail;
    char blocking;
} Probe;

static PyObject *probe_new(PyTypeObject *type, PyObject *args, PyObject *kwargs) {
    Probe *self = (Probe *)type->tp_alloc(type, 0);
    if (self) self->blocking = 1;
    return (PyObject *)self;
}

static void probe_dealloc(Probe *self) {
    Py_XDECREF(self->dictionary);
    Py_TYPE(self)->tp_free((PyObject *)self);
}

static int probe_setattro(PyObject *object, PyObject *name, PyObject *value) {
    Probe *self = (Probe *)object;
    if (value) self->sets++;
    else self->deletes++;
    if (self->fail) {
        PyErr_SetString(PyExc_ValueError, "native mutation sentinel");
        return -1;
    }
    return PyObject_GenericSetAttr(object, name, value);
}

static PyGetSetDef probe_getset[] = {
    {"__dict__", PyObject_GenericGetDict, PyObject_GenericSetDict, NULL, NULL},
    {NULL}
};

static PyMemberDef probe_members[] = {
    {"_asyncio_future_blocking", T_BOOL, offsetof(Probe, blocking), 0, NULL},
    {NULL}
};

static PyTypeObject Probe_Type = {
    PyVarObject_HEAD_INIT(NULL, 0)
    .tp_name = "attribute_mutation_probe.Probe",
    .tp_basicsize = sizeof(Probe),
    .tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE,
    .tp_new = probe_new,
    .tp_dealloc = (destructor)probe_dealloc,
    .tp_setattro = probe_setattro,
    .tp_getattro = PyObject_GenericGetAttr,
    .tp_dictoffset = offsetof(Probe, dictionary),
    .tp_getset = probe_getset,
    .tp_members = probe_members,
};

static PyObject *raw_set(PyObject *module, PyObject *args) {
    PyObject *object, *name, *value;
    if (!PyArg_ParseTuple(args, "OOO", &object, &name, &value)) return NULL;
    if (PyObject_GenericSetAttr(object, name, value) < 0) return NULL;
    Py_RETURN_NONE;
}

static PyObject *raw_delete(PyObject *module, PyObject *args) {
    PyObject *object, *name;
    if (!PyArg_ParseTuple(args, "OO", &object, &name)) return NULL;
    if (PyObject_GenericSetAttr(object, name, NULL) < 0) return NULL;
    Py_RETURN_NONE;
}

static PyObject *counts(PyObject *module, PyObject *object) {
    if (!PyObject_TypeCheck(object, &Probe_Type)) {
        PyErr_SetString(PyExc_TypeError, "expected Probe");
        return NULL;
    }
    Probe *self = (Probe *)object;
    return Py_BuildValue("nn", self->sets, self->deletes);
}

static PyObject *set_failure(PyObject *module, PyObject *args) {
    Probe *self;
    int fail;
    if (!PyArg_ParseTuple(args, "O!p", &Probe_Type, &self, &fail)) return NULL;
    self->fail = fail;
    Py_RETURN_NONE;
}

static PyObject *slot_type(PyObject *module, PyObject *unused) {
    PyType_Slot slots[] = {
        {Py_tp_new, PyType_GenericNew},
        {Py_tp_getattro, PyObject_GenericGetAttr},
        {0, NULL}
    };
    PyType_Spec spec = {"attribute_mutation_probe.NativeSlots", sizeof(PyObject), 0,
                        Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE, slots};
    return PyType_FromSpec(&spec);
}

static int watcher_id = -1;
static Py_ssize_t watcher_calls = 0;

static int watch_callback(PyObject *type) {
    watcher_calls++;
    return 0;
}

static PyObject *watch_start(PyObject *module, PyObject *type) {
    if (watcher_id >= 0) {
        PyErr_SetString(PyExc_RuntimeError, "watch already active");
        return NULL;
    }
    watcher_id = PyType_AddWatcher(watch_callback);
    if (watcher_id < 0) return NULL;
    if (PyType_Watch(watcher_id, type) < 0) {
        PyType_ClearWatcher(watcher_id);
        watcher_id = -1;
        return NULL;
    }
    watcher_calls = 0;
    Py_RETURN_NONE;
}

static PyObject *watch_arm(PyObject *module, PyObject *type) {
    if (!PyType_Check(type)) {
        PyErr_SetString(PyExc_TypeError, "expected type");
        return NULL;
    }
    if (!PyUnstable_Type_AssignVersionTag((PyTypeObject *)type)) {
        PyErr_SetString(PyExc_RuntimeError, "version tag assignment failed");
        return NULL;
    }
    watcher_calls = 0;
    Py_RETURN_NONE;
}

static PyObject *watch_state(PyObject *module, PyObject *type) {
    if (!PyType_Check(type)) {
        PyErr_SetString(PyExc_TypeError, "expected type");
        return NULL;
    }
    return Py_BuildValue("nI", watcher_calls, ((PyTypeObject *)type)->tp_version_tag);
}

static PyObject *is_abstract(PyObject *module, PyObject *type) {
    if (!PyType_Check(type)) {
        PyErr_SetString(PyExc_TypeError, "expected type");
        return NULL;
    }
    return PyBool_FromLong((((PyTypeObject *)type)->tp_flags & Py_TPFLAGS_IS_ABSTRACT) != 0);
}

static PyObject *watch_stop(PyObject *module, PyObject *type) {
    if (watcher_id >= 0) {
        if (PyType_Unwatch(watcher_id, type) < 0) return NULL;
        if (PyType_ClearWatcher(watcher_id) < 0) return NULL;
        watcher_id = -1;
    }
    Py_RETURN_NONE;
}

static PyMethodDef methods[] = {
    {"is_abstract", is_abstract, METH_O, NULL},
    {"watch_start", watch_start, METH_O, NULL},
    {"watch_arm", watch_arm, METH_O, NULL},
    {"watch_state", watch_state, METH_O, NULL},
    {"watch_stop", watch_stop, METH_O, NULL},
    {"slot_type", slot_type, METH_NOARGS, NULL},
    {"raw_set", raw_set, METH_VARARGS, NULL},
    {"raw_delete", raw_delete, METH_VARARGS, NULL},
    {"counts", counts, METH_O, NULL},
    {"set_failure", set_failure, METH_VARARGS, NULL},
    {NULL}
};

static struct PyModuleDef definition = {
    PyModuleDef_HEAD_INIT, "attribute_mutation_probe", NULL, -1, methods
};

PyMODINIT_FUNC PyInit_attribute_mutation_probe(void) {
    if (PyType_Ready(&Probe_Type) < 0) return NULL;
    PyObject *module = PyModule_Create(&definition);
    if (!module) return NULL;
    if (PyModule_AddObjectRef(module, "Probe", (PyObject *)&Probe_Type) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    return module;
}
