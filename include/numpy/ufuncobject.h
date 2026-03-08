#ifndef MOLT_NUMPY_UFUNCOBJECT_H
#define MOLT_NUMPY_UFUNCOBJECT_H

#include <numpy/ndarrayobject.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef MOLT_NUMPY_UFUNC_CORE_TYPES
#define MOLT_NUMPY_UFUNC_CORE_TYPES
typedef void (*PyUFuncGenericFunction)(
    char **args,
    npy_intp const *dimensions,
    npy_intp const *strides,
    void *innerloopdata
);

struct _tagPyUFuncObject;

typedef int (PyUFunc_TypeResolutionFunc)(
    struct _tagPyUFuncObject *ufunc,
    NPY_CASTING casting,
    PyArrayObject **operands,
    PyObject *type_tup,
    PyArray_Descr **out_dtypes
);

typedef int (PyUFunc_ProcessCoreDimsFunc)(
    struct _tagPyUFuncObject *ufunc,
    npy_intp *core_dim_sizes
);
#endif

typedef void (PyUFunc_MaskedStridedInnerLoopFunc)(
    char **dataptrs,
    npy_intp *strides,
    char *maskptr,
    npy_intp mask_stride,
    npy_intp count,
    NpyAuxData *innerloopdata
);

typedef struct PyUFunc_LoopSlot {
    int slot;
    void *pfunc;
} PyUFunc_LoopSlot;

typedef struct PyUFunc_PyFuncData {
    int nin;
    int nout;
    PyObject *callable;
} PyUFunc_PyFuncData;

typedef struct _loop1d_info {
    PyUFuncGenericFunction func;
    void *data;
    int *arg_types;
    struct _loop1d_info *next;
    int nargs;
    PyArray_Descr **arg_dtypes;
} PyUFunc_Loop1d;

static void **PyUFunc_API = NULL;

#define PyUFunc_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyUFunc_Type)

#define PyUFunc_None (-1)
#define PyUFunc_Zero 0
#define PyUFunc_One 1
#define PyUFunc_MinusOne 2
#define PyUFunc_ReorderableNone (-2)
#define PyUFunc_IdentityValue (-3)

static PyUFuncGenericFunction PyUFunc_O_O = NULL;
static PyUFuncGenericFunction PyUFunc_OO_O = NULL;
static PyUFuncGenericFunction PyUFunc_O_O_method = NULL;
static PyUFuncGenericFunction PyUFunc_OO_O_method = NULL;
static PyUFuncGenericFunction PyUFunc_On_Om = NULL;
static PyUFuncGenericFunction PyUFunc_f_f = NULL;
static PyUFuncGenericFunction PyUFunc_ff_f = NULL;
static PyUFuncGenericFunction PyUFunc_f_f_As_d_d = NULL;
static PyUFuncGenericFunction PyUFunc_ff_f_As_dd_d = NULL;
static PyUFuncGenericFunction PyUFunc_d_d = NULL;
static PyUFuncGenericFunction PyUFunc_dd_d = NULL;
static PyUFuncGenericFunction PyUFunc_g_g = NULL;
static PyUFuncGenericFunction PyUFunc_gg_g = NULL;
static PyUFuncGenericFunction PyUFunc_F_F = NULL;
static PyUFuncGenericFunction PyUFunc_FF_F = NULL;
static PyUFuncGenericFunction PyUFunc_F_F_As_D_D = NULL;
static PyUFuncGenericFunction PyUFunc_FF_F_As_DD_D = NULL;
static PyUFuncGenericFunction PyUFunc_D_D = NULL;
static PyUFuncGenericFunction PyUFunc_DD_D = NULL;
static PyUFuncGenericFunction PyUFunc_G_G = NULL;
static PyUFuncGenericFunction PyUFunc_GG_G = NULL;
static PyUFuncGenericFunction PyUFunc_e_e = NULL;
static PyUFuncGenericFunction PyUFunc_ee_e = NULL;
static PyUFuncGenericFunction PyUFunc_e_e_As_f_f = NULL;
static PyUFuncGenericFunction PyUFunc_e_e_As_d_d = NULL;
static PyUFuncGenericFunction PyUFunc_ee_e_As_ff_f = NULL;
static PyUFuncGenericFunction PyUFunc_ee_e_As_dd_d = NULL;

