#ifndef MOLT_NUMPY_NDARRAYTYPES_H
#define MOLT_NUMPY_NDARRAYTYPES_H

#include <Python.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef signed char npy_bool;
typedef signed char npy_byte;
typedef unsigned char npy_ubyte;
typedef short npy_short;
typedef unsigned short npy_ushort;
typedef int npy_int;
typedef unsigned int npy_uint;
typedef long npy_long;
typedef unsigned long npy_ulong;
typedef long long npy_longlong;
typedef unsigned long long npy_ulonglong;
typedef float npy_float;
typedef double npy_double;
typedef long double npy_longdouble;
typedef Py_ssize_t npy_intp;
typedef size_t npy_uintp;

#define _MOLT_NUMPY_OBJECT_HEAD PyObject *ob_base

typedef struct PyArray_Descr {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyTypeObject *typeobj;
    char kind;
    char type;
    char byteorder;
    char flags;
    int type_num;
    int elsize;
    int alignment;
} PyArray_Descr;

typedef struct PyArrayObject_fields {
    _MOLT_NUMPY_OBJECT_HEAD;
    char *data;
    int nd;
    npy_intp *dimensions;
    npy_intp *strides;
    PyObject *base;
    PyArray_Descr *descr;
    int flags;
} PyArrayObject_fields;

typedef PyArrayObject_fields PyArrayObject;

typedef struct PyArray_Dims {
    npy_intp *ptr;
    int len;
} PyArray_Dims;

typedef struct PyArray_DatetimeMetaData {
    int base;
    int num;
} PyArray_DatetimeMetaData;

typedef PyArray_DatetimeMetaData PyArray_DatetimeDTypeMetaData;

typedef struct PyArray_ArrFuncs {
    int _molt_reserved;
} PyArray_ArrFuncs;

typedef struct PyArrayFlagsObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject *array;
} PyArrayFlagsObject;

typedef struct PyArrayArrayConverterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyObject *object;
} PyArrayArrayConverterObject;

typedef struct PyArrayIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject *ao;
    npy_intp index;
    npy_intp size;
    char *dataptr;
} PyArrayIterObject;

typedef struct PyArrayMultiIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject **iters;
    int numiter;
} PyArrayMultiIterObject;

typedef struct PyArray_DTypeMeta {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArray_Descr *singleton;
} PyArray_DTypeMeta;

typedef struct PyArrayMethodObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyArrayMethodObject;

typedef struct PyArrayMethod_Context {
    PyArray_DTypeMeta *descriptors[3];
} PyArrayMethod_Context;

typedef int (*PyArrayMethod_StridedLoop)(
    PyArrayMethod_Context *context,
    char **data,
    npy_intp *dimensions,
    npy_intp *strides,
    void *auxdata
);

typedef struct PyArrayMethod_Spec {
    const char *name;
    int nin;
    int nout;
    int casting;
    int flags;
    void *slots;
} PyArrayMethod_Spec;

typedef struct PyArrayDTypeMeta_Spec {
    const char *name;
    int flags;
    void *slots;
} PyArrayDTypeMeta_Spec;

typedef struct PyUFuncObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int nin;
    int nout;
    int nargs;
} PyUFuncObject;

typedef struct PyVoidScalarObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyVoidScalarObject;

typedef struct PyDatetimeScalarObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyDatetimeScalarObject;

#define NPY_API_VERSION 0x00000012
#define NPY_FEATURE_VERSION 0x00000012

#define NPY_BOOL 0
#define NPY_BYTE 1
#define NPY_UBYTE 2
#define NPY_SHORT 3
#define NPY_USHORT 4
#define NPY_INT 5
#define NPY_UINT 6
#define NPY_LONG 7
#define NPY_ULONG 8
#define NPY_LONGLONG 9
#define NPY_ULONGLONG 10
#define NPY_FLOAT 11
#define NPY_DOUBLE 12
#define NPY_LONGDOUBLE 13
#define NPY_CFLOAT 14
#define NPY_CDOUBLE 15
#define NPY_CLONGDOUBLE 16
#define NPY_OBJECT 17
#define NPY_STRING 18
#define NPY_UNICODE 19
#define NPY_VOID 20
#define NPY_DATETIME 21
#define NPY_TIMEDELTA 22

#define NPY_ARRAY_C_CONTIGUOUS 0x0001
#define NPY_ARRAY_F_CONTIGUOUS 0x0002
#define NPY_ARRAY_OWNDATA 0x0004
#define NPY_ARRAY_WRITEABLE 0x0400

#define NPY_ITEM_REFCOUNT 0x01
#define NPY_NEEDS_PYAPI 0x02
#define NPY_LIST_PICKLE 0x04

#define PyTypeNum_ISBOOL(t) ((t) == NPY_BOOL)
#define PyTypeNum_ISINTEGER(t) ((t) >= NPY_BYTE && (t) <= NPY_ULONGLONG)
#define PyTypeNum_ISUNSIGNED(t) ((t) == NPY_UBYTE || (t) == NPY_USHORT || (t) == NPY_UINT || (t) == NPY_ULONG || (t) == NPY_ULONGLONG)
#define PyTypeNum_ISFLOAT(t) ((t) == NPY_FLOAT || (t) == NPY_DOUBLE || (t) == NPY_LONGDOUBLE)
#define PyTypeNum_ISCOMPLEX(t) ((t) == NPY_CFLOAT || (t) == NPY_CDOUBLE || (t) == NPY_CLONGDOUBLE)
#define PyTypeNum_ISNUMBER(t) (PyTypeNum_ISINTEGER(t) || PyTypeNum_ISFLOAT(t) || PyTypeNum_ISCOMPLEX(t))
#define PyTypeNum_ISSTRING(t) ((t) == NPY_STRING || (t) == NPY_UNICODE)
#define PyTypeNum_ISDATETIME(t) ((t) == NPY_DATETIME || (t) == NPY_TIMEDELTA)
#define PyTypeNum_ISUSERDEF(t) ((t) >= 256)
#define PyTypeNum_ISEXTENDED(t) PyTypeNum_ISUSERDEF(t)
#define PyTypeNum_ISFLEXIBLE(t) ((t) == NPY_STRING || (t) == NPY_UNICODE || (t) == NPY_VOID)

#ifdef __cplusplus
}
#endif

#endif
