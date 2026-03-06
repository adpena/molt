#ifndef MOLT_NUMPY_CONFIG_H
#define MOLT_NUMPY_CONFIG_H

#ifndef NUMPY_CORE_SRC_COMMON_NPY_CONFIG_H_
#error config.h should never be included directly, include npy_config.h instead
#endif

#define HAVE_NPY_CONFIG_H 1

#ifndef SIZEOF_PY_INTPTR_T
#define SIZEOF_PY_INTPTR_T __SIZEOF_POINTER__
#endif
#ifndef SIZEOF_PY_LONG_LONG
#define SIZEOF_PY_LONG_LONG __SIZEOF_LONG_LONG__
#endif
#ifndef SIZEOF_OFF_T
#ifdef __SIZEOF_OFF_T__
#define SIZEOF_OFF_T __SIZEOF_OFF_T__
#else
#define SIZEOF_OFF_T __SIZEOF_LONG__
#endif
#endif

#if defined(__has_include)
#if __has_include(<features.h>)
#define HAVE_FEATURES_H 1
#endif
#if __has_include(<xlocale.h>)
#define HAVE_XLOCALE_H 1
#endif
#if __has_include(<dlfcn.h>)
#define HAVE_DLFCN_H 1
#endif
#if __has_include(<execinfo.h>)
#define HAVE_EXECINFO_H 1
#endif
#if __has_include(<sys/mman.h>)
#define HAVE_SYS_MMAN_H 1
#endif
#if __has_include(<xmmintrin.h>)
#define HAVE_XMMINTRIN_H 1
#endif
#if __has_include(<emmintrin.h>)
#define HAVE_EMMINTRIN_H 1
#endif
#if __has_include(<immintrin.h>)
#define HAVE_IMMINTRIN_H 1
#endif
#endif

#if !defined(_WIN32)
#define HAVE_FSEEKO 1
#define HAVE_FTELLO 1
#endif

#if defined(__has_builtin)
#if __has_builtin(__builtin_isnan)
#define HAVE___BUILTIN_ISNAN 1
#endif
#if __has_builtin(__builtin_isinf)
#define HAVE___BUILTIN_ISINF 1
#endif
#if __has_builtin(__builtin_isfinite)
#define HAVE___BUILTIN_ISFINITE 1
#endif
#if __has_builtin(__builtin_bswap32)
#define HAVE___BUILTIN_BSWAP32 1
#endif
#if __has_builtin(__builtin_bswap64)
#define HAVE___BUILTIN_BSWAP64 1
#endif
#if __has_builtin(__builtin_expect)
#define HAVE___BUILTIN_EXPECT 1
#endif
#if __has_builtin(__builtin_mul_overflow)
#define HAVE___BUILTIN_MUL_OVERFLOW 1
#endif
#if __has_builtin(__builtin_prefetch)
#define HAVE___BUILTIN_PREFETCH 1
#endif
#endif

#if defined(__has_attribute)
#if __has_attribute(nonnull)
#define HAVE_ATTRIBUTE_NONNULL 1
#endif
#endif

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define HAVE_THREAD_LOCAL 1
#endif

#if defined(__GNUC__) || defined(__clang__)
#define HAVE___THREAD 1
#endif

#endif
