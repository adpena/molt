#ifndef MOLT_NUMPY_NDARRAYTYPES_H
#define MOLT_NUMPY_NDARRAYTYPES_H

#include <inttypes.h>
#include <limits.h>

#include <Python.h>
#include <numpy/numpyconfig.h>
#include <numpy/npy_endian.h>
#include <numpy/utils.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned char npy_bool;
typedef signed char npy_byte;
typedef unsigned char npy_ubyte;
typedef short npy_short;
typedef unsigned short npy_ushort;
typedef int npy_int;
typedef unsigned int npy_uint;
typedef long npy_long;
typedef unsigned long npy_ulong;
typedef PY_LONG_LONG npy_longlong;
typedef unsigned PY_LONG_LONG npy_ulonglong;
typedef float npy_float;
typedef double npy_double;
#if NPY_SIZEOF_LONGDOUBLE == NPY_SIZEOF_DOUBLE
typedef double npy_longdouble;
#else
typedef long double npy_longdouble;
#endif
typedef float npy_float32;
typedef double npy_float64;
typedef signed char npy_int8;
typedef unsigned char npy_uint8;
typedef short npy_int16;
typedef unsigned short npy_uint16;
typedef int npy_int32;
typedef unsigned int npy_uint32;
#if NPY_SIZEOF_LONG == 8
typedef npy_long npy_int64;
typedef npy_ulong npy_uint64;
#else
typedef npy_longlong npy_int64;
typedef npy_ulonglong npy_uint64;
#endif
typedef npy_uint16 npy_half;
typedef npy_half npy_float16;
typedef Py_ssize_t npy_intp;
typedef size_t npy_uintp;
typedef intptr_t npy_hash_t;
typedef npy_int64 npy_datetime;
typedef npy_int64 npy_timedelta;

#if !defined(__STDC_NO_COMPLEX__)
typedef float _Complex _molt_npy_cfloat_value;
typedef double _Complex _molt_npy_cdouble_value;
#if NPY_SIZEOF_LONGDOUBLE == NPY_SIZEOF_DOUBLE
typedef double _Complex _molt_npy_clongdouble_value;
#else
typedef long double _Complex _molt_npy_clongdouble_value;
#endif
#else
typedef struct {
    npy_float real;
    npy_float imag;
} _molt_npy_cfloat_value;

typedef struct {
    npy_double real;
    npy_double imag;
} _molt_npy_cdouble_value;

typedef struct {
    npy_longdouble real;
    npy_longdouble imag;
} _molt_npy_clongdouble_value;
#endif

#ifndef NPY_INT64_FMT
#define NPY_INT64_FMT PRId64
#endif

#ifndef NPY_INT32_FMT
#define NPY_INT32_FMT PRId32
#endif

#define NPY_MAX_INT64 9223372036854775807LL
#define NPY_MIN_INT64 (-NPY_MAX_INT64 - 1LL)
#define NPY_MAX_UINT64 18446744073709551615ULL

#ifndef NPY_DATETIME_FMT
#define NPY_DATETIME_FMT NPY_INT64_FMT
#endif

#define NPY_DATETIME_NAT NPY_MIN_INT64

#define _MOLT_NUMPY_OBJECT_HEAD PyObject *ob_base
#define NPY_MAXDIMS 64
#define NPY_MAXDIMS_LEGACY_ITERS 32
#ifndef NPY_MAXARGS
#define NPY_MAXARGS 64
#endif

static inline PyTypeObject *_molt_numpy_builtin_type_borrowed(const char *name) {
    return _molt_builtin_type_object_borrowed(name);
}

typedef struct NpyAuxData_tag NpyAuxData;
typedef struct PyArray_ArrFuncs PyArray_ArrFuncs;
typedef struct PyArrayIterObject_tag PyArrayIterObject;

