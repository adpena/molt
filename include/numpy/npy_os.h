#ifndef MOLT_NUMPY_NPY_OS_H
#define MOLT_NUMPY_NPY_OS_H

/*
 * Source-compat overlay derived from NumPy 2.4.2 public npy_os.h.
 * This is a bounded public-platform macro lane, not a private build graph.
 */

#if defined(linux) || defined(__linux) || defined(__linux__)
#define NPY_OS_LINUX
#elif defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) || \
    defined(__DragonFly__)
#define NPY_OS_BSD
#ifdef __FreeBSD__
#define NPY_OS_FREEBSD
#elif defined(__NetBSD__)
#define NPY_OS_NETBSD
#elif defined(__OpenBSD__)
#define NPY_OS_OPENBSD
#elif defined(__DragonFly__)
#define NPY_OS_DRAGONFLY
#endif
#elif defined(sun) || defined(__sun)
#define NPY_OS_SOLARIS
#elif defined(__CYGWIN__)
#define NPY_OS_CYGWIN
#elif defined(_WIN32)
#if defined(__MINGW32__) || defined(__MINGW64__)
#define NPY_OS_MINGW
#elif defined(_WIN64)
#define NPY_OS_WIN64
#else
#define NPY_OS_WIN32
#endif
#elif defined(__APPLE__)
#define NPY_OS_DARWIN
#elif defined(__HAIKU__)
#define NPY_OS_HAIKU
#else
#define NPY_OS_UNKNOWN
#endif

#endif
