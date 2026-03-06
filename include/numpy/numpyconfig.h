#ifndef MOLT_NUMPY_NUMPYCONFIG_H
#define MOLT_NUMPY_NUMPYCONFIG_H

#include <_numpyconfig.h>

#ifndef NPY_1_7_API_VERSION
#define NPY_1_7_API_VERSION 0x00000007
#define NPY_1_8_API_VERSION 0x00000008
#define NPY_1_9_API_VERSION 0x00000009
#define NPY_1_10_API_VERSION 0x0000000a
#define NPY_1_11_API_VERSION 0x0000000a
#define NPY_1_12_API_VERSION 0x0000000a
#define NPY_1_13_API_VERSION 0x0000000b
#define NPY_1_14_API_VERSION 0x0000000c
#define NPY_1_15_API_VERSION 0x0000000c
#define NPY_1_16_API_VERSION 0x0000000d
#define NPY_1_17_API_VERSION 0x0000000d
#define NPY_1_18_API_VERSION 0x0000000d
#define NPY_1_19_API_VERSION 0x0000000d
#define NPY_1_20_API_VERSION 0x0000000e
#define NPY_1_21_API_VERSION 0x0000000e
#define NPY_1_22_API_VERSION 0x0000000f
#define NPY_1_23_API_VERSION 0x00000010
#define NPY_1_24_API_VERSION 0x00000010
#define NPY_1_25_API_VERSION 0x00000011
#define NPY_2_0_API_VERSION 0x00000012
#define NPY_2_1_API_VERSION 0x00000013
#define NPY_2_2_API_VERSION 0x00000013
#define NPY_2_3_API_VERSION 0x00000014
#define NPY_2_4_API_VERSION 0x00000015
#endif

#ifndef NPY_ABI_VERSION
#define NPY_ABI_VERSION 0x02000000
#endif

#ifndef NPY_VERSION
#define NPY_VERSION NPY_ABI_VERSION
#endif

#ifndef NPY_API_VERSION
#define NPY_API_VERSION NPY_2_4_API_VERSION
#endif

#ifndef NPY_FEATURE_VERSION
#define NPY_FEATURE_VERSION NPY_API_VERSION
#endif

#ifndef NPY_SIZEOF_SHORT
#define NPY_SIZEOF_SHORT __SIZEOF_SHORT__
#endif
#ifndef NPY_SIZEOF_INT
#define NPY_SIZEOF_INT __SIZEOF_INT__
#endif
#ifndef NPY_SIZEOF_LONG
#define NPY_SIZEOF_LONG __SIZEOF_LONG__
#endif
#ifndef NPY_SIZEOF_LONGLONG
#define NPY_SIZEOF_LONGLONG __SIZEOF_LONG_LONG__
#endif
#ifndef NPY_SIZEOF_WCHAR_T
#define NPY_SIZEOF_WCHAR_T __SIZEOF_WCHAR_T__
#endif
#ifndef NPY_SIZEOF_FLOAT
#define NPY_SIZEOF_FLOAT __SIZEOF_FLOAT__
#endif
#ifndef NPY_SIZEOF_COMPLEX_FLOAT
#define NPY_SIZEOF_COMPLEX_FLOAT (2 * __SIZEOF_FLOAT__)
#endif
#ifndef NPY_SIZEOF_DOUBLE
#define NPY_SIZEOF_DOUBLE __SIZEOF_DOUBLE__
#endif
#ifndef NPY_SIZEOF_COMPLEX_DOUBLE
#define NPY_SIZEOF_COMPLEX_DOUBLE (2 * __SIZEOF_DOUBLE__)
#endif
#ifndef NPY_SIZEOF_LONGDOUBLE
#define NPY_SIZEOF_LONGDOUBLE __SIZEOF_LONG_DOUBLE__
#endif
#ifndef NPY_SIZEOF_INTP
#define NPY_SIZEOF_INTP __SIZEOF_POINTER__
#endif
#ifndef NPY_SIZEOF_PY_INTPTR_T
#define NPY_SIZEOF_PY_INTPTR_T __SIZEOF_POINTER__
#endif
#ifndef NPY_SIZEOF_UINTP
#define NPY_SIZEOF_UINTP __SIZEOF_POINTER__
#endif
#ifndef NPY_SIZEOF_OFF_T
#define NPY_SIZEOF_OFF_T __SIZEOF_LONG__
#endif
#ifndef NPY_SIZEOF_PY_LONG_LONG
#define NPY_SIZEOF_PY_LONG_LONG __SIZEOF_LONG_LONG__
#endif
#ifndef NPY_SIZEOF_COMPLEX_LONGDOUBLE
#define NPY_SIZEOF_COMPLEX_LONGDOUBLE (2 * __SIZEOF_LONG_DOUBLE__)
#endif

#ifndef NPY_VISIBILITY_HIDDEN
#if defined(__GNUC__) || defined(__clang__)
#define NPY_VISIBILITY_HIDDEN __attribute__((visibility("hidden")))
#else
#define NPY_VISIBILITY_HIDDEN
#endif
#endif

#ifndef NPY_NO_EXPORT
#define NPY_NO_EXPORT NPY_VISIBILITY_HIDDEN
#endif

#ifndef NPY_INLINE_MATH
#define NPY_INLINE_MATH 0
#endif

#ifndef NPY_INLINE
#if defined(_MSC_VER) && !defined(__clang__)
#define NPY_INLINE __inline
#elif defined(__GNUC__) || defined(__clang__)
#if defined(__STRICT_ANSI__)
#define NPY_INLINE __inline__
#else
#define NPY_INLINE inline
#endif
#else
#define NPY_INLINE
#endif
#endif

#ifndef NPY_FINLINE
#if defined(_MSC_VER)
#ifdef __cplusplus
#define NPY_FINLINE __forceinline
#else
#define NPY_FINLINE static __forceinline
#endif
#elif defined(__GNUC__) || defined(__clang__)
#ifdef __cplusplus
#define NPY_FINLINE inline __attribute__((always_inline))
#else
#define NPY_FINLINE static inline __attribute__((always_inline))
#endif
#else
#ifdef __cplusplus
#define NPY_FINLINE inline
#else
#define NPY_FINLINE static NPY_INLINE
#endif
#endif
#endif

#ifndef NPY_NOINLINE
#if defined(_MSC_VER)
#define NPY_NOINLINE static __declspec(noinline)
#elif defined(__GNUC__) || defined(__clang__)
#define NPY_NOINLINE static __attribute__((noinline))
#else
#define NPY_NOINLINE static
#endif
#endif

#ifndef NPY_TLS
#ifdef __cplusplus
#define NPY_TLS thread_local
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define NPY_TLS _Thread_local
#elif defined(__GNUC__) || defined(__clang__)
#define NPY_TLS __thread
#elif defined(_MSC_VER)
#define NPY_TLS __declspec(thread)
#else
#define NPY_TLS
#endif
#endif

#ifndef NPY_LIKELY
#if defined(__has_builtin)
#if __has_builtin(__builtin_expect)
#define NPY_LIKELY(x) __builtin_expect(!!(x), 1)
#define NPY_UNLIKELY(x) __builtin_expect(!!(x), 0)
#endif
#endif
#endif

#ifndef NPY_LIKELY
#define NPY_LIKELY(x) (x)
#define NPY_UNLIKELY(x) (x)
#endif

#endif
