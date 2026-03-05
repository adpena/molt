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
typedef signed char npy_int8;
typedef unsigned char npy_uint8;
typedef short npy_int16;
typedef unsigned short npy_uint16;
typedef int npy_int32;
typedef unsigned int npy_uint32;
typedef long long npy_int64;
typedef unsigned long long npy_uint64;
typedef Py_ssize_t npy_intp;
typedef size_t npy_uintp;
typedef intptr_t npy_hash_t;

#define _MOLT_NUMPY_OBJECT_HEAD PyObject *ob_base

static inline PyTypeObject *_molt_numpy_builtin_type_borrowed(const char *name) {
    return _molt_builtin_type_object_borrowed(name);
}

typedef struct NpyAuxData_tag NpyAuxData;
typedef struct PyArray_ArrFuncs PyArray_ArrFuncs;

typedef enum {
    NPY_BOOL = 0,
    NPY_BYTE = 1,
    NPY_UBYTE = 2,
    NPY_SHORT = 3,
    NPY_USHORT = 4,
    NPY_INT = 5,
    NPY_UINT = 6,
    NPY_LONG = 7,
    NPY_ULONG = 8,
    NPY_LONGLONG = 9,
    NPY_ULONGLONG = 10,
    NPY_FLOAT = 11,
    NPY_DOUBLE = 12,
    NPY_LONGDOUBLE = 13,
    NPY_CFLOAT = 14,
    NPY_CDOUBLE = 15,
    NPY_CLONGDOUBLE = 16,
    NPY_OBJECT = 17,
    NPY_STRING = 18,
    NPY_UNICODE = 19,
    NPY_VOID = 20,
    NPY_DATETIME = 21,
    NPY_TIMEDELTA = 22,
    NPY_HALF = 23,
    NPY_CHAR = 24,
    NPY_NOTYPE = 25,
    NPY_USERDEF = 256
} NPY_TYPES;

typedef enum {
    NPY_NO_CASTING = 0,
    NPY_EQUIV_CASTING = 1,
    NPY_SAFE_CASTING = 2,
    NPY_SAME_KIND_CASTING = 3,
    NPY_UNSAFE_CASTING = 4
} NPY_CASTING;

typedef enum {
    NPY_ANYORDER = -1,
    NPY_CORDER = 0,
    NPY_FORTRANORDER = 1,
    NPY_KEEPORDER = 2
} NPY_ORDER;

typedef enum {
    NPY_QUICKSORT = 0,
    NPY_HEAPSORT = 1,
    NPY_MERGESORT = 2,
    NPY_STABLESORT = 2,
    NPY_SORT_DEFAULT = 0,
    NPY_SORT_STABLE = 2,
    NPY_SORT_DESCENDING = 4
} NPY_SORTKIND;

typedef enum {
    NPY_CLIP = 0,
    NPY_WRAP = 1,
    NPY_RAISE = 2
} NPY_CLIPMODE;

typedef enum {
    NPY_SEARCHLEFT = 0,
    NPY_SEARCHRIGHT = 1
} NPY_SEARCHSIDE;

typedef enum {
    NPY_NOSCALAR = -1,
    NPY_BOOL_SCALAR = 0,
    NPY_INTPOS_SCALAR = 1,
    NPY_INTNEG_SCALAR = 2,
    NPY_FLOAT_SCALAR = 3,
    NPY_COMPLEX_SCALAR = 4,
    NPY_OBJECT_SCALAR = 5
} NPY_SCALARKIND;

typedef enum {
    NPY_FR_Y = 0,
    NPY_FR_M = 1,
    NPY_FR_W = 2,
    NPY_FR_D = 4,
    NPY_FR_h = 5,
    NPY_FR_m = 6,
    NPY_FR_s = 7,
    NPY_FR_ms = 8,
    NPY_FR_us = 9,
    NPY_FR_ns = 10,
    NPY_FR_ps = 11,
    NPY_FR_fs = 12,
    NPY_FR_as = 13,
    NPY_FR_GENERIC = 14
} NPY_DATETIMEUNIT;

