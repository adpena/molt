#ifndef MOLT_NUMPY_NPY_MATH_H
#define MOLT_NUMPY_NPY_MATH_H

#include <complex.h>
#include <math.h>

#include <numpy/numpyconfig.h>
#include <numpy/npy_common.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyArray_MAX(a, b) ((a) > (b) ? (a) : (b))
#define PyArray_MIN(a, b) ((a) < (b) ? (a) : (b))
#define npy_isnan(x) isnan((x))
#define npy_isinf(x) isinf((x))

static inline double npy_creal(const npy_cdouble z) {
#if defined(__cplusplus)
    return z._Val[0];
#elif defined(__STDC_NO_COMPLEX__)
    return z.real;
#else
    return creal(z);
#endif
}

static inline double npy_cimag(const npy_cdouble z) {
#if defined(__cplusplus)
    return z._Val[1];
#elif defined(__STDC_NO_COMPLEX__)
    return z.imag;
#else
    return cimag(z);
#endif
}

static inline void npy_csetreal(npy_cdouble *z, const double r) {
#if defined(__cplusplus)
    z->_Val[0] = r;
#elif defined(__STDC_NO_COMPLEX__)
    z->real = r;
#else
    *z = r + npy_cimag(*z) * I;
#endif
}

static inline void npy_csetimag(npy_cdouble *z, const double i) {
#if defined(__cplusplus)
    z->_Val[1] = i;
#elif defined(__STDC_NO_COMPLEX__)
    z->imag = i;
#else
    *z = npy_creal(*z) + i * I;
#endif
}

static inline float npy_crealf(const npy_cfloat z) {
#if defined(__cplusplus)
    return z._Val[0];
#elif defined(__STDC_NO_COMPLEX__)
    return z.real;
#else
    return crealf(z);
#endif
}

static inline float npy_cimagf(const npy_cfloat z) {
#if defined(__cplusplus)
    return z._Val[1];
#elif defined(__STDC_NO_COMPLEX__)
    return z.imag;
#else
    return cimagf(z);
#endif
}

static inline void npy_csetrealf(npy_cfloat *z, const float r) {
#if defined(__cplusplus)
    z->_Val[0] = r;
#elif defined(__STDC_NO_COMPLEX__)
    z->real = r;
#else
    *z = (npy_cfloat)(r + npy_cimagf(*z) * I);
#endif
}

static inline void npy_csetimagf(npy_cfloat *z, const float i) {
#if defined(__cplusplus)
    z->_Val[1] = i;
#elif defined(__STDC_NO_COMPLEX__)
    z->imag = i;
#else
    *z = (npy_cfloat)(npy_crealf(*z) + i * I);
#endif
}

static inline npy_longdouble npy_creall(const npy_clongdouble z) {
#if defined(__cplusplus)
    return (npy_longdouble)z._Val[0];
#elif defined(__STDC_NO_COMPLEX__)
    return z.real;
#else
    return creall(z);
#endif
}

static inline npy_longdouble npy_cimagl(const npy_clongdouble z) {
#if defined(__cplusplus)
    return (npy_longdouble)z._Val[1];
#elif defined(__STDC_NO_COMPLEX__)
    return z.imag;
#else
    return cimagl(z);
#endif
}

static inline void npy_csetreall(npy_clongdouble *z, const npy_longdouble r) {
#if defined(__cplusplus)
    z->_Val[0] = r;
#elif defined(__STDC_NO_COMPLEX__)
    z->real = r;
#else
    *z = (npy_clongdouble)(r + npy_cimagl(*z) * I);
#endif
}

static inline void npy_csetimagl(npy_clongdouble *z, const npy_longdouble i) {
#if defined(__cplusplus)
    z->_Val[1] = i;
#elif defined(__STDC_NO_COMPLEX__)
    z->imag = i;
#else
    *z = (npy_clongdouble)(npy_creall(*z) + i * I);
#endif
}

#define NPY_CSETREAL(z, r) npy_csetreal((z), (r))
#define NPY_CSETIMAG(z, i) npy_csetimag((z), (i))
#define NPY_CSETREALF(z, r) npy_csetrealf((z), (r))
#define NPY_CSETIMAGF(z, i) npy_csetimagf((z), (i))
#define NPY_CSETREALL(z, r) npy_csetreall((z), (r))
#define NPY_CSETIMAGL(z, i) npy_csetimagl((z), (i))

NPY_NO_EXPORT int npy_clear_floatstatus_barrier(char *param);
NPY_NO_EXPORT int npy_get_floatstatus_barrier(char *param);

#ifdef __cplusplus
}
#endif

#endif
