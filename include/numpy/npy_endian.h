#ifndef MOLT_NUMPY_NPY_ENDIAN_H
#define MOLT_NUMPY_NPY_ENDIAN_H

/* Source-compat overlay derived from NumPy 2.4.2 public npy_endian.h. */

#if defined(NPY_HAVE_ENDIAN_H) || defined(NPY_HAVE_SYS_ENDIAN_H)
#if defined(NPY_HAVE_ENDIAN_H)
#include <endian.h>
#elif defined(NPY_HAVE_SYS_ENDIAN_H)
#include <sys/endian.h>
#endif

#if defined(BYTE_ORDER) && defined(BIG_ENDIAN) && defined(LITTLE_ENDIAN)
#define NPY_BYTE_ORDER BYTE_ORDER
#define NPY_LITTLE_ENDIAN LITTLE_ENDIAN
#define NPY_BIG_ENDIAN BIG_ENDIAN
#elif defined(_BYTE_ORDER) && defined(_BIG_ENDIAN) && defined(_LITTLE_ENDIAN)
#define NPY_BYTE_ORDER _BYTE_ORDER
#define NPY_LITTLE_ENDIAN _LITTLE_ENDIAN
#define NPY_BIG_ENDIAN _BIG_ENDIAN
#elif defined(__BYTE_ORDER) && defined(__BIG_ENDIAN) && defined(__LITTLE_ENDIAN)
#define NPY_BYTE_ORDER __BYTE_ORDER
#define NPY_LITTLE_ENDIAN __LITTLE_ENDIAN
#define NPY_BIG_ENDIAN __BIG_ENDIAN
#endif
#endif

#ifndef NPY_BYTE_ORDER
#include <numpy/npy_cpu.h>

#define NPY_LITTLE_ENDIAN 1234
#define NPY_BIG_ENDIAN 4321

#if defined(NPY_CPU_X86) \
    || defined(NPY_CPU_AMD64) \
    || defined(NPY_CPU_IA64) \
    || defined(NPY_CPU_ALPHA) \
    || defined(NPY_CPU_ARMEL) \
    || defined(NPY_CPU_ARMEL_AARCH32) \
    || defined(NPY_CPU_ARMEL_AARCH64) \
    || defined(NPY_CPU_SH_LE) \
    || defined(NPY_CPU_PPC64LE) \
    || defined(NPY_CPU_ARCEL) \
    || defined(NPY_CPU_RISCV64) \
    || defined(NPY_CPU_RISCV32) \
    || defined(NPY_CPU_LOONGARCH64) \
    || defined(NPY_CPU_SW_64) \
    || defined(NPY_CPU_WASM)
#define NPY_BYTE_ORDER NPY_LITTLE_ENDIAN
#else
#define NPY_BYTE_ORDER NPY_BIG_ENDIAN
#endif
#endif

#endif
