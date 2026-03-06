#ifndef MOLT_NUMPY_NPY_NO_DEPRECATED_API_H
#define MOLT_NUMPY_NPY_NO_DEPRECATED_API_H

/* Source-compat overlay derived from NumPy 2.4.2 public npy_no_deprecated_api.h. */

#ifndef NPY_NO_DEPRECATED_API
#if defined(MOLT_NUMPY_NDARRAYTYPES_H) || defined(MOLT_NUMPY_NPY_COMMON_H)
#error "npy_no_deprecated_api.h must be first among numpy includes."
#else
#define NPY_NO_DEPRECATED_API NPY_API_VERSION
#endif
#endif

#endif
