/* Target C data-model authority for both Molt Python.h surfaces.
 * Use the target standard headers, never the build host or OS name. All public
 * values are preprocessing integer constants, including on MSVC. */
#ifndef MOLT_C_DATA_MODEL_H
#define MOLT_C_DATA_MODEL_H

#include <limits.h>
#include <stdint.h>
#include <stddef.h>

#if CHAR_BIT != 8 || UINT_MAX != 0xFFFFFFFFU || ULLONG_MAX != 0xFFFFFFFFFFFFFFFFULL
#error "Molt C ABI requires 8-bit bytes, 32-bit int and 64-bit long long"
#endif

#if UINTPTR_MAX == 0xFFFFFFFFU
#define MOLT_TARGET_SIZEOF_VOID_P 4
#elif UINTPTR_MAX == 0xFFFFFFFFFFFFFFFFULL
#define MOLT_TARGET_SIZEOF_VOID_P 8
#else
#error "Molt C ABI requires 32-bit or 64-bit pointers"
#endif

#if ULONG_MAX == 0xFFFFFFFFUL
#define MOLT_TARGET_SIZEOF_LONG 4
#elif ULONG_MAX == 0xFFFFFFFFFFFFFFFFULL
#define MOLT_TARGET_SIZEOF_LONG 8
#else
#error "Molt C ABI requires 32-bit or 64-bit long"
#endif

#if SIZE_MAX != UINTPTR_MAX
#error "Molt C ABI requires pointer-sized size_t"
#endif
#if MOLT_TARGET_SIZEOF_LONG > MOLT_TARGET_SIZEOF_VOID_P
#error "Molt C ABI requires ILP32, LP64 or LLP64"
#endif

#ifdef SIZEOF_VOID_P
#if SIZEOF_VOID_P != MOLT_TARGET_SIZEOF_VOID_P
#error "SIZEOF_VOID_P conflicts with the target C data model"
#endif
#else
#define SIZEOF_VOID_P MOLT_TARGET_SIZEOF_VOID_P
#endif

#ifdef SIZEOF_INT
#if SIZEOF_INT != 4
#error "SIZEOF_INT conflicts with the target C data model"
#endif
#else
#define SIZEOF_INT 4
#endif

#ifdef SIZEOF_LONG
#if SIZEOF_LONG != MOLT_TARGET_SIZEOF_LONG
#error "SIZEOF_LONG conflicts with the target C data model"
#endif
#else
#define SIZEOF_LONG MOLT_TARGET_SIZEOF_LONG
#endif

#ifdef SIZEOF_LONG_LONG
#if SIZEOF_LONG_LONG != 8
#error "SIZEOF_LONG_LONG conflicts with the target C data model"
#endif
#else
#define SIZEOF_LONG_LONG 8
#endif

#ifdef SIZEOF_SIZE_T
#if SIZEOF_SIZE_T != MOLT_TARGET_SIZEOF_VOID_P
#error "SIZEOF_SIZE_T conflicts with the target C data model"
#endif
#else
#define SIZEOF_SIZE_T MOLT_TARGET_SIZEOF_VOID_P
#endif

#ifdef LONG_BIT
#if LONG_BIT != (SIZEOF_LONG * CHAR_BIT)
#error "LONG_BIT conflicts with the target C data model"
#endif
#else
#define LONG_BIT (SIZEOF_LONG * CHAR_BIT)
#endif

#endif /* MOLT_C_DATA_MODEL_H */