typedef enum NPY_TYPES {
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

enum NPY_TYPECHAR {
    NPY_BOOLLTR = '?',
    NPY_BYTELTR = 'b',
    NPY_UBYTELTR = 'B',
    NPY_SHORTLTR = 'h',
    NPY_USHORTLTR = 'H',
    NPY_INTLTR = 'i',
    NPY_UINTLTR = 'I',
    NPY_LONGLTR = 'l',
    NPY_ULONGLTR = 'L',
    NPY_LONGLONGLTR = 'q',
    NPY_ULONGLONGLTR = 'Q',
    NPY_HALFLTR = 'e',
    NPY_FLOATLTR = 'f',
    NPY_DOUBLELTR = 'd',
    NPY_LONGDOUBLELTR = 'g',
    NPY_CFLOATLTR = 'F',
    NPY_CDOUBLELTR = 'D',
    NPY_CLONGDOUBLELTR = 'G',
    NPY_OBJECTLTR = 'O',
    NPY_STRINGLTR = 'S',
    NPY_DEPRECATED_STRINGLTR2 = 'a',
    NPY_UNICODELTR = 'U',
    NPY_VOIDLTR = 'V',
    NPY_DATETIMELTR = 'M',
    NPY_TIMEDELTALTR = 'm',
    NPY_CHARLTR = 'c',
    NPY_VSTRINGLTR = 'T',
};

#if NPY_SIZEOF_LONG == 8
#define NPY_INT64 NPY_LONG
#define NPY_UINT64 NPY_ULONG
#else
#define NPY_INT64 NPY_LONGLONG
#define NPY_UINT64 NPY_ULONGLONG
#endif

#ifndef NPY_SAME_VALUE_CASTING_FLAG
#define NPY_SAME_VALUE_CASTING_FLAG 64
#endif

typedef enum {
    _NPY_ERROR_OCCURRED_IN_CAST = -1,
    NPY_NO_CASTING = 0,
    NPY_EQUIV_CASTING = 1,
    NPY_SAFE_CASTING = 2,
    NPY_SAME_KIND_CASTING = 3,
    NPY_UNSAFE_CASTING = 4,
    NPY_SAME_VALUE_CASTING = NPY_UNSAFE_CASTING | NPY_SAME_VALUE_CASTING_FLAG
} NPY_CASTING;

typedef enum {
    NPY_ANYORDER = -1,
    NPY_CORDER = 0,
    NPY_FORTRANORDER = 1,
    NPY_KEEPORDER = 2
} NPY_ORDER;

typedef enum {
    NPY_VALID = 0,
    NPY_SAME = 1,
    NPY_FULL = 2
} NPY_CORRELATEMODE;

#define NPY_LITTLE '<'
#define NPY_BIG '>'
#define NPY_NATIVE '='
#define NPY_SWAP 's'
#define NPY_IGNORE '|'

#if NPY_BYTE_ORDER == NPY_BIG_ENDIAN
#define NPY_NATBYTE NPY_BIG
#define NPY_OPPBYTE NPY_LITTLE
#else
#define NPY_NATBYTE NPY_LITTLE
#define NPY_OPPBYTE NPY_BIG
#endif

typedef enum {
    NPY_QUICKSORT = 0,
    NPY_HEAPSORT = 1,
    NPY_MERGESORT = 2,
    NPY_STABLESORT = 2,
    NPY_SORT_DEFAULT = 0,
    NPY_SORT_STABLE = 2,
    NPY_SORT_DESCENDING = 4
} NPY_SORTKIND;

#define NPY_NSORTS (NPY_STABLESORT + 1)
#define NPY_NTYPES_ABI_COMPATIBLE 21

typedef enum {
    NPY_INTROSELECT = 0,
} NPY_SELECTKIND;

#define NPY_NSELECTS (NPY_INTROSELECT + 1)

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

#define NPY_NSCALARKINDS (NPY_OBJECT_SCALAR + 1)

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

#define NPY_DATETIME_NUMUNITS (NPY_FR_GENERIC + 1)

typedef struct PyArray_Descr {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyTypeObject *typeobj;
    char kind;
    char type;
    char byteorder;
    char _former_flags;
    npy_uint64 flags;
    int type_num;
    int elsize;
    int alignment;
    void *subarray;
    PyObject *names;
    PyObject *fields;
    PyObject *metadata;
    npy_hash_t hash;
    NpyAuxData *c_metadata;
} PyArray_Descr;

typedef struct _arr_descr {
    PyArray_Descr *base;
    PyObject *shape;
} PyArray_ArrayDescr;

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyTypeObject *typeobj;
    char kind;
    char type;
    char byteorder;
    char flags;
    int type_num;
    int elsize;
    int alignment;
    PyArray_ArrayDescr *subarray;
    PyObject *fields;
    PyObject *names;
    PyArray_ArrFuncs *f;
    PyObject *metadata;
    NpyAuxData *c_metadata;
    npy_hash_t hash;
} PyArray_DescrProto;

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyTypeObject *typeobj;
    char kind;
    char type;
    char byteorder;
    char _former_flags;
    int type_num;
    npy_uint64 flags;
    npy_intp elsize;
    npy_intp alignment;
    PyObject *metadata;
    npy_hash_t hash;
    void *reserved_null[2];
    PyArray_ArrayDescr *subarray;
    PyObject *fields;
    PyObject *names;
    NpyAuxData *c_metadata;
} _PyArray_LegacyDescr;

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
    NPY_DATETIMEUNIT base;
    int num;
} PyArray_DatetimeMetaData;

