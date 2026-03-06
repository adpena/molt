#ifndef MOLT_NUMPY_NPY_MATH_H
#define MOLT_NUMPY_NPY_MATH_H

#include <math.h>

#include <numpy/npy_common.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyArray_MAX(a, b) ((a) > (b) ? (a) : (b))
#define PyArray_MIN(a, b) ((a) < (b) ? (a) : (b))
#define npy_isnan(x) isnan((x))
#define npy_isinf(x) isinf((x))

#ifdef __cplusplus
}
#endif

#endif
