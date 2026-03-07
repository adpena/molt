#ifndef MOLT_NUMPY_TEMPL_COMMON_H
#define MOLT_NUMPY_TEMPL_COMMON_H

/*
 * Compile-focused compatibility overlay derived from NumPy's
 * numpy/_core/src/common/templ_common.h.src for source builds that expect the
 * generated header to exist next to the C sources.
 */

#include "numpy/npy_common.h"

#include <assert.h>
#include <stdlib.h>

static inline int npy_mul_sizes_with_overflow(npy_intp *r, npy_intp a, npy_intp b) {
#ifdef HAVE___BUILTIN_MUL_OVERFLOW
    return __builtin_mul_overflow(a, b, r);
#else
    const npy_intp half_sz = ((npy_intp)1 << ((sizeof(a) * 8 - 1) / 2));

    assert(a >= 0 && b >= 0);
    *r = a * b;
    if (NPY_UNLIKELY((a | b) >= half_sz) && a != 0 && b > NPY_MAX_INTP / a) {
        return 1;
    }
    return 0;
#endif
}

static inline int npy_mul_with_overflow_size_t(size_t *r, size_t a, size_t b) {
#ifdef HAVE___BUILTIN_MUL_OVERFLOW
    return __builtin_mul_overflow(a, b, r);
#else
    const size_t half_sz = ((size_t)1 << ((sizeof(a) * 8 - 1) / 2));

    *r = a * b;
    if ((NPY_UNLIKELY((a | b) >= half_sz) || (a | b) < 0) && a != 0 && b > ((size_t)-1) / a) {
        return 1;
    }
    return 0;
#endif
}

static inline int npy_mul_with_overflow_int(npy_int *r, npy_int a, npy_int b) {
#ifdef HAVE___BUILTIN_MUL_OVERFLOW
    return __builtin_mul_overflow(a, b, r);
#else
    const npy_int half_sz = ((npy_int)1 << ((sizeof(a) * 8 - 1) / 2));

    *r = a * b;
    if ((NPY_UNLIKELY((a | b) >= half_sz) || (a | b) < 0)
        && a != 0
        && abs(b) > abs(NPY_MAX_INT / a)) {
        return 1;
    }
    return 0;
#endif
}

#endif
