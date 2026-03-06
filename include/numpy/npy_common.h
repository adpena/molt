#ifndef MOLT_NUMPY_NPY_COMMON_H
#define MOLT_NUMPY_NPY_COMMON_H

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef Py_USING_UNICODE
#define Py_USING_UNICODE 1
#endif

#ifndef NPY_LIKELY
#if defined(__GNUC__) || defined(__clang__)
#define NPY_LIKELY(x) __builtin_expect(!!(x), 1)
#define NPY_UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define NPY_LIKELY(x) (x)
#define NPY_UNLIKELY(x) (x)
#endif
#endif

#define PyComplex64ArrType_Type PyCFloatArrType_Type
#define PyComplex128ArrType_Type PyCDoubleArrType_Type
#define PyComplex160ArrType_Type PyCLongDoubleArrType_Type
#define PyComplex192ArrType_Type PyCLongDoubleArrType_Type
#define PyComplex256ArrType_Type PyCLongDoubleArrType_Type
#define PyInt8ArrType_Type PyByteArrType_Type
#define PyInt16ArrType_Type PyShortArrType_Type
#define PyInt32ArrType_Type PyIntArrType_Type
#if NPY_SIZEOF_LONG == 8
#define PyInt64ArrType_Type PyLongArrType_Type
#define PyUInt64ArrType_Type PyULongArrType_Type
#else
#define PyInt64ArrType_Type PyLongLongArrType_Type
#define PyUInt64ArrType_Type PyULongLongArrType_Type
#endif
#define PyUInt8ArrType_Type PyUByteArrType_Type
#define PyUInt16ArrType_Type PyUShortArrType_Type
#define PyUInt32ArrType_Type PyUIntArrType_Type
#if NPY_SIZEOF_INTP == NPY_SIZEOF_LONG
#define PyIntpArrType_Type PyLongArrType_Type
#define PyUIntpArrType_Type PyULongArrType_Type
#elif NPY_SIZEOF_INTP == NPY_SIZEOF_INT
#define PyIntpArrType_Type PyIntArrType_Type
#define PyUIntpArrType_Type PyUIntArrType_Type
#else
#define PyIntpArrType_Type PyLongLongArrType_Type
#define PyUIntpArrType_Type PyULongLongArrType_Type
#endif

#define NPY_MAX_INT64 9223372036854775807LL
#define NPY_MIN_INT64 (-NPY_MAX_INT64 - 1LL)
#define NPY_MAX_UINT64 18446744073709551615ULL
#define NPY_MIN_DATETIME NPY_MIN_INT64
#define NPY_MAX_DATETIME NPY_MAX_INT64
#define NPY_MIN_TIMEDELTA NPY_MIN_INT64
#define NPY_MAX_TIMEDELTA NPY_MAX_INT64

#if NPY_SIZEOF_LONGDOUBLE == NPY_SIZEOF_DOUBLE
#define longdouble_t double
#else
#define longdouble_t long double
#endif

#if !defined(__STDC_NO_COMPLEX__)
typedef float _Complex npy_cfloat;
typedef double _Complex npy_cdouble;
typedef longdouble_t _Complex npy_clongdouble;
#else
typedef struct {
    npy_float real;
    npy_float imag;
} npy_cfloat;

typedef struct {
    npy_double real;
    npy_double imag;
} npy_cdouble;

typedef struct {
    npy_longdouble real;
    npy_longdouble imag;
} npy_clongdouble;
#endif

typedef npy_cfloat npy_complex64;
typedef npy_cdouble npy_complex128;
typedef npy_clongdouble npy_complex160;
typedef npy_clongdouble npy_complex192;
typedef npy_clongdouble npy_complex256;

#ifndef NPY_INTP_FMT
#if defined(_WIN64)
#define NPY_INTP_FMT "lld"
#elif INTPTR_MAX == LONG_MAX
#define NPY_INTP_FMT "ld"
#elif INTPTR_MAX == INT_MAX
#define NPY_INTP_FMT "d"
#else
#define NPY_INTP_FMT "lld"
#endif
#endif

typedef PyCFloatScalarObject PyComplex64ScalarObject;
typedef PyCDoubleScalarObject PyComplex128ScalarObject;
typedef PyCLongDoubleScalarObject PyComplex160ScalarObject;
typedef PyCLongDoubleScalarObject PyComplex192ScalarObject;
typedef PyCLongDoubleScalarObject PyComplex256ScalarObject;
#ifdef __cplusplus
}
#endif

#endif