typedef struct PyArray_Descr {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyTypeObject *typeobj;
    char kind;
    char type;
    char byteorder;
    npy_uint64 flags;
    int type_num;
    int elsize;
    int alignment;
    void *subarray;
    PyObject *names;
    PyObject *fields;
    PyObject *metadata;
} PyArray_Descr;

typedef struct _arr_descr {
    PyArray_Descr *base;
    PyObject *shape;
} PyArray_ArrayDescr;

typedef struct {
    PyArray_Descr base;
    PyObject *fields;
    PyObject *names;
    PyArray_ArrFuncs *f;
    PyObject *metadata;
    NpyAuxData *c_metadata;
    npy_hash_t hash;
} PyArray_DescrProto;

typedef struct PyArrayObject_fields {
    _MOLT_NUMPY_OBJECT_HEAD;
    char *data;
    int nd;
    npy_intp *dimensions;
    npy_intp *strides;
    PyObject *base;
    PyArray_Descr *descr;
    int flags;
    PyObject *weakreflist;
    void *mem_handler;
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

typedef int (*PyArray_CompareFunc)(const void *, const void *, PyArrayObject *);
typedef int (*PyArray_ArgFunc)(const void *, npy_intp, void *);
typedef int (*PyArray_ArgSortFunc)(void *, npy_intp *, npy_intp, void *);
typedef void (*PyArray_CopySwapFunc)(void *, const void *, int, void *);
typedef void (*PyArray_CopySwapNFunc)(void *, npy_intp, const void *, npy_intp, npy_intp, int, void *);

typedef struct PyArray_ArrFuncs {
    PyArray_CompareFunc compare;
    PyArray_ArgFunc argmax;
    PyArray_ArgFunc argmin;
    PyArray_ArgSortFunc argsort;
    PyArray_CopySwapFunc copyswap;
    PyArray_CopySwapNFunc copyswapn;
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

typedef PyArrayIterObject PyArrayIterObject_tag;

typedef struct PyArrayMultiIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject **iters;
    int numiter;
} PyArrayMultiIterObject;

typedef struct PyArrayMapIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyArrayMapIterObject;

typedef struct PyArrayNeighborhoodIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject *ao;
    npy_intp index;
} PyArrayNeighborhoodIterObject;

typedef struct PyArray_DTypeMeta {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArray_Descr *singleton;
    int type_num;
    PyTypeObject *scalar_type;
    npy_uint64 flags;
    void *dt_slots;
    void *reserved[3];
} PyArray_DTypeMeta;

typedef PyArray_DTypeMeta PyArray_DTypeMeta_tag;

typedef struct PyArrayMethodObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    const char *name;
    int nin;
    int nout;
    int casting;
    int flags;
    PyArray_DTypeMeta **dtypes;
    void *slots;
} PyArrayMethodObject;

typedef PyArrayMethodObject PyArrayMethodObject_tag;

typedef struct PyBoundArrayMethodObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayMethodObject *method;
} PyBoundArrayMethodObject;

typedef struct PyArrayMethod_Context {
    PyObject *caller;
    PyArrayMethodObject *method;
    PyArray_Descr *const *descriptors;
    void *_reserved;
    npy_uint64 flags;
    void *parameters;
} PyArrayMethod_Context;

typedef PyArrayMethod_Context PyArrayMethod_Context_tag;

typedef int (*PyArrayMethod_StridedLoop)(
    PyArrayMethod_Context *context,
    char *const *data,
    const npy_intp *dimensions,
    const npy_intp *strides,
    NpyAuxData *auxdata
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

typedef struct PyArray_Chunk {
    npy_intp start;
    npy_intp stop;
} PyArray_Chunk;

typedef struct PyArrayInterface {
    int two;
    int nd;
    char typekind;
    int itemsize;
    int flags;
    npy_intp *shape;
    npy_intp *strides;
    void *data;
    PyObject *descr;
} PyArrayInterface;

typedef struct PyUFuncObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int nin;
    int nout;
    int nargs;
} PyUFuncObject;

typedef enum {
    NPY_COPY_NEVER = 0,
    NPY_COPY_ALWAYS = 1,
    NPY_COPY_IF_NEEDED = 2
} NPY_COPYMODE;

