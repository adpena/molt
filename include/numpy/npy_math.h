#ifndef MOLT_NUMPY_NPY_MATH_H
#define MOLT_NUMPY_NPY_MATH_H

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyArray_MAX(a, b) ((a) > (b) ? (a) : (b))
#define PyArray_MIN(a, b) ((a) < (b) ? (a) : (b))

#ifdef __cplusplus
}
#endif

#endif
