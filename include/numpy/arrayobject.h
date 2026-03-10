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

static void **PyArray_API = NULL;

static inline int _import_array(void) {
    void *api_ptr = PyCapsule_Import("numpy.core._multiarray_umath._ARRAY_API", 0);
    if (api_ptr == NULL) {
        return -1;
    }
    PyArray_API = (void **)api_ptr;
    return 0;
}

#define import_array()                                                             \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            return NULL;                                                           \
        }                                                                          \
    } while (0)

#define import_array1(ret)                                                         \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            return (ret);                                                          \
        }                                                                          \
    } while (0)

#define import_array2(msg, ret)                                                    \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            PyErr_SetString(PyExc_ImportError, (msg));                            \
            return (ret);                                                          \
        }                                                                          \
    } while (0)

#ifdef __cplusplus
}
#endif

#endif
