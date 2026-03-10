#ifndef MOLT_NUMPY_UFUNCOBJECT_H
#define MOLT_NUMPY_UFUNCOBJECT_H

#include <numpy/ndarrayobject.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)

#ifdef __cplusplus
}
#endif

#endif
