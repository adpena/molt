#ifndef MOLT_NUMPY_ARRAYSCALARS_H
#define MOLT_NUMPY_ARRAYSCALARS_H

/*
 * Source-compat overlay derived from NumPy 2.4.2 public arrayscalars.h.
 * Molt keeps this intentionally partial and compile-focused.
 */

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef PyObject PyStringScalarObject;

PyAPI_DATA(PyBoolScalarObject) _PyArrayScalar_BoolValues[2];

#ifndef Py_LIMITED_API
typedef struct {
    PyObject_HEAD
    Py_UCS4 *obval;
#if NPY_FEATURE_VERSION >= NPY_1_20_API_VERSION
    char *buffer_fmt;
#endif
} PyUnicodeScalarObject;
#endif

#define PyArrayScalar_New(cls) \
    Py##cls##ArrType_Type.tp_alloc(&Py##cls##ArrType_Type, 0)

#define PyArrayScalar_False ((PyObject *)(&(_PyArrayScalar_BoolValues[0])))
#define PyArrayScalar_True ((PyObject *)(&(_PyArrayScalar_BoolValues[1])))
#define PyArrayScalar_FromLong(i) \
    ((PyObject *)(&(_PyArrayScalar_BoolValues[((i) != 0)])))
#define PyArrayScalar_RETURN_BOOL_FROM_LONG(i) \
    do { \
        PyObject *_molt_bool_scalar = PyArrayScalar_FromLong(i); \
        Py_INCREF(_molt_bool_scalar); \
        return _molt_bool_scalar; \
    } while (0)
#define PyArrayScalar_RETURN_FALSE \
    do { \
        Py_INCREF(PyArrayScalar_False); \
        return PyArrayScalar_False; \
    } while (0)
#define PyArrayScalar_RETURN_TRUE \
    do { \
        Py_INCREF(PyArrayScalar_True); \
        return PyArrayScalar_True; \
    } while (0)

#ifndef Py_LIMITED_API
#define PyArrayScalar_VAL(obj, cls) (((Py##cls##ScalarObject *)(obj))->obval)
#define PyArrayScalar_ASSIGN(obj, cls, val) \
    (PyArrayScalar_VAL((obj), cls) = (val))
#endif

#ifdef __cplusplus
}
#endif

#endif