#define _MOLT_NUMPY_DEFINE_SCALAR_OBJECT(name, field_type) \
    typedef struct name {                                  \
        _MOLT_NUMPY_OBJECT_HEAD;                           \
        field_type obval;                                  \
    } name

_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyBoolScalarObject, npy_bool);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyByteScalarObject, npy_byte);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyUByteScalarObject, npy_ubyte);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyShortScalarObject, npy_short);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyUShortScalarObject, npy_ushort);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyIntScalarObject, npy_int);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyUIntScalarObject, npy_uint);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyLongScalarObject, npy_long);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyULongScalarObject, npy_ulong);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyLongLongScalarObject, npy_longlong);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyULongLongScalarObject, npy_ulonglong);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyHalfScalarObject, npy_short);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyFloatScalarObject, npy_float);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyDoubleScalarObject, npy_double);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyLongDoubleScalarObject, npy_longdouble);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCFloatScalarObject, Py_complex);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCDoubleScalarObject, Py_complex);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCLongDoubleScalarObject, Py_complex);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyVoidScalarObject, int);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyDatetimeScalarObject, int);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyTimedeltaScalarObject, int);

#undef _MOLT_NUMPY_DEFINE_SCALAR_OBJECT

typedef void (NpyAuxData_FreeFunc)(NpyAuxData *);
typedef NpyAuxData *(NpyAuxData_CloneFunc)(NpyAuxData *);

struct NpyAuxData_tag {
    NpyAuxData_FreeFunc *free;
    NpyAuxData_CloneFunc *clone;
};

typedef PyObject *(PyArray_GetItemFunc)(void *, void *);
typedef void (PyArray_VectorUnaryFunc)(void *, void *, npy_intp, void *, void *);
typedef struct npy_unpacked_static_string {
    size_t size;
    const char *buf;
} npy_static_string;

typedef struct npy_string_allocator npy_string_allocator;

typedef struct {
    void *ctx;
    void *(*malloc)(void *ctx, size_t size);
    void *(*calloc)(void *ctx, size_t nelem, size_t elsize);
    void *(*realloc)(void *ctx, void *ptr, size_t new_size);
    void (*free)(void *ctx, void *ptr, size_t size);
} PyDataMemAllocator;

typedef struct {
    char name[127];
    unsigned char version;
    PyDataMemAllocator allocator;
} PyDataMem_Handler;

typedef struct {
    PyArray_Descr base;
    PyObject *na_object;
    char coerce;
    char has_nan_na;
    char has_string_na;
    char array_owned;
    npy_static_string default_string;
    npy_static_string na_name;
    npy_string_allocator *allocator;
} PyArray_StringDTypeObject;

#define NPY_API_VERSION 0x00000012
#define NPY_FEATURE_VERSION 0x00000012
#define NPY_FAIL 0
#define NPY_SUCCEED 1
#define NPY_NTYPES_LEGACY 24
#define NPY_INTP NPY_LONGLONG
#define NPY_UINTP NPY_ULONGLONG
#define NPY_UINT8 NPY_UBYTE
#define NPY_DEFAULT_INT NPY_INTP

#define NPY_ARRAY_C_CONTIGUOUS 0x0001
#define NPY_ARRAY_F_CONTIGUOUS 0x0002
#define NPY_ARRAY_OWNDATA 0x0004
#define NPY_ARRAY_FORCECAST 0x0010
#define NPY_ARRAY_ENSURECOPY 0x0020
#define NPY_ARRAY_ENSUREARRAY 0x0040
#define NPY_ARRAY_NOTSWAPPED 0x0080
#define NPY_ARRAY_ALIGNED 0x0100
#define NPY_ARRAY_WRITEBACKIFCOPY 0x0200
#define NPY_ARRAY_WRITEABLE 0x0400
#define NPY_ARRAY_BEHAVED (NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE)
#define NPY_ARRAY_BEHAVED_NS (NPY_ARRAY_BEHAVED | NPY_ARRAY_NOTSWAPPED)
#define NPY_ARRAY_CARRAY (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_BEHAVED)
#define NPY_ARRAY_CARRAY_RO (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define NPY_ARRAY_FARRAY (NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_BEHAVED)
#define NPY_ARRAY_FARRAY_RO (NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define NPY_ARRAY_DEFAULT NPY_ARRAY_CARRAY
#define NPY_ARRAY_UPDATE_ALL (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_ALIGNED)

