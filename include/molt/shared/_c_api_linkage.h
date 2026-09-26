/* One external C-API data linkage policy for both Python.h transports.
 * A Windows extension importing data from a shared runtime/ABI image opts in
 * explicitly; static, WASM, and non-Windows linkage retain plain externs.
 * Embedders that already define PyAPI_DATA keep their chosen policy. */
#ifndef MOLT_C_API_LINKAGE_H
#define MOLT_C_API_LINKAGE_H

#ifndef PyAPI_DATA
#if defined(_WIN32) && defined(MOLT_CPYTHON_ABI_SHARED) && MOLT_CPYTHON_ABI_SHARED
#define PyAPI_DATA(RTYPE) extern __declspec(dllimport) RTYPE
#else
#define PyAPI_DATA(RTYPE) extern RTYPE
#endif
#endif

#endif /* MOLT_C_API_LINKAGE_H */
