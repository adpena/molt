#ifndef MOLT_NUMPY_NPY_2_COMPAT_H
#define MOLT_NUMPY_NPY_2_COMPAT_H

#include <numpy/arrayobject.h>

#ifndef NPY_RAVEL_AXIS
#define NPY_RAVEL_AXIS NPY_MIN_INT
#endif

#ifndef NPY_DEFAULT_ASSIGN_CASTING
#define NPY_DEFAULT_ASSIGN_CASTING NPY_SAME_KIND_CASTING
#endif

#endif
