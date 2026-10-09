/* Canonical CPython-layout numeric scalar headers for every Molt C surface.
 * Install this shared directory with either Python.h tier. */
#ifndef MOLT_NUMERIC_SCALAR_ABI_H
#define MOLT_NUMERIC_SCALAR_ABI_H

/* CPython selects its different free-threaded object layout by macro presence,
 * including Py_GIL_DISABLED=0. Both transports expose the traditional layout;
 * Molt's atomic-refcount feature does not change this C ABI. */
#ifdef Py_GIL_DISABLED
#error "Molt C headers do not support the CPython free-threaded object layout (Py_GIL_DISABLED)"
#endif
/* The declared CPython 3.12 ABI prepends two pointers under Py_TRACE_REFS.
 * Its presence is incompatible with the release-layout structs below. */
#ifdef Py_TRACE_REFS
#error "Molt C headers do not support the CPython 3.12 trace-reference object layout (Py_TRACE_REFS)"
#endif

typedef struct _object PyObject;
typedef struct _typeobject PyTypeObject;
typedef struct _longobject PyLongObject;

#ifndef PyObject_HEAD
#define PyObject_HEAD       \
    Py_ssize_t ob_refcnt;   \
    PyTypeObject *ob_type;
#endif

#ifndef PyObject_VAR_HEAD
#define PyObject_VAR_HEAD   \
    PyObject_HEAD           \
    Py_ssize_t ob_size;
#endif

struct _object {
    PyObject_HEAD
};

typedef struct {
    PyObject_VAR_HEAD
} PyVarObject;

typedef struct {
    uintptr_t lv_tag;
    digit ob_digit[1];
} _PyLongValue;

struct _longobject {
    PyObject_HEAD
    _PyLongValue long_value;
};

typedef struct {
    PyObject_HEAD
    double ob_fval;
} PyFloatObject;

typedef struct {
    PyObject_HEAD
    Py_complex cval;
} PyComplexObject;

#endif /* MOLT_NUMERIC_SCALAR_ABI_H */
