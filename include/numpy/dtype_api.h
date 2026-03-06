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

typedef NPY_CASTING (PyArrayMethod_ResolveDescriptorsWithScalar)(
    PyArrayMethodObject *method,
    PyArray_DTypeMeta *const *dtypes,
    PyArray_Descr *const *given_descrs,
    PyObject *const *input_scalars,
    PyArray_Descr **loop_descrs,
    npy_intp *view_offset
);

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE)
typedef int (PyArrayMethod_GetMaskedStridedLoop)(
    PyArrayMethod_Context *context,
    int aligned,
    int move_references,
    const npy_intp *strides,
    PyArrayMethod_StridedLoop **out_loop,
    NpyAuxData **out_auxdata,
    NPY_ARRAYMETHOD_FLAGS *flags
);
#endif

typedef int (PyArrayMethod_GetLoop)(
    PyArrayMethod_Context *context,
    int aligned,
    int move_references,
    const npy_intp *strides,
    PyArrayMethod_StridedLoop **out_loop,
    NpyAuxData **out_auxdata,
    NPY_ARRAYMETHOD_FLAGS *flags
);

typedef int (PyArrayMethod_GetReductionInitial)(
    PyArrayMethod_Context *context,
    npy_bool reduction_is_empty,
    void *initial
);

typedef int (PyArrayMethod_TranslateGivenDescriptors)(
    int nin,
    int nout,
    PyArray_DTypeMeta *const wrapped_dtypes[],
    PyArray_Descr *const given_descrs[],
    PyArray_Descr *new_descrs[]
);

typedef int (PyArrayMethod_TranslateLoopDescriptors)(
    int nin,
    int nout,
    PyArray_DTypeMeta *const new_dtypes[],
    PyArray_Descr *const given_descrs[],
    PyArray_Descr *original_descrs[],
    PyArray_Descr *loop_descrs[]
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

typedef PyArray_Descr *(PyArrayDTypeMeta_DiscoverDescrFromPyobject)(
    PyArray_DTypeMeta *cls,
    PyObject *obj
);

typedef int (PyArrayDTypeMeta_IsKnownScalarType)(
    PyArray_DTypeMeta *cls,
    PyTypeObject *obj
);

typedef PyArray_Descr *(PyArrayDTypeMeta_DefaultDescriptor)(PyArray_DTypeMeta *cls);
typedef PyArray_DTypeMeta *(PyArrayDTypeMeta_CommonDType)(
    PyArray_DTypeMeta *dtype1,
    PyArray_DTypeMeta *dtype2
);
typedef PyArray_Descr *(PyArrayDTypeMeta_CommonInstance)(
    PyArray_Descr *dtype1,
    PyArray_Descr *dtype2
);
typedef PyArray_Descr *(PyArrayDTypeMeta_EnsureCanonical)(PyArray_Descr *dtype);
typedef PyArray_Descr *(PyArrayDTypeMeta_FinalizeDescriptor)(PyArray_Descr *dtype);
typedef int (PyArrayDTypeMeta_GetConstant)(PyArray_Descr *descr, int id, void *data);
typedef int (PyArrayDTypeMeta_SetItem)(PyArray_Descr *descr, PyObject *obj, char *data);
typedef PyObject *(PyArrayDTypeMeta_GetItem)(PyArray_Descr *descr, char *data);

#define NPY_DTYPE(descr) ((PyArray_DTypeMeta *)Py_TYPE(descr))

typedef int (PyArrayMethod_GetReductionInitial)(
    PyArrayMethod_Context *context,
    npy_bool reduction_is_empty,
    void *initial
);

typedef int (PyArrayMethod_TranslateGivenDescriptors)(
    int nin,
    int nout,
    PyArray_DTypeMeta *const wrapped_dtypes[],
    PyArray_Descr *const given_descrs[],
    PyArray_Descr *new_descrs[]
);

typedef int (PyArrayMethod_TranslateLoopDescriptors)(
    int nin,
    int nout,
    PyArray_DTypeMeta *const new_dtypes[],
    PyArray_Descr *const given_descrs[],
    PyArray_Descr *original_descrs[],
    PyArray_Descr *loop_descrs[]
);

typedef struct {
    int flags;
} PyArrayMethod_SortParameters;

static inline int _molt_numpy_dtype_unavailable_i32(const char *name) {
    PyErr_Format(
        PyExc_RuntimeError,
        "%s is not yet implemented in Molt's NumPy compatibility layer",
        name);
    return -1;
}

#define PyArrayMethod_GetLoop(...) \
    _molt_numpy_dtype_unavailable_i32("PyArrayMethod_GetLoop")
#define PyArrayMethod_GetReductionInitial(...) \
    _molt_numpy_dtype_unavailable_i32("PyArrayMethod_GetReductionInitial")
#define PyArrayMethod_ResolveDescriptors(...) \
    ((NPY_CASTING)_molt_numpy_dtype_unavailable_i32("PyArrayMethod_ResolveDescriptors"))
#define PyArrayMethod_ResolveDescriptorsWithScalar(...) \
    ((NPY_CASTING)_molt_numpy_dtype_unavailable_i32("PyArrayMethod_ResolveDescriptorsWithScalar"))
#define PyArrayMethod_TranslateGivenDescriptors(...) \
    _molt_numpy_dtype_unavailable_i32("PyArrayMethod_TranslateGivenDescriptors")
#define PyArrayMethod_TranslateLoopDescriptors(...) \
    _molt_numpy_dtype_unavailable_i32("PyArrayMethod_TranslateLoopDescriptors")

#ifdef __cplusplus
}
#endif

#endif