#define NPY_ITEM_REFCOUNT 0x01
#define NPY_NEEDS_PYAPI 0x02
#define NPY_LIST_PICKLE 0x04

#define PyTypeNum_ISBOOL(t) ((t) == NPY_BOOL)
#define PyTypeNum_ISINTEGER(t) ((t) >= NPY_BYTE && (t) <= NPY_ULONGLONG)
#define PyTypeNum_ISUNSIGNED(t) ((t) == NPY_UBYTE || (t) == NPY_USHORT || (t) == NPY_UINT || (t) == NPY_ULONG || (t) == NPY_ULONGLONG)
#define PyTypeNum_ISSIGNED(t) (PyTypeNum_ISINTEGER(t) && !PyTypeNum_ISUNSIGNED(t))
#define PyTypeNum_ISFLOAT(t) ((t) == NPY_FLOAT || (t) == NPY_DOUBLE || (t) == NPY_LONGDOUBLE)
#define PyTypeNum_ISCOMPLEX(t) ((t) == NPY_CFLOAT || (t) == NPY_CDOUBLE || (t) == NPY_CLONGDOUBLE)
#define PyTypeNum_ISNUMBER(t) (PyTypeNum_ISINTEGER(t) || PyTypeNum_ISFLOAT(t) || PyTypeNum_ISCOMPLEX(t))
#define PyTypeNum_ISSTRING(t) ((t) == NPY_STRING || (t) == NPY_UNICODE)
#define PyTypeNum_ISDATETIME(t) ((t) == NPY_DATETIME || (t) == NPY_TIMEDELTA)
#define PyTypeNum_ISOBJECT(t) ((t) == NPY_OBJECT)
#define PyTypeNum_ISUSERDEF(t) ((t) >= NPY_USERDEF)
#define PyTypeNum_ISEXTENDED(t) PyTypeNum_ISUSERDEF(t)
#define PyTypeNum_ISFLEXIBLE(t) ((t) == NPY_STRING || (t) == NPY_UNICODE || (t) == NPY_VOID)

static inline int PyDataType_ELSIZE(const PyArray_Descr *descr) {
    return descr != NULL ? descr->elsize : 0;
}

static inline int PyDataType_FLAGS(const PyArray_Descr *descr) {
    return descr != NULL ? descr->flags : 0;
}

static inline int PyDataType_ALIGNMENT(const PyArray_Descr *descr) {
    return descr != NULL ? descr->alignment : 0;
}

#define PyDataType_SET_ELSIZE(descr, value) ((descr) != NULL ? ((descr)->elsize = (int)(value)) : 0)
#define PyDataType_NAMES(descr) ((descr) != NULL ? (descr)->names : NULL)
#define PyDataType_FIELDS(descr) ((descr) != NULL ? (descr)->fields : NULL)
#define PyDataType_SUBARRAY(descr) ((descr) != NULL ? (descr)->subarray : NULL)
#define PyDataType_SHAPE(descr) \
    ((PyDataType_SUBARRAY(descr) != NULL) ? ((PyArray_ArrayDescr *)PyDataType_SUBARRAY(descr))->shape : NULL)
#define PyDataType_METADATA(descr) ((descr) != NULL ? (descr)->metadata : NULL)
#define PyDataType_FLAGCHK(descr, flag) (((descr) != NULL) && (((descr)->flags & (flag)) == (flag)))
#define PyDataType_ISBOOL(descr) PyTypeNum_ISBOOL((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISINTEGER(descr) PyTypeNum_ISINTEGER((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISUNSIGNED(descr) PyTypeNum_ISUNSIGNED((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISSIGNED(descr) PyTypeNum_ISSIGNED((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISFLOAT(descr) PyTypeNum_ISFLOAT((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISCOMPLEX(descr) PyTypeNum_ISCOMPLEX((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISNUMBER(descr) PyTypeNum_ISNUMBER((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISSTRING(descr) PyTypeNum_ISSTRING((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISOBJECT(descr) PyTypeNum_ISOBJECT((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISUSERDEF(descr) PyTypeNum_ISUSERDEF((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISEXTENDED(descr) PyTypeNum_ISEXTENDED((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISFLEXIBLE(descr) PyTypeNum_ISFLEXIBLE((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISBYTESWAPPED(descr) ((descr) != NULL && (descr)->byteorder != '=' && (descr)->byteorder != '|')
#define PyDataType_ISNOTSWAPPED(descr) (!PyDataType_ISBYTESWAPPED(descr))
#define PyDataType_ISLEGACY(descr) ((descr) != NULL && (descr)->type_num >= 0)

