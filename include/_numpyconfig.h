#ifndef MOLT_NUMPY__NUMPYCONFIG_H
#define MOLT_NUMPY__NUMPYCONFIG_H

#ifndef NPY_HAVE_ENDIAN_H
#if defined(__has_include)
#if __has_include(<endian.h>)
#define NPY_HAVE_ENDIAN_H 1
#endif
#endif
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
#ifndef NPY_SIZEOF_COMPLEX_LONGDOUBLE
#define NPY_SIZEOF_COMPLEX_LONGDOUBLE (2 * __SIZEOF_LONG_DOUBLE__)
#endif
#ifndef NPY_SIZEOF_PY_INTPTR_T
#ifdef SIZEOF_PY_INTPTR_T
#define NPY_SIZEOF_PY_INTPTR_T SIZEOF_PY_INTPTR_T
#else
#define NPY_SIZEOF_PY_INTPTR_T __SIZEOF_POINTER__
#endif
#endif
#ifndef NPY_SIZEOF_INTP
#define NPY_SIZEOF_INTP __SIZEOF_POINTER__
#endif
#ifndef NPY_SIZEOF_UINTP
#define NPY_SIZEOF_UINTP __SIZEOF_POINTER__
#endif
#ifndef NPY_SIZEOF_WCHAR_T
#define NPY_SIZEOF_WCHAR_T __SIZEOF_WCHAR_T__
#endif
#ifndef NPY_SIZEOF_OFF_T
#ifdef SIZEOF_OFF_T
#define NPY_SIZEOF_OFF_T SIZEOF_OFF_T
#elif defined(__SIZEOF_OFF_T__)
#define NPY_SIZEOF_OFF_T __SIZEOF_OFF_T__
#else
#define NPY_SIZEOF_OFF_T __SIZEOF_LONG__
#endif
#endif
#ifndef NPY_SIZEOF_PY_LONG_LONG
#ifdef SIZEOF_PY_LONG_LONG
#define NPY_SIZEOF_PY_LONG_LONG SIZEOF_PY_LONG_LONG
#else
#define NPY_SIZEOF_PY_LONG_LONG __SIZEOF_LONG_LONG__
#endif
#endif
#ifndef NPY_SIZEOF_LONGLONG
#define NPY_SIZEOF_LONGLONG __SIZEOF_LONG_LONG__
#endif

#ifndef NPY_NO_SMP
#define NPY_NO_SMP 0
#endif

#ifndef NPY_VISIBILITY_HIDDEN
#if defined(__GNUC__) || defined(__clang__)
#define NPY_VISIBILITY_HIDDEN __attribute__((visibility("hidden")))
#else
#define NPY_VISIBILITY_HIDDEN
#endif
#endif

#ifndef NPY_ABI_VERSION
#define NPY_ABI_VERSION 0x02000000
#endif
#ifndef NPY_API_VERSION
#define NPY_API_VERSION 0x00000015
#endif

#ifndef __STDC_FORMAT_MACROS
#define __STDC_FORMAT_MACROS 1
#endif

#endif
