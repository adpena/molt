#ifndef MOLT_NUMPY_UFUNCOBJECT_H
#define MOLT_NUMPY_UFUNCOBJECT_H

#include <numpy/ndarrayobject.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PyUFunc_Type PyArray_Type

typedef void (*PyUFuncGenericFunction)(
    char **args,
    npy_intp const *dimensions,
    npy_intp const *strides,
    void *innerloopdata
);

typedef void (PyUFunc_MaskedStridedInnerLoopFunc)(
    char **dataptrs,
    npy_intp *strides,
    char *maskptr,
    npy_intp mask_stride,
    npy_intp count,
    NpyAuxData *innerloopdata
);

typedef int (PyUFunc_TypeResolutionFunc)(
    PyUFuncObject *ufunc,
    NPY_CASTING casting,
    PyArrayObject **operands,
    PyObject *type_tup,
    PyArray_Descr **out_dtypes
);

typedef int (PyUFunc_ProcessCoreDimsFunc)(
    PyUFuncObject *ufunc,
    npy_intp *core_dim_sizes
);

typedef struct PyUFunc_PyFuncData {
    int nin;
    int nout;
    PyObject *callable;
} PyUFunc_PyFuncData;

typedef struct PyUFunc_Loop1d {
    PyUFuncGenericFunction func;
    void *data;
    int *arg_types;
    struct PyUFunc_Loop1d *next;
    int nargs;
    PyArray_Descr **arg_dtypes;
} PyUFunc_Loop1d;

typedef struct PyUFunc_LoopSlot {
    const char *name;
    PyArrayMethod_Spec *spec;
} PyUFunc_LoopSlot;

#define PyUFunc_Zero 0
#define PyUFunc_One 1
#define PyUFunc_MinusOne 2
#define PyUFunc_None -1
#define PyUFunc_ReorderableNone -2
#define PyUFunc_IdentityValue -3

#define UFUNC_REDUCE 0
#define UFUNC_ACCUMULATE 1
#define UFUNC_REDUCEAT 2
#define UFUNC_OUTER 3
#define UFUNC_CORE_DIM_SIZE_INFERRED 0x0002
#define UFUNC_CORE_DIM_CAN_IGNORE 0x0004
#define UFUNC_CORE_DIM_MISSING 0x00040000
#define UFUNC_OBJ_ISOBJECT 1
#define UFUNC_OBJ_NEEDS_API 2

#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)

static void *PyUFunc_API[1] = {NULL};

static inline void _molt_numpy_ufunc_noop_loop(
    char **args,
    npy_intp const *dimensions,
    npy_intp const *strides,
    void *innerloopdata
) {
    (void)args;
    (void)dimensions;
    (void)strides;
    (void)innerloopdata;
}

#define PyUFunc_d_d _molt_numpy_ufunc_noop_loop
#define PyUFunc_D_D _molt_numpy_ufunc_noop_loop
#define PyUFunc_f_f _molt_numpy_ufunc_noop_loop
#define PyUFunc_g_g _molt_numpy_ufunc_noop_loop
#define PyUFunc_F_F _molt_numpy_ufunc_noop_loop
#define PyUFunc_G_G _molt_numpy_ufunc_noop_loop
#define PyUFunc_O_O _molt_numpy_ufunc_noop_loop
#define PyUFunc_dd_d _molt_numpy_ufunc_noop_loop
#define PyUFunc_DD_D _molt_numpy_ufunc_noop_loop
#define PyUFunc_ff_f _molt_numpy_ufunc_noop_loop
#define PyUFunc_gg_g _molt_numpy_ufunc_noop_loop
#define PyUFunc_FF_F _molt_numpy_ufunc_noop_loop
#define PyUFunc_GG_G _molt_numpy_ufunc_noop_loop
#define PyUFunc_OO_O _molt_numpy_ufunc_noop_loop
#define PyUFunc_f_f_As_d_d _molt_numpy_ufunc_noop_loop
#define PyUFunc_F_F_As_D_D _molt_numpy_ufunc_noop_loop
#define PyUFunc_ff_f_As_dd_d _molt_numpy_ufunc_noop_loop
#define PyUFunc_FF_F_As_DD_D _molt_numpy_ufunc_noop_loop
#define PyUFunc_O_O_method _molt_numpy_ufunc_noop_loop
#define PyUFunc_OO_O_method _molt_numpy_ufunc_noop_loop
#define PyUFunc_On_Om _molt_numpy_ufunc_noop_loop