typedef struct PyArray_DatetimeDTypeMetaData PyArray_DatetimeDTypeMetaData;

typedef struct {
    npy_int64 year;
    npy_int32 month;
    npy_int32 day;
    npy_int32 hour;
    npy_int32 min;
    npy_int32 sec;
    npy_int32 us;
    npy_int32 ps;
    npy_int32 as;
} npy_datetimestruct;

typedef struct {
    npy_int64 day;
    npy_int32 sec;
    npy_int32 us;
    npy_int32 ps;
    npy_int32 as;
} npy_timedeltastruct;

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
typedef enum {
    NPY_DEVICE_CPU = 0,
} NPY_DEVICE;
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
typedef struct {
    struct PyArray_DTypeMeta *dtype;
    PyArray_Descr *descr;
} npy_dtype_info;
#endif

typedef PyObject *(PyArray_GetItemFunc)(void *, void *);
typedef int (PyArray_SetItemFunc)(PyObject *, void *, void *);
typedef void (PyArray_CopySwapNFunc)(void *, npy_intp, void *, npy_intp, npy_intp, int, void *);
typedef void (PyArray_CopySwapFunc)(void *, void *, int, void *);
typedef npy_bool (PyArray_NonzeroFunc)(void *, void *);
typedef int (PyArray_CompareFunc)(const void *, const void *, void *);
typedef int (PyArray_ArgFunc)(void *, npy_intp, npy_intp *, void *);
typedef void (PyArray_DotFunc)(void *, npy_intp, void *, npy_intp, void *, npy_intp, void *);
typedef int (PyArray_ScanFunc)(FILE *, void *, char *, PyArray_Descr *);
typedef int (PyArray_FromStrFunc)(char *, void *, char **, PyArray_Descr *);
typedef int (PyArray_FillFunc)(void *, npy_intp, void *);
typedef int (PyArray_SortFunc)(void *, npy_intp, void *);
typedef int (PyArray_FillWithScalarFunc)(void *, npy_intp, void *, void *);
typedef int (PyArray_ScalarKindFunc)(void *);
typedef int (PyArray_ArgSortFunc)(void *, npy_intp *, npy_intp, void *);
typedef void (PyArray_VectorUnaryFunc)(void *, void *, npy_intp, void *, void *);

typedef struct PyArray_ArrFuncs {
    PyArray_VectorUnaryFunc *cast[NPY_NTYPES_ABI_COMPATIBLE];
    PyArray_GetItemFunc *getitem;
    PyArray_SetItemFunc *setitem;
    PyArray_CopySwapNFunc *copyswapn;
    PyArray_CopySwapFunc *copyswap;
    PyArray_CompareFunc *compare;
    PyArray_ArgFunc *argmax;
    PyArray_DotFunc *dotfunc;
    PyArray_ScanFunc *scanfunc;
    PyArray_FromStrFunc *fromstr;
    PyArray_NonzeroFunc *nonzero;
    PyArray_FillFunc *fill;
    PyArray_FillWithScalarFunc *fillwithscalar;
    PyArray_SortFunc *sort[NPY_NSORTS];
    PyArray_ArgSortFunc *argsort[NPY_NSORTS];
    PyObject *castdict;
    PyArray_ScalarKindFunc *scalarkind;
    int **cancastscalarkindto;
    int *cancastto;
    void *_unused1;
    void *_unused2;
    void *_unused3;
    PyArray_ArgFunc *argmin;
} PyArray_ArrFuncs;

