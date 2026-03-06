#ifndef MOLT_NUMPY_ARRAYOBJECT_H
#define MOLT_NUMPY_ARRAYOBJECT_H

#include <numpy/ndarrayobject.h>
#include <numpy/ufuncobject.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef NPY_NO_DEPRECATED_API
#define NPY_NO_DEPRECATED_API NPY_API_VERSION
#endif

#define Py_ARRAYOBJECT_H 1

#ifdef __cplusplus
}
#endif

#endif