static inline int PyUFunc_ImportUFuncAPI(void) {
    return 0;
}

static inline int PyUFunc_getfperr(void) {
    return 0;
}

static inline PyObject *PyUFunc_FromFuncAndDataAndSignatureAndIdentity(
    PyUFuncGenericFunction *func,
    void *const *data,
    const char *types,
    int ntypes,
    int nin,
    int nout,
    int identity,
    const char *name,
    const char *doc,
    const int unused,
    const char *signature,
    PyObject *identity_value
) {
    PyUFuncObject *ufunc;
    (void)unused;
    if (nin < 0 || nout < 0 || nin + nout > NPY_MAXARGS) {
        PyErr_SetString(PyExc_ValueError, "invalid ufunc operand count");
        return NULL;
    }
    ufunc = (PyUFuncObject *)PyMem_Calloc(1, sizeof(PyUFuncObject));
    if (ufunc == NULL) {
        return NULL;
    }
    ufunc->ob_base = (PyObject *)&PyUFunc_Type;
    ufunc->nin = nin;
    ufunc->nout = nout;
    ufunc->nargs = nin + nout;
    ufunc->identity = identity;
    ufunc->functions = (void *)func;
    ufunc->data = data;
    ufunc->types = types;
    ufunc->ntypes = ntypes;
    ufunc->name = name;
    ufunc->doc = doc;
    ufunc->core_signature = signature;
    ufunc->identity_value = identity_value;
    if (identity_value != NULL) {
        Py_INCREF(identity_value);
    }
    return (PyObject *)ufunc;
}

static inline PyObject *PyUFunc_FromFuncAndDataAndSignature(
    PyUFuncGenericFunction *func,
    void *const *data,
    const char *types,
    int ntypes,
    int nin,
    int nout,
    int identity,
    const char *name,
    const char *doc,
    int unused,
    const char *signature
) {
    return PyUFunc_FromFuncAndDataAndSignatureAndIdentity(
        func, data, types, ntypes, nin, nout, identity, name, doc, unused, signature, NULL);
}

static inline PyObject *PyUFunc_FromFuncAndData(
    PyUFuncGenericFunction *func,
    void *const *data,
    const char *types,
    int ntypes,
    int nin,
    int nout,
    int identity,
    const char *name,
    const char *doc,
    int unused
) {
    return PyUFunc_FromFuncAndDataAndSignature(
        func, data, types, ntypes, nin, nout, identity, name, doc, unused, NULL);
}

static inline int PyUFunc_RegisterLoopForType(
    PyUFuncObject *ufunc,
    int usertype,
    PyUFuncGenericFunction function,
    const int *arg_types,
    void *data
) {
    (void)ufunc;
    (void)usertype;
    (void)function;
    (void)arg_types;
    (void)data;
    return 0;
}

static inline int PyUFunc_ReplaceLoopBySignature(
    PyUFuncObject *ufunc,
    PyUFuncGenericFunction newfunc,
    const int *signature,
    PyUFuncGenericFunction *oldfunc
) {
    (void)ufunc;
    (void)newfunc;
    (void)signature;
    if (oldfunc != NULL) {
        *oldfunc = NULL;
    }
    return 0;
}

static inline int PyUFunc_AddLoopsFromSpecs(PyUFunc_LoopSlot *slots) {
    (void)slots;
    return 0;
}

#ifdef __cplusplus
}
#endif

#endif