typedef struct PyArrayFlagsObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject *array;
} PyArrayFlagsObject;

typedef struct PyArrayArrayConverterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyObject *object;
} PyArrayArrayConverterObject;

typedef char *(*npy_iter_get_dataptr_t)(PyArrayIterObject *iter, const npy_intp *);

struct PyArrayIterObject_tag {
    _MOLT_NUMPY_OBJECT_HEAD;
    int nd_m1;
    npy_intp index;
    npy_intp size;
    npy_intp coordinates[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp dims_m1[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp strides[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp backstrides[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp factors[NPY_MAXDIMS_LEGACY_ITERS];
    PyArrayObject *ao;
    char *dataptr;
    npy_bool contiguous;
    npy_intp bounds[NPY_MAXDIMS_LEGACY_ITERS][2];
    npy_intp limits[NPY_MAXDIMS_LEGACY_ITERS][2];
    npy_intp limits_sizes[NPY_MAXDIMS_LEGACY_ITERS];
    npy_iter_get_dataptr_t translate;
};

typedef struct PyArrayNeighborhoodIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int nd_m1;
    PyArrayObject *ao;
    npy_intp index;
    npy_intp size;
    npy_intp coordinates[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp dims_m1[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp strides[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp backstrides[NPY_MAXDIMS_LEGACY_ITERS];
    npy_intp factors[NPY_MAXDIMS_LEGACY_ITERS];
    char *dataptr;
    npy_bool contiguous;
    npy_intp bounds[NPY_MAXDIMS_LEGACY_ITERS][2];
    npy_intp limits[NPY_MAXDIMS_LEGACY_ITERS][2];
    npy_intp limits_sizes[NPY_MAXDIMS_LEGACY_ITERS];
    npy_iter_get_dataptr_t translate;
    npy_intp nd;
    npy_intp dimensions[NPY_MAXDIMS_LEGACY_ITERS];
    PyArrayIterObject *_internal_iter;
    char *constant;
    int mode;
} PyArrayNeighborhoodIterObject;

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

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
typedef struct PyArray_DTypeMeta {
    PyHeapTypeObject super;
    PyArray_Descr *singleton;
    int type_num;
    PyTypeObject *scalar_type;
    npy_uint64 flags;
    void *dt_slots;
    void *reserved[3];
} PyArray_DTypeMeta;
#else
typedef struct PyArray_DTypeMeta {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArray_Descr *singleton;
    int type_num;
    PyTypeObject *scalar_type;
    npy_uint64 flags;
    void *dt_slots;
    void *reserved[3];
} PyArray_DTypeMeta;
#endif

typedef PyArray_DTypeMeta PyArray_DTypeMeta_tag;

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
typedef struct PyArrayMethodObject_tag PyArrayMethodObject;
#else
typedef struct PyArrayMethodObject_tag {
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
#endif

typedef struct PyArrayMethod_Context_tag {
    PyObject *caller;
    PyArrayMethodObject *method;
    PyArray_Descr *const *descriptors;
    void *_reserved;
    npy_uint64 flags;
    void *parameters;
} PyArrayMethod_Context;

typedef PyArrayMethod_Context PyArrayMethod_Context_tag;

typedef int (PyArrayMethod_StridedLoop)(
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
    PyArray_DTypeMeta **dtypes;
    PyType_Slot *slots;
} PyArrayMethod_Spec;

typedef struct PyArrayDTypeMeta_Spec {
    const char *name;
    int flags;
    void *slots;
} PyArrayDTypeMeta_Spec;

typedef struct PyArray_Chunk {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyObject *base;
    void *ptr;
    npy_intp len;
    int flags;
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

typedef struct _tagPyUFuncObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int nin;
    int nout;
    int nargs;
    int identity;
    PyUFuncGenericFunction *functions;
    void *const *data;
    int ntypes;
    int reserved1;
    const char *name;
    const char *types;
    const char *doc;
    void *ptr;
    PyObject *obj;
    PyObject *userloops;
    int core_enabled;
    int core_num_dim_ix;
    int *core_num_dims;
    int *core_dim_ixs;
    int *core_offsets;
    char *core_signature;
    PyUFunc_TypeResolutionFunc *type_resolver;
    PyObject *dict;
#ifndef Py_LIMITED_API
    vectorcallfunc vectorcall;
#else
    void *vectorcall;
#endif
    void *reserved3;
    npy_uint32 *op_flags;
    npy_uint32 iter_flags;
    npy_intp *core_dim_sizes;
    npy_uint32 *core_dim_flags;
    PyObject *identity_value;
} PyUFuncObject;

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
typedef enum {
    NPY_COPY_NEVER = 0,
    NPY_COPY_ALWAYS = 1,
    NPY_COPY_IF_NEEDED = 2
} NPY_COPYMODE;
#endif

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
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyHalfScalarObject, npy_half);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyFloatScalarObject, npy_float);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyDoubleScalarObject, npy_double);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyLongDoubleScalarObject, npy_longdouble);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCFloatScalarObject, _molt_npy_cfloat_value);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCDoubleScalarObject, _molt_npy_cdouble_value);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyCLongDoubleScalarObject, _molt_npy_clongdouble_value);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyObjectScalarObject, PyObject *);
_MOLT_NUMPY_DEFINE_SCALAR_OBJECT(PyScalarObject, char);