#define PyArray_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayDescr_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayDescr_TypeFull PyArrayDescr_Type
#define PyArrayDTypeMeta_Type (*_molt_numpy_builtin_type_borrowed("type"))
#define PyArrayMethod_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyGenericArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayArrayConverter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayFlags_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayFunctionDispatcher_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayMultiIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayNeighborhoodIter_Type (*_molt_numpy_builtin_type_borrowed("object"))

#define PyBoolArrType_Type (*_molt_numpy_builtin_type_borrowed("bool"))
#define PyByteArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyUByteArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyShortArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyUShortArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyIntArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyUIntArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyLongArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyULongArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyLongLongArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyULongLongArrType_Type (*_molt_numpy_builtin_type_borrowed("int"))
#define PyHalfArrType_Type (*_molt_numpy_builtin_type_borrowed("float"))
#define PyFloatArrType_Type (*_molt_numpy_builtin_type_borrowed("float"))
#define PyDoubleArrType_Type (*_molt_numpy_builtin_type_borrowed("float"))
#define PyLongDoubleArrType_Type (*_molt_numpy_builtin_type_borrowed("float"))
#define PyCFloatArrType_Type (*_molt_numpy_builtin_type_borrowed("complex"))
#define PyCDoubleArrType_Type (*_molt_numpy_builtin_type_borrowed("complex"))
#define PyCLongDoubleArrType_Type (*_molt_numpy_builtin_type_borrowed("complex"))
#define PyStringArrType_Type (*_molt_numpy_builtin_type_borrowed("bytes"))
#define PyUnicodeArrType_Type (*_molt_numpy_builtin_type_borrowed("str"))
#define PyVoidArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyDatetimeArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyTimedeltaArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyNumberArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyIntegerArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PySignedIntegerArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyUnsignedIntegerArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyInexactArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyFloatingArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyComplexFloatingArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyFlexibleArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyCharacterArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyObjectArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyTimeIntegerArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))

#define PyArray_BoolDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("bool"))
#define PyArray_PyLongDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("int"))
#define PyArray_PyFloatDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("float"))
#define PyArray_StringDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("str"))
#define PyArray_IntpDType PyArray_PyLongDType
#define PyArray_UIntpDType PyArray_PyLongDType
#define PyArray_BytesDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("bytes"))
#define PyArray_UnicodeDType PyArray_StringDType
#define PyArray_ObjectDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_PyComplexDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("complex"))
#define PyArray_DefaultIntDType PyArray_PyLongDType

static PyDataMem_Handler PyDataMem_DefaultHandler = {
    "molt",
    1,
    {NULL, NULL, NULL, NULL, NULL},
};

#define PyDataMem_NEW(size) PyMem_Malloc((size_t)(size))
#define PyDataMem_NEW_ZEROED(nelem, elsize) PyMem_Calloc((size_t)(nelem), (size_t)(elsize))
#define PyDataMem_RENEW(ptr, size) PyMem_Realloc((ptr), (size_t)(size))
#define PyDataMem_FREE(ptr) PyMem_Free((ptr))

static inline PyObject *PyDataMem_GetHandler(void) {
    Py_RETURN_NONE;
}

static inline PyObject *PyDataMem_SetHandler(PyObject *handler) {
    if (handler == NULL) {
        Py_RETURN_NONE;
    }
    Py_INCREF(handler);
    return handler;
}

#ifdef __cplusplus
}
#endif

#endif
