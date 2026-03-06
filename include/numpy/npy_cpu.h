#ifndef MOLT_NUMPY_NPY_CPU_H
#define MOLT_NUMPY_NPY_CPU_H

#include <numpy/numpyconfig.h>

#if defined(__i386__) || defined(i386) || defined(_M_IX86)
#define NPY_CPU_X86
#elif defined(__x86_64__) || defined(__amd64__) || defined(__x86_64) || defined(_M_AMD64)
#define NPY_CPU_AMD64
#elif defined(__powerpc64__) && defined(__LITTLE_ENDIAN__)
#define NPY_CPU_PPC64LE
#elif defined(__powerpc64__) && defined(__BIG_ENDIAN__)
#define NPY_CPU_PPC64
#elif defined(__ppc__) || defined(__powerpc__) || defined(_ARCH_PPC)
#define NPY_CPU_PPC
#elif defined(__sparc__) || defined(__sparc)
#define NPY_CPU_SPARC
#elif defined(__s390__)
#define NPY_CPU_S390
#elif defined(__ia64)
#define NPY_CPU_IA64
#elif defined(__hppa)
#define NPY_CPU_HPPA
#elif defined(__alpha__)
#define NPY_CPU_ALPHA
#elif defined(__arm__) || defined(__aarch64__) || defined(_M_ARM64)
#if defined(__ARMEB__) || defined(__AARCH64EB__)
#if defined(__ARM_32BIT_STATE)
#define NPY_CPU_ARMEB_AARCH32
#elif defined(__ARM_64BIT_STATE)
#define NPY_CPU_ARMEB_AARCH64
#else
#define NPY_CPU_ARMEB
#endif
#elif defined(__ARMEL__) || defined(__AARCH64EL__) || defined(_M_ARM64)
#if defined(__ARM_32BIT_STATE)
#define NPY_CPU_ARMEL_AARCH32
#elif defined(__ARM_64BIT_STATE) || defined(_M_ARM64) || defined(__AARCH64EL__)
#define NPY_CPU_ARMEL_AARCH64
#else
#define NPY_CPU_ARMEL
#endif
#else
#error Unknown ARM CPU
#endif
#elif defined(__sh__) && defined(__LITTLE_ENDIAN__)
#define NPY_CPU_SH_LE
#elif defined(__sh__) && defined(__BIG_ENDIAN__)
#define NPY_CPU_SH_BE
#elif defined(__arc__) && defined(__LITTLE_ENDIAN__)
#define NPY_CPU_ARCEL
#elif defined(__arc__) && defined(__BIG_ENDIAN__)
#define NPY_CPU_ARCEB
#elif defined(__riscv)
#if __riscv_xlen == 64
#define NPY_CPU_RISCV64
#elif __riscv_xlen == 32
#define NPY_CPU_RISCV32
#endif
#elif defined(__loongarch_lp64)
#define NPY_CPU_LOONGARCH64
#elif defined(__sw_64__)
#define NPY_CPU_SW_64
#elif defined(__EMSCRIPTEN__) || defined(__wasm__)
#define NPY_CPU_WASM
#else
#error Unknown CPU, please extend Molt's NumPy compatibility header
#endif

#endif