#undef _MOLT_NUMPY_DEFINE_SCALAR_OBJECT

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    Py_ssize_t ob_size;
    char *obval;
    _PyArray_LegacyDescr *descr;
    int flags;
    PyObject *base;
#if NPY_FEATURE_VERSION >= NPY_1_20_API_VERSION
    void *_buffer_info;
#endif
} PyVoidScalarObject;

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    npy_datetime obval;
    PyArray_DatetimeMetaData obmeta;
} PyDatetimeScalarObject;

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    npy_timedelta obval;
    PyArray_DatetimeMetaData obmeta;
} PyTimedeltaScalarObject;

typedef void (NpyAuxData_FreeFunc)(NpyAuxData *);
typedef NpyAuxData *(NpyAuxData_CloneFunc)(NpyAuxData *);

struct NpyAuxData_tag {
    NpyAuxData_FreeFunc *free;
    NpyAuxData_CloneFunc *clone;
    void *reserved[2];
};

#define NPY_AUXDATA_FREE(auxdata) \
    do { \
        if ((auxdata) != NULL) { \
            (auxdata)->free(auxdata); \
        } \
    } while (0)

#define NPY_AUXDATA_CLONE(auxdata) \
    ((auxdata)->clone(auxdata))

struct PyArray_DatetimeDTypeMetaData {
    NpyAuxData base;
    PyArray_DatetimeMetaData meta;
};

typedef PyObject *(PyArray_GetItemFunc)(void *, void *);
typedef void (PyArray_BinSearchFunc)(
    const char *,
    const char *,
    char *,
    npy_intp,
    npy_intp,
    npy_intp,
    npy_intp,
    npy_intp,
    PyArrayObject *
);
typedef int (PyArray_ArgBinSearchFunc)(
    const char *,
    const char *,
    const char *,
    char *,
    npy_intp,
    npy_intp,
    npy_intp,
    npy_intp,
    npy_intp,
    npy_intp,
    PyArrayObject *
);
typedef int (PyArray_ArgPartitionFunc)(
    void *,
    npy_intp *,
    npy_intp,
    npy_intp,
    npy_intp *,
    npy_intp *,
    npy_intp,
    void *
);
typedef int (PyArray_FinalizeFunc)();
typedef int (PyArray_GetDTypeCopySwapFn)();
typedef int (PyArray_GetStridedCopySwapFn)();
typedef int (PyArray_GetStridedCopySwapPairFn)();
typedef int (PyArray_GetStridedNumericCastFn)();
typedef int (PyArray_MaskedStridedUnaryOp)();
typedef int (PyArray_PartitionFunc)();
typedef int (PyArray_ReduceLoopFunc)();
typedef int (PyArray_TransferMaskedStridedToNDim)();
typedef int (PyArray_TransferNDimToStrided)();
typedef int (PyArray_TransferStridedToNDim)();
typedef int (PyArray_AssignReduceIdentityFunc)();
typedef int (PyArray_ArgSortImpl)();
typedef int (PyArray_SortImpl)();

typedef struct {
    _MOLT_NUMPY_OBJECT_HEAD;
    npy_uint8 lane_data[64];
} PySIMDVectorObject;

