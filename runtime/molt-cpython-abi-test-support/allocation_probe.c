#ifdef _MSC_VER
#define _Thread_local __declspec(thread)
#endif
#ifdef MOLT_PUBLIC_HEADER_PROBE
#include <molt/Python.h>
#else
#include <Python.h>
#endif

#define MOLT_ALLOC_JOIN_INNER(left, right) left##right
#define MOLT_ALLOC_JOIN(left, right) MOLT_ALLOC_JOIN_INNER(left, right)
#define MOLT_ALLOC_PROBE(suffix) MOLT_ALLOC_JOIN(MOLT_ALLOCATION_PROBE, suffix)

static int allocator_calls;
static PyTypeObject *allocator_type;
static Py_ssize_t allocator_items;

PyObject *MOLT_ALLOC_PROBE(_allocator)(PyTypeObject *type, Py_ssize_t nitems) {
    ++allocator_calls;
    allocator_type = type;
    allocator_items = nitems;
    return _PyObject_New(type);
}

/* Rust supplies complete native type layouts. Both C facades consume only
 * opaque type pointers and their shared object/variable-object headers. */
int MOLT_ALLOCATION_PROBE(PyTypeObject *fixed_type, PyTypeObject *variable_type,
                          PyTypeObject *custom_type, Py_ssize_t fixed_size) {
    PyObject fixed_init = {0}, fixed_macro = {0};
    PyVarObject variable_init = {0}, variable_macro = {0};
    Py_ssize_t fixed_refs = Py_REFCNT(fixed_type);
    Py_ssize_t variable_refs = Py_REFCNT(variable_type);
    Py_ssize_t custom_refs = Py_REFCNT(custom_type);
    PyObject *objects[6] = {NULL};
    PyTypeObject *owners[6] = {
        fixed_type, fixed_type, variable_type, variable_type, variable_type, custom_type
    };
    PyObject *(*generic_alloc)(PyTypeObject *, Py_ssize_t) = PyType_GenericAlloc;
    PyObject *(*generic_new)(PyTypeObject *, PyObject *, PyObject *) = PyType_GenericNew;
    int result = 0;

    if (PyObject_Init(&fixed_init, fixed_type) != &fixed_init
            || fixed_init.ob_type != fixed_type || fixed_init.ob_refcnt != 1
            || Py_REFCNT(fixed_type) != fixed_refs + 1) return 1;
    Py_DECREF((PyObject *)fixed_type); /* retire the stack fixture's type owner */
    if (PyObject_INIT(&fixed_macro, fixed_type) != &fixed_macro
            || fixed_macro.ob_type != fixed_type || fixed_macro.ob_refcnt != 1
            || Py_REFCNT(fixed_type) != fixed_refs + 1) return 2;
    Py_DECREF((PyObject *)fixed_type);
    if (PyObject_InitVar(&variable_init, variable_type, 7) != &variable_init
            || variable_init.ob_type != variable_type || variable_init.ob_refcnt != 1
            || Py_SIZE(&variable_init) != 7
            || Py_REFCNT(variable_type) != variable_refs + 1) return 3;
    Py_DECREF((PyObject *)variable_type);
    if (PyObject_INIT_VAR(&variable_macro, variable_type, 9) != &variable_macro
            || variable_macro.ob_type != variable_type || variable_macro.ob_refcnt != 1
            || Py_SIZE(&variable_macro) != 9
            || Py_REFCNT(variable_type) != variable_refs + 1) return 4;
    Py_DECREF((PyObject *)variable_type);

    allocator_calls = 0;
    allocator_type = NULL;
    allocator_items = -1;
    objects[0] = _PyObject_New(fixed_type);
    objects[1] = PyObject_New(PyObject, fixed_type);
    objects[2] = (PyObject *)_PyObject_NewVar(variable_type, 3);
    objects[3] = (PyObject *)PyObject_NewVar(PyVarObject, variable_type, 4);
    objects[4] = generic_alloc(variable_type, 5);
    objects[5] = generic_new(custom_type, NULL, NULL);
    for (int index = 0; index < 6; ++index) {
        if (objects[index] == NULL || objects[index]->ob_refcnt != 1
                || Py_TYPE(objects[index]) != owners[index]) {
            result = 10 + index;
            goto done;
        }
    }
    if (allocator_calls != 1 || allocator_type != custom_type || allocator_items != 0) {
        result = 20;
        goto done;
    }
    if (Py_REFCNT(fixed_type) != fixed_refs + 2
            || Py_REFCNT(variable_type) != variable_refs + 3
            || Py_REFCNT(custom_type) != custom_refs + 1) {
        result = 21;
        goto done;
    }
    for (int index = 2; index <= 4; ++index) {
        PyVarObject *variable = (PyVarObject *)objects[index];
        Py_ssize_t items = index + 1;
        if (Py_SIZE(variable) != items) {
            result = 22;
            goto done;
        }
        for (Py_ssize_t item = 0; item < items; ++item) {
            if (((PyObject **)(variable + 1))[item] != NULL) {
                result = 23;
                goto done;
            }
            ((PyObject **)(variable + 1))[item] = objects[0];
        }
    }
    for (int index = 0; index < 6; ++index) {
        if (index >= 2 && index <= 4) continue;
        for (Py_ssize_t byte = sizeof(PyObject); byte < fixed_size; ++byte) {
            if (((unsigned char *)objects[index])[byte] != 0) {
                result = 24;
                goto done;
            }
        }
        ((unsigned char *)objects[index])[fixed_size - 1] = 0x5a;
    }
done:
    for (int index = 0; index < 6; ++index) {
        if (objects[index] != NULL) {
            PyObject_Free(objects[index]);
            Py_DECREF((PyObject *)owners[index]);
        }
    }
    if (!result && (Py_REFCNT(fixed_type) != fixed_refs
            || Py_REFCNT(variable_type) != variable_refs
            || Py_REFCNT(custom_type) != custom_refs)) return 25;
    return result;
}
