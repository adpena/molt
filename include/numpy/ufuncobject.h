#ifndef MOLT_NUMPY_UFUNCOBJECT_H
#define MOLT_NUMPY_UFUNCOBJECT_H

#include <numpy/ndarrayobject.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*PyUFuncGenericFunction)(
    char **args,
    const npy_intp *dimensions,
    const npy_intp *strides,
    void *innerloopdata
);

typedef struct PyUFunc_LoopSlot {
    int _molt_reserved;
} PyUFunc_LoopSlot;

#define PyUFunc_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyUFunc_None -1

#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)

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
