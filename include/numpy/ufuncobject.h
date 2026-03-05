#ifndef MOLT_NUMPY_UFUNCOBJECT_H
#define MOLT_NUMPY_UFUNCOBJECT_H

#include <numpy/ndarrayobject.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)

static inline int PyUFunc_GiveFloatingpointErrors(const char *name, int fpe_errors) {
    (void)name;
    (void)fpe_errors;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUFunc_GiveFloatingpointErrors is not yet implemented in Molt's NumPy compatibility layer");
    return -1;
}

#ifdef __cplusplus
}
#endif

#endif
