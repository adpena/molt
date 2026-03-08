#ifndef MOLT_NUMPY_COMPAT_NPY_SORT_H
#define MOLT_NUMPY_COMPAT_NPY_SORT_H

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
#endif

NPY_NO_EXPORT int npy_quicksort(void *start, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_heapsort(void *start, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_mergesort(void *start, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_timsort(void *start, npy_intp num, void *varr);

NPY_NO_EXPORT int npy_aquicksort(void *start, npy_intp *arg, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_aheapsort(void *start, npy_intp *arg, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_amergesort(void *start, npy_intp *arg, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_atimsort(void *start, npy_intp *arg, npy_intp num, void *varr);

NPY_NO_EXPORT int npy_quicksort_impl(void *start, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_aquicksort_impl(void *start, npy_intp *arg, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_mergesort_impl(void *start, npy_intp num, void *varr);
NPY_NO_EXPORT int npy_amergesort_impl(void *start, npy_intp *arg, npy_intp num, void *varr);

#ifdef __cplusplus
}
#endif

#endif
