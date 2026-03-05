#ifndef MOLT_NUMPY_DTYPE_API_H
#define MOLT_NUMPY_DTYPE_API_H

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    NPY_METH_REQUIRES_PYAPI = 1 << 0,
    NPY_METH_NO_FLOATINGPOINT_ERRORS = 1 << 1,
    NPY_METH_SUPPORTS_UNALIGNED = 1 << 2,
    NPY_METH_IS_REORDERABLE = 1 << 3,
    NPY_METH_RUNTIME_FLAGS = (
        NPY_METH_REQUIRES_PYAPI | NPY_METH_NO_FLOATINGPOINT_ERRORS),
} NPY_ARRAYMETHOD_FLAGS;

#define PyArrayMethod_COMBINED_FLAGS(lhs, rhs) \
    ((NPY_ARRAYMETHOD_FLAGS)((lhs) | (rhs)))
#define PyArrayMethod_MINIMAL_FLAGS 0

typedef NPY_CASTING (PyArrayMethod_ResolveDescriptors)(
    PyArrayMethodObject *method,
    PyArray_DTypeMeta *const *dtypes,
    PyArray_Descr *const *given_descrs,
    PyArray_Descr **loop_descrs,
    npy_intp *view_offset
);

typedef int (PyArrayMethod_GetMaskedStridedLoop)(
    PyArrayMethod_Context *context,
    int aligned,
    int move_references,
    const npy_intp *strides,
    PyArrayMethod_StridedLoop **out_loop,
    NpyAuxData **out_auxdata,
    NPY_ARRAYMETHOD_FLAGS *flags
);

typedef int (PyArrayMethod_TraverseLoop)(
    void *traverse_context,
    const PyArray_Descr *descr,
    char *data,
    npy_intp size,
    npy_intp stride,
    NpyAuxData *auxdata
);

typedef int (PyArrayMethod_GetTraverseLoop)(
    void *traverse_context,
    const PyArray_Descr *descr,
    int aligned,
    npy_intp fixed_stride,
    PyArrayMethod_TraverseLoop **out_loop,
    NpyAuxData **out_auxdata,
    NPY_ARRAYMETHOD_FLAGS *flags
);

typedef int (PyArrayMethod_PromoterFunction)(
    PyObject *ufunc,
    PyArray_DTypeMeta *const op_dtypes[],
    PyArray_DTypeMeta *const signature[],
    PyArray_DTypeMeta *new_op_dtypes[]
);

typedef struct {
    int flags;
} PyArrayMethod_SortParameters;

#ifdef __cplusplus
}
#endif

#endif
