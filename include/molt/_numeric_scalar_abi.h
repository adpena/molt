/* Canonical CPython-layout numeric scalar headers for every Molt C surface. */
#ifndef MOLT_NUMERIC_SCALAR_ABI_H
#define MOLT_NUMERIC_SCALAR_ABI_H

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
