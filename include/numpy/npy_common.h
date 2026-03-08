#ifndef MOLT_NUMPY_NPY_COMMON_H
#define MOLT_NUMPY_NPY_COMMON_H

#include <stdio.h>
#include <limits.h>

#ifdef _WIN32
#include <io.h>
#else
#include <sys/types.h>
#include <unistd.h>
#endif

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

#ifndef NPY_STEALS_REF_TO_ARG
#ifdef WITH_CPYCHECKER_STEALS_REFERENCE_TO_ARG_ATTRIBUTE
#define NPY_STEALS_REF_TO_ARG(n) \
    __attribute__((cpychecker_steals_reference_to_arg(n)))
#else
#define NPY_STEALS_REF_TO_ARG(n)
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
#define NPY_MAX_INT INT_MAX
#define NPY_MIN_INT INT_MIN
#define NPY_MAX_UINT UINT_MAX
#define NPY_MAX_LONG LONG_MAX
#define NPY_MIN_LONG LONG_MIN
#define NPY_MAX_ULONG ULONG_MAX
#define NPY_MAX_LONGLONG NPY_MAX_INT64
#define NPY_MIN_LONGLONG NPY_MIN_INT64
#define NPY_MAX_ULONGLONG NPY_MAX_UINT64
#define NPY_MIN_DATETIME NPY_MIN_INT64
#define NPY_MAX_DATETIME NPY_MAX_INT64
#define NPY_MIN_TIMEDELTA NPY_MIN_INT64
#define NPY_MAX_TIMEDELTA NPY_MAX_INT64
#define NPY_SIZEOF_DATETIME 8
#define NPY_SIZEOF_TIMEDELTA 8
#define NPY_SIZEOF_HALF 2

#ifndef NPY_BITSOF_FLOAT
#define NPY_BITSOF_FLOAT (NPY_SIZEOF_FLOAT * CHAR_BIT)
#endif
#ifndef NPY_BITSOF_DOUBLE
#define NPY_BITSOF_DOUBLE (NPY_SIZEOF_DOUBLE * CHAR_BIT)
#endif
#ifndef NPY_BITSOF_LONGDOUBLE
#define NPY_BITSOF_LONGDOUBLE (NPY_SIZEOF_LONGDOUBLE * CHAR_BIT)
#endif

#if !defined(HAVE_LDOUBLE_IEEE_QUAD_BE) && !defined(HAVE_LDOUBLE_IEEE_QUAD_LE) && \
    !defined(HAVE_LDOUBLE_IEEE_DOUBLE_LE) && !defined(HAVE_LDOUBLE_IEEE_DOUBLE_BE) && \
    !defined(HAVE_LDOUBLE_INTEL_EXTENDED_12_BYTES_LE) && \
    !defined(HAVE_LDOUBLE_INTEL_EXTENDED_16_BYTES_LE) && \
    !defined(HAVE_LDOUBLE_MOTOROLA_EXTENDED_12_BYTES_BE) && \
    !defined(HAVE_LDOUBLE_IBM_DOUBLE_DOUBLE_LE) && \
    !defined(HAVE_LDOUBLE_IBM_DOUBLE_DOUBLE_BE)
#if NPY_SIZEOF_LONGDOUBLE == NPY_SIZEOF_DOUBLE
#if NPY_BYTE_ORDER == NPY_LITTLE_ENDIAN
#define HAVE_LDOUBLE_IEEE_DOUBLE_LE 1
#else
#define HAVE_LDOUBLE_IEEE_DOUBLE_BE 1
#endif
#endif
#endif

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

#define NPY_SSIZE_T_PYFMT "n"
#define constchar char

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

#if NPY_SIZEOF_INTP == NPY_SIZEOF_LONG
#define NPY_MAX_INTP NPY_MAX_LONG
#define NPY_MIN_INTP NPY_MIN_LONG
#define NPY_MAX_UINTP NPY_MAX_ULONG
#elif NPY_SIZEOF_INTP == NPY_SIZEOF_INT
#define NPY_MAX_INTP NPY_MAX_INT
#define NPY_MIN_INTP NPY_MIN_INT
#define NPY_MAX_UINTP NPY_MAX_UINT
#else
#define NPY_MAX_INTP NPY_MAX_LONGLONG
#define NPY_MIN_INTP NPY_MIN_LONGLONG
#define NPY_MAX_UINTP NPY_MAX_ULONGLONG
#endif

#ifdef _WIN32
#define npy_fseek _fseeki64
#define npy_ftell _ftelli64
#define npy_lseek _lseeki64
typedef npy_int64 npy_off_t;
#if NPY_SIZEOF_INT == 8
#define NPY_OFF_T_PYFMT "i"
#elif NPY_SIZEOF_LONG == 8
#define NPY_OFF_T_PYFMT "l"
#else
#define NPY_OFF_T_PYFMT "L"
#endif
#else
#define npy_fseek fseeko
#define npy_ftell ftello
#define npy_lseek lseek
typedef off_t npy_off_t;
#if NPY_SIZEOF_OFF_T == NPY_SIZEOF_SHORT
#define NPY_OFF_T_PYFMT "h"
#elif NPY_SIZEOF_OFF_T == NPY_SIZEOF_INT
#define NPY_OFF_T_PYFMT "i"
#elif NPY_SIZEOF_OFF_T == NPY_SIZEOF_LONG
#define NPY_OFF_T_PYFMT "l"
#else
#define NPY_OFF_T_PYFMT "L"
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
