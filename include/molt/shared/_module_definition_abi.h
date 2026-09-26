/* One PyModuleDef layout and initializer authority for both Molt headers.
 * Include after PyObject, Py_ssize_t, and PyMethodDef are defined. */
#ifndef MOLT_MODULE_DEFINITION_ABI_H
#define MOLT_MODULE_DEFINITION_ABI_H

typedef struct PyModuleDef_Slot {
    int slot;
    void *value;
} PyModuleDef_Slot;

typedef struct PyModuleDef_Base {
    PyObject_HEAD
    PyObject *(*m_init)(void);
    Py_ssize_t m_index;
    PyObject *m_copy;
} PyModuleDef_Base;

#define PyModuleDef_HEAD_INIT { 1, NULL, NULL, 0, NULL }

typedef struct PyModuleDef {
    PyModuleDef_Base m_base;
    const char *m_name;
    const char *m_doc;
    Py_ssize_t m_size;
    PyMethodDef *m_methods;
    PyModuleDef_Slot *m_slots;
    int (*m_traverse)(PyObject *, int (*)(PyObject *, void *), void *);
    int (*m_clear)(PyObject *);
    void (*m_free)(void *);
} PyModuleDef;

#endif /* MOLT_MODULE_DEFINITION_ABI_H */