static void *PyArray_StringDType_DTypeSpec = NULL;
static void *PyArray_StringDType_Slots = NULL;
static void *PyArray_StringDType_casts = NULL;
static void *PyArray_StringDType_members = NULL;
static void *PyArray_StringDType_methods = NULL;
typedef struct npy_unpacked_static_string {
    size_t size;
    const char *buf;
} npy_static_string;

typedef struct npy_packed_static_string npy_packed_static_string;

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

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
typedef enum {
    NPY_AS_TYPE_COPY_IF_NEEDED = 0,
    NPY_AS_TYPE_COPY_ALWAYS = 1,
} NPY_ASTYPECOPYMODE;
#endif

#define NPY_FAIL 0
#define NPY_SUCCEED 1
#define NPY_FALSE 0
#define NPY_TRUE 1
#define NPY_NTYPES_LEGACY 24
#define NPY_UINT8 NPY_UBYTE
#define NPY_DEFAULT_INT NPY_INTP
#define NPY_DEFAULT_TYPE NPY_DOUBLE

#ifndef NPY_INTP
#if defined(_WIN64)
#define NPY_INTP NPY_LONGLONG
#define NPY_UINTP NPY_ULONGLONG
#elif INTPTR_MAX == LONG_MAX
#define NPY_INTP NPY_LONG
#define NPY_UINTP NPY_ULONG
#elif INTPTR_MAX == INT_MAX
#define NPY_INTP NPY_INT
#define NPY_UINTP NPY_UINT
#else
#define NPY_INTP NPY_LONGLONG
#define NPY_UINTP NPY_ULONGLONG
#endif
#endif

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
#define NPY_ITEM_HASOBJECT 0x01
#define NPY_LIST_PICKLE 0x02
#define NPY_ITEM_IS_POINTER 0x04
#define NPY_NEEDS_INIT 0x08
#define NPY_NEEDS_PYAPI 0x10
#define NPY_USE_GETITEM 0x20
#define NPY_USE_SETITEM 0x40
#define NPY_ALIGNED_STRUCT 0x80
#define NPY_FROM_FIELDS (NPY_NEEDS_INIT | NPY_LIST_PICKLE | NPY_ITEM_REFCOUNT | NPY_NEEDS_PYAPI)
#define NPY_OBJECT_DTYPE_FLAGS \
    (NPY_LIST_PICKLE | NPY_USE_GETITEM | NPY_ITEM_IS_POINTER | NPY_ITEM_REFCOUNT | NPY_NEEDS_INIT | NPY_NEEDS_PYAPI)

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
#define PyDataType_SUBARRAY(descr) \
    ((descr) != NULL ? (PyArray_ArrayDescr *)(descr)->subarray : NULL)
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
#define PyDataType_ISDATETIME(descr) PyTypeNum_ISDATETIME((descr) != NULL ? (descr)->type_num : NPY_NOTYPE)
#define PyDataType_ISBYTESWAPPED(descr) ((descr) != NULL && (descr)->byteorder != '=' && (descr)->byteorder != '|')
#define PyDataType_ISNOTSWAPPED(descr) (!PyDataType_ISBYTESWAPPED(descr))
#define PyDataType_ISLEGACY(descr) ((descr) != NULL && (descr)->type_num >= 0)
#define PyDataType_ PyDataType_ELSIZE