static inline int PyUFunc_getfperr(void) {
    return 0;
}

static inline void PyUFunc_clearfperr(void) {}

static inline int PyUFunc_ImportUFuncAPI(void) {
    return 0;
}

static inline int _import_umath(void) {
    return PyUFunc_ImportUFuncAPI();
}

#define import_umath()                                                             \
    do {                                                                           \
        if (_import_umath() < 0) {                                                 \
            return NULL;                                                           \
        }                                                                          \
    } while (0);

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyUFunc_FromFuncAndDataAndSignatureAndIdentity(...) _molt_numpy_unavailable_obj("PyUFunc_FromFuncAndDataAndSignatureAndIdentity")
#define PyUFunc_RegisterLoopForType(...) _molt_numpy_unavailable_i32("PyUFunc_RegisterLoopForType")
#define PyUFunc_RegisterLoopForDescr(...) _molt_numpy_unavailable_i32("PyUFunc_RegisterLoopForDescr")
#define PyUFunc_ReplaceLoopBySignature(...) _molt_numpy_unavailable_i32("PyUFunc_ReplaceLoopBySignature")
#define PyUFunc_AddLoopFromSpec(...) _molt_numpy_unavailable_i32("PyUFunc_AddLoopFromSpec")
#define PyUFunc_AddPromoter(...) _molt_numpy_unavailable_i32("PyUFunc_AddPromoter")
#define PyUFunc_AddWrappingLoop(...) _molt_numpy_unavailable_i32("PyUFunc_AddWrappingLoop")
#define PyUFunc_DefaultTypeResolver(...) _molt_numpy_unavailable_i32("PyUFunc_DefaultTypeResolver")
#define PyUFunc_ValidateCasting(...) _molt_numpy_unavailable_i32("PyUFunc_ValidateCasting")
#endif

static inline int PyUFunc_GiveFloatingpointErrors(const char *name, int fpe_errors) {
    (void)name;
    (void)fpe_errors;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUFunc_GiveFloatingpointErrors is not yet implemented in Molt's NumPy compatibility layer");
    return -1;
}

static inline int PyUFunc_AddLoop(PyObject *ufunc, PyObject *info, int ignore) {
    (void)ufunc;
    (void)info;
    (void)ignore;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUFunc_AddLoop is not yet implemented in Molt's NumPy compatibility layer");
    return -1;
}

static inline int PyUFunc_AddLoopFromSpec_int(
    PyObject *ufunc,
    PyArrayMethod_Spec *spec,
    int private_api
) {
    (void)ufunc;
    (void)spec;
    (void)private_api;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUFunc_AddLoopFromSpec_int is not yet implemented in Molt's NumPy compatibility layer");
    return -1;
}

static inline int PyUFunc_AddLoopsFromSpecs(PyUFunc_LoopSlot *slots) {
    (void)slots;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUFunc_AddLoopsFromSpecs is not yet implemented in Molt's NumPy compatibility layer");
    return -1;
}

static inline PyObject *PyUFunc_FromFuncAndData(
    PyUFuncGenericFunction *funcs,
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
    (void)funcs;
    (void)data;
    (void)types;
    (void)ntypes;
    (void)nin;
    (void)nout;
    (void)identity;
    (void)name;
    (void)doc;
    (void)unused;
    return _molt_numpy_unavailable_obj("PyUFunc_FromFuncAndData");
}

static inline PyObject *PyUFunc_FromFuncAndDataAndSignature(
    PyUFuncGenericFunction *funcs,
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
    (void)funcs;
    (void)data;
    (void)types;
    (void)ntypes;
    (void)nin;
    (void)nout;
    (void)identity;
    (void)name;
    (void)doc;
    (void)unused;
    (void)signature;
    return _molt_numpy_unavailable_obj("PyUFunc_FromFuncAndDataAndSignature");
}

#ifdef __cplusplus
}
#endif

#endif