#define PyArray_Type (*_molt_numpy_builtin_type_borrowed("object"))
#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
extern PyArray_DTypeMeta PyArrayDescr_TypeFull;
#define PyArrayDescr_Type (*(PyTypeObject *)&PyArrayDescr_TypeFull)
#else
#define PyArrayDescr_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayDescr_TypeFull PyArrayDescr_Type
#endif
#define PyArrayDTypeMeta_Type (*_molt_numpy_builtin_type_borrowed("type"))
#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyArrayMethod_Type (*_molt_numpy_builtin_type_borrowed("object"))
#endif
#define PyGenericArrType_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayArrayConverter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayFlags_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayFunctionDispatcher_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayMapIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayMultiIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayNeighborhoodIter_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyFortran_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_ PyArray_Check

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

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyArray_BoolDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("bool"))
#define PyArray_ByteDType PyArray_PyLongDType
#define PyArray_UByteDType PyArray_PyLongDType
#define PyArray_ShortDType PyArray_PyLongDType
#define PyArray_UShortDType PyArray_PyLongDType
#define PyArray_IntDType PyArray_PyLongDType
#define PyArray_UIntDType PyArray_PyLongDType
#define PyArray_LongDType PyArray_PyLongDType
#define PyArray_ULongDType PyArray_PyLongDType
#define PyArray_LongLongDType PyArray_PyLongDType
#define PyArray_ULongLongDType PyArray_PyLongDType
#define PyArray_Int8DType PyArray_PyLongDType
#define PyArray_UInt8DType PyArray_PyLongDType
#define PyArray_Int16DType PyArray_PyLongDType
#define PyArray_UInt16DType PyArray_PyLongDType
#define PyArray_Int32DType PyArray_PyLongDType
#define PyArray_UInt32DType PyArray_PyLongDType
#define PyArray_Int64DType PyArray_PyLongDType
#define PyArray_UInt64DType PyArray_PyLongDType
#define PyArray_PyLongDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("int"))
#define PyArray_PyFloatDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("float"))
#define PyArray_HalfDType PyArray_PyFloatDType
#define PyArray_FloatDType PyArray_PyFloatDType
#define PyArray_DoubleDType PyArray_PyFloatDType
#define PyArray_StringDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("str"))
#define PyArray_IntpDType PyArray_PyLongDType
#define PyArray_UIntpDType PyArray_PyLongDType
#define PyArray_BytesDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("bytes"))
#define PyArray_UnicodeDType PyArray_StringDType
#define PyArray_ObjectDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_VoidDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_DatetimeDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_PyComplexDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("complex"))
#define PyArray_CFloatDType PyArray_PyComplexDType
#define PyArray_CDoubleDType PyArray_PyComplexDType
#define PyArray_CLongDoubleDType PyArray_PyComplexDType
#define PyArray_LongDoubleDType PyArray_PyFloatDType
#define PyArray_ComplexAbstractDType PyArray_PyComplexDType
#define PyArray_DefaultIntDType PyArray_PyLongDType
#define PyArray_TimedeltaDType ((PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object"))
#define PyArray_IntAbstractDType PyArray_PyLongDType
#define PyArray_FloatAbstractDType PyArray_PyFloatDType
#endif

#ifndef NPY_BEGIN_THREADS_DEF
#define NPY_BEGIN_THREADS_DEF
#define NPY_BEGIN_THREADS do { } while (0)
#define NPY_END_THREADS do { } while (0)
#define NPY_BEGIN_THREADS_DESCR(dtype) do { (void)(dtype); } while (0)
#define NPY_END_THREADS_DESCR(dtype) do { (void)(dtype); } while (0)
#define NPY_BEGIN_THREADS_THRESHOLDED(loop_size) do { (void)(loop_size); } while (0)
#define NPY_ALLOW_C_API_DEF
#define NPY_ALLOW_C_API do { } while (0)
#define NPY_DISABLE_C_API do { } while (0)
#endif

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

static inline int PyArrayNeighborhoodIter_Next(PyArrayNeighborhoodIterObject *iter) {
    if (iter == NULL || iter->translate == NULL) {
        return -1;
    }
    iter->index += 1;
    iter->dataptr = iter->translate((PyArrayIterObject *)iter, iter->coordinates);
    return 0;
}

static inline int PyArrayNeighborhoodIter_Next2D(
    PyArrayNeighborhoodIterObject *iter,
    npy_intp *x,
    npy_intp *y
) {
    if (x != NULL) {
        *x = iter != NULL ? iter->coordinates[0] : 0;
    }
    if (y != NULL) {
        *y = iter != NULL ? iter->coordinates[1] : 0;
    }
    return PyArrayNeighborhoodIter_Next(iter);
}

static inline int PyArrayNeighborhoodIter_Reset(PyArrayNeighborhoodIterObject *iter) {
    npy_intp i;
    if (iter == NULL || iter->translate == NULL) {
        return -1;
    }
    for (i = 0; i < iter->nd; i++) {
        iter->coordinates[i] = iter->bounds[i][0];
    }
    iter->dataptr = iter->translate((PyArrayIterObject *)iter, iter->coordinates);
    return 0;
}

#ifdef __cplusplus
}
#endif

#endif
