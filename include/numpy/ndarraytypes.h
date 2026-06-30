#ifndef MOLT_NUMPY_NDARRAYTYPES_H
#define MOLT_NUMPY_NDARRAYTYPES_H

#include <limits.h>
#include <Python.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef signed char npy_bool;
typedef int8_t npy_int8;
typedef uint8_t npy_uint8;
typedef int16_t npy_int16;
typedef uint16_t npy_uint16;
typedef int32_t npy_int32;
typedef uint32_t npy_uint32;
typedef int64_t npy_int64;
typedef uint64_t npy_uint64;
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
    PyObject *mem_handler;
} PyArrayObject_fields;

typedef PyArrayObject_fields PyArrayObject;

typedef struct PyArray_Dims {
    npy_intp *ptr;
    int len;
} PyArray_Dims;

typedef struct PyArray_ArrayDescr {
    PyArray_Descr *base;
    PyObject *shape;
} PyArray_ArrayDescr;

#ifndef NPY_LIKELY
#if defined(__GNUC__) || defined(__clang__)
#define NPY_LIKELY(x) __builtin_expect(!!(x), 1)
#define NPY_UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define NPY_LIKELY(x) (x)
#define NPY_UNLIKELY(x) (x)
#endif
#endif

typedef struct PyArray_DatetimeMetaData {
    int base;
    int num;
} PyArray_DatetimeMetaData;

typedef PyArray_DatetimeMetaData PyArray_DatetimeDTypeMetaData;

typedef struct PyArray_ArrFuncs {
    int _molt_reserved;
} PyArray_ArrFuncs;

typedef struct PyDataMemAllocator {
    void *ctx;
    void *(*malloc)(void *ctx, size_t size);
    void *(*calloc)(void *ctx, size_t nelem, size_t elsize);
    void *(*realloc)(void *ctx, void *ptr, size_t new_size);
    void (*free)(void *ctx, void *ptr, size_t size);
} PyDataMemAllocator;

typedef struct NpyAuxData_tag NpyAuxData;
typedef void (NpyAuxData_FreeFunc)(NpyAuxData *);
typedef NpyAuxData *(NpyAuxData_CloneFunc)(NpyAuxData *);

struct NpyAuxData_tag {
    NpyAuxData_FreeFunc *free;
    NpyAuxData_CloneFunc *clone;
};

typedef struct NPY_cast_info {
    void *func;
    NpyAuxData *auxdata;
} NPY_cast_info;

typedef struct PyArrayMethod_Context PyArrayMethod_Context;

typedef PyObject *(PyArray_GetItemFunc)(void *, void *);
typedef int (PyArray_SetItemFunc)(PyObject *, void *, void *);
typedef void (PyArray_CopySwapNFunc)(void *, npy_intp, void *, npy_intp, npy_intp, int, void *);
typedef void (PyArray_CopySwapFunc)(void *, void *, int, void *);
typedef npy_bool (PyArray_NonzeroFunc)(void *, void *);
typedef int (PyArray_CompareFunc)(const void *, const void *, void *);
typedef int (PyArray_ArgFunc)(void *, npy_intp, npy_intp *, void *);
typedef void (PyArray_DotFunc)(void *, npy_intp, void *, npy_intp, void *, npy_intp, void *);
typedef void (PyArray_VectorUnaryFunc)(void *, void *, npy_intp, void *, void *);
typedef int (PyArray_ScanFunc)(FILE *, void *, char *, PyArray_Descr *);
typedef int (PyArray_FromStrFunc)(char *, void *, char **, PyArray_Descr *);
typedef int (PyArray_FillFunc)(void *, npy_intp, void *);
typedef int (PyArray_SortFunc)(void *, npy_intp, void *);
typedef int (PyArray_ArgSortFunc)(void *, npy_intp *, npy_intp, void *);
typedef int (PyArray_FillWithScalarFunc)(void *, npy_intp, void *, void *);
typedef int (PyArray_ScalarKindFunc)(void *);
typedef int (PyArray_FinalizeFunc)(PyArrayObject *, PyObject *);
typedef int (PyArray_SortImpl)(PyArrayObject *, int, int);
typedef PyObject *(PyArray_ArgSortImpl)(PyArrayObject *, int, int);
typedef int (PyArray_AssignReduceIdentityFunc)(PyArrayObject *, PyObject *);
typedef void (PyArray_MaskedStridedUnaryOp)(
    char *, npy_intp, char *, npy_intp, char *, npy_intp, npy_intp, void *);
typedef int (PyArray_ReduceLoopFunc)(
    PyArrayMethod_Context *, char **, npy_intp *, npy_intp *, void *);
typedef int (PyArray_BinSearchFunc)(
    const void *, const void *, npy_intp, npy_intp *, PyArray_Descr *);
typedef int (PyArray_ArgBinSearchFunc)(
    const void *, const void *, npy_intp, npy_intp *, PyArray_Descr *);

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

#define NPY_MAXARGS 64
#define NPY_MAXDIMS 64

typedef struct PyArrayMultiIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int numiter;
    npy_intp size;
    npy_intp index;
    int nd;
    npy_intp dimensions[NPY_MAXDIMS];
    PyArrayIterObject *iters[NPY_MAXARGS];
} PyArrayMultiIterObject;

typedef struct PyArrayNeighborhoodIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArrayObject *ao;
    npy_intp index;
    npy_intp size;
    char *dataptr;
} PyArrayNeighborhoodIterObject;

typedef struct PyArrayMapIterObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyObject *index;
} PyArrayMapIterObject;

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

typedef struct PyArray_Chunk {
    npy_intp start;
    npy_intp end;
    npy_intp stride;
} PyArray_Chunk;

typedef struct {
    npy_intp perm;
    npy_intp stride;
} npy_stride_sort_item;

typedef PyObject PyArray_ArrayFunctionDispatcherObject;
typedef PyObject PyArrayIdentityHash;
typedef PyObject PyBoundArrayMethodObject;

typedef struct PyArray_DTypeMeta {
    _MOLT_NUMPY_OBJECT_HEAD;
    PyArray_Descr *singleton;
} PyArray_DTypeMeta;

typedef struct PyArrayMethodObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyArrayMethodObject;

struct PyArrayMethod_Context {
    PyArray_DTypeMeta *descriptors[3];
};

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
    int identity;
    void *functions;
    void *const *data;
    int ntypes;
    const char *name;
    const char *types;
    const char *doc;
    const char *core_signature;
    PyObject *identity_value;
} PyUFuncObject;

typedef struct PyVoidScalarObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyVoidScalarObject;

typedef struct PyDatetimeScalarObject {
    _MOLT_NUMPY_OBJECT_HEAD;
    int _molt_reserved;
} PyDatetimeScalarObject;

typedef PyObject PyBoolScalarObject;
typedef PyObject PyByteScalarObject;
typedef PyObject PyUByteScalarObject;
typedef PyObject PyShortScalarObject;
typedef PyObject PyUShortScalarObject;
typedef PyObject PyIntScalarObject;
typedef PyObject PyUIntScalarObject;
typedef PyObject PyLongScalarObject;
typedef PyObject PyULongScalarObject;
typedef PyObject PyLongLongScalarObject;
typedef PyObject PyULongLongScalarObject;
typedef PyObject PyFloatScalarObject;
typedef PyObject PyDoubleScalarObject;
typedef PyObject PyLongDoubleScalarObject;
typedef PyObject PyCFloatScalarObject;
typedef PyObject PyCDoubleScalarObject;
typedef PyObject PyCLongDoubleScalarObject;
typedef PyObject PyObjectScalarObject;
typedef PyObject PyStringScalarObject;
typedef PyObject PyUnicodeScalarObject;
typedef PyObject PyTimedeltaScalarObject;
typedef PyObject PyHalfScalarObject;
typedef PyObject PyIntUScalarObject;
typedef PyObject PyScalarObject;

#define NPY_API_VERSION 0x00000012
#define NPY_FEATURE_VERSION 0x00000012

enum NPY_TYPES {
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
    NPY_NTYPES_LEGACY = 24,
    NPY_NOTYPE = 25,
    NPY_USERDEF = 256,
    NPY_NTYPES_ABI_COMPATIBLE = 21,
    NPY_VSTRING = 2056,
};

#define NPY_INT8 NPY_BYTE
#define NPY_UINT8 NPY_UBYTE
#define NPY_INT16 NPY_SHORT
#define NPY_UINT16 NPY_USHORT
#define NPY_INT32 NPY_INT
#define NPY_UINT32 NPY_UINT
#define NPY_INT64 NPY_LONGLONG
#define NPY_UINT64 NPY_ULONGLONG
#define NPY_FLOAT32 NPY_FLOAT
#define NPY_FLOAT64 NPY_DOUBLE
#define NPY_FLOAT128 NPY_LONGDOUBLE
#define NPY_COMPLEX64 NPY_CFLOAT
#define NPY_COMPLEX128 NPY_CDOUBLE
#define NPY_COMPLEX256 NPY_CLONGDOUBLE
#if INTPTR_MAX == INT64_MAX
#define NPY_INTP NPY_LONGLONG
#define NPY_UINTP NPY_ULONGLONG
#else
#define NPY_INTP NPY_INT
#define NPY_UINTP NPY_UINT
#endif

#define NPY_MAX_INT8 127
#define NPY_MIN_INT8 -128
#define NPY_MAX_UINT8 255
#define NPY_MAX_INT16 32767
#define NPY_MIN_INT16 -32768
#define NPY_MAX_UINT16 65535
#define NPY_MAX_INT32 2147483647
#define NPY_MIN_INT32 (-NPY_MAX_INT32 - 1)
#define NPY_MAX_UINT32 4294967295U
#define NPY_MAX_INT64 9223372036854775807LL
#define NPY_MIN_INT64 (-NPY_MAX_INT64 - 1LL)
#define NPY_MAX_UINT64 18446744073709551615ULL
#define NPY_MAX_BYTE SCHAR_MAX
#define NPY_MIN_BYTE SCHAR_MIN
#define NPY_MAX_UBYTE UCHAR_MAX
#define NPY_MAX_SHORT SHRT_MAX
#define NPY_MIN_SHORT SHRT_MIN
#define NPY_MAX_USHORT USHRT_MAX
#define NPY_MAX_INT INT_MAX
#define NPY_MIN_INT INT_MIN
#define NPY_MAX_UINT UINT_MAX
#define NPY_MAX_LONG LONG_MAX
#define NPY_MIN_LONG LONG_MIN
#define NPY_MAX_ULONG ULONG_MAX
#define NPY_MAX_LONGLONG NPY_MAX_INT64
#define NPY_MIN_LONGLONG NPY_MIN_INT64
#define NPY_MAX_ULONGLONG NPY_MAX_UINT64
#define NPY_MIN_DATETIME NPY_MIN_INT64
#define NPY_MAX_DATETIME NPY_MAX_INT64
#define NPY_MIN_TIMEDELTA NPY_MIN_INT64
#define NPY_MAX_TIMEDELTA NPY_MAX_INT64
#if INTPTR_MAX == INT64_MAX
#define NPY_MAX_INTP NPY_MAX_LONGLONG
#define NPY_MIN_INTP NPY_MIN_LONGLONG
#define NPY_MAX_UINTP NPY_MAX_ULONGLONG
#else
#define NPY_MAX_INTP NPY_MAX_INT
#define NPY_MIN_INTP NPY_MIN_INT
#define NPY_MAX_UINTP NPY_MAX_UINT
#endif

#define NPY_NEIGHBORHOOD_ITER_ZERO_PADDING 0
#define NPY_NEIGHBORHOOD_ITER_ONE_PADDING 1
#define NPY_NEIGHBORHOOD_ITER_CONSTANT_PADDING 2
#define NPY_NEIGHBORHOOD_ITER_MIRROR_PADDING 3
#define NPY_NEIGHBORHOOD_ITER_CIRCULAR_PADDING 4

typedef enum {
    NPY_QUICKSORT = 0,
    NPY_HEAPSORT = 1,
    NPY_MERGESORT = 2,
    NPY_STABLESORT = 2,
    NPY_SORT_DEFAULT = 0,
    NPY_SORT_STABLE = 2,
    NPY_SORT_DESCENDING = 4,
} NPY_SORTKIND;

#define NPY_NSORTS (NPY_STABLESORT + 1)

typedef enum {
    NPY_INTROSELECT = 0,
} NPY_SELECTKIND;

typedef enum {
    NPY_SEARCHLEFT = 0,
    NPY_SEARCHRIGHT = 1,
} NPY_SEARCHSIDE;

typedef enum {
    NPY_NOSCALAR = -1,
    NPY_BOOL_SCALAR,
    NPY_INTPOS_SCALAR,
    NPY_INTNEG_SCALAR,
    NPY_FLOAT_SCALAR,
    NPY_COMPLEX_SCALAR,
    NPY_OBJECT_SCALAR,
} NPY_SCALARKIND;

typedef enum {
    NPY_ANYORDER = -1,
    NPY_CORDER = 0,
    NPY_FORTRANORDER = 1,
    NPY_KEEPORDER = 2,
} NPY_ORDER;

typedef enum {
    NPY_NO_CASTING = 0,
    NPY_EQUIV_CASTING = 1,
    NPY_SAFE_CASTING = 2,
    NPY_SAME_KIND_CASTING = 3,
    NPY_UNSAFE_CASTING = 4,
} NPY_CASTING;

#ifndef NPY_CONSTANT_PYSCALAR
#define NPY_CONSTANT_PYSCALAR 0
#endif
#ifndef NPY_CONSTANT_ZERO
#define NPY_CONSTANT_ZERO 1
#endif
#ifndef NPY_CONSTANT_ONE
#define NPY_CONSTANT_ONE 2
#endif
#ifndef NPY_CONSTANT_MINUS_ONE
#define NPY_CONSTANT_MINUS_ONE 3
#endif
#ifndef NPY_CONSTANT_INFINITY
#define NPY_CONSTANT_INFINITY 6
#endif
#ifndef NPY_CONSTANT_NAN
#define NPY_CONSTANT_NAN 7
#endif

#ifndef NPY_ALLOW_THREADS
#define NPY_ALLOW_THREADS 1
#endif

#define NPY_BEGIN_THREADS_DEF PyThreadState *_save = NULL;
#if NPY_ALLOW_THREADS
#define NPY_BEGIN_ALLOW_THREADS Py_BEGIN_ALLOW_THREADS
#define NPY_END_ALLOW_THREADS Py_END_ALLOW_THREADS
#define NPY_BEGIN_THREADS \
    do {                  \
        _save = PyEval_SaveThread(); \
    } while (0)
#define NPY_END_THREADS                       \
    do {                                      \
        if (_save != NULL) {                  \
            PyEval_RestoreThread(_save);      \
            _save = NULL;                     \
        }                                     \
    } while (0)
#define NPY_BEGIN_THREADS_THRESHOLDED(loop_size) \
    do {                                         \
        if ((loop_size) > 500) {                 \
            _save = PyEval_SaveThread();         \
        }                                        \
    } while (0)
#define NPY_ALLOW_C_API_DEF PyGILState_STATE __save__;
#define NPY_ALLOW_C_API \
    do {                \
        __save__ = PyGILState_Ensure(); \
    } while (0)
#define NPY_DISABLE_C_API \
    do {                  \
        PyGILState_Release(__save__); \
    } while (0)
#else
#define NPY_BEGIN_ALLOW_THREADS
#define NPY_END_ALLOW_THREADS
#define NPY_BEGIN_THREADS
#define NPY_END_THREADS
#define NPY_BEGIN_THREADS_THRESHOLDED(loop_size)
#define NPY_BEGIN_THREADS_DESCR(dtype)
#define NPY_END_THREADS_DESCR(dtype)
#define NPY_ALLOW_C_API_DEF
#define NPY_ALLOW_C_API
#define NPY_DISABLE_C_API
#endif

#define NPY_ARRAY_C_CONTIGUOUS 0x0001
#define NPY_ARRAY_F_CONTIGUOUS 0x0002
#define NPY_ARRAY_OWNDATA 0x0004
#define NPY_ARRAY_FORCECAST 0x0010
#define NPY_ARRAY_ENSURECOPY 0x0020
#define NPY_ARRAY_ENSUREARRAY 0x0040
#define NPY_ARRAY_ELEMENTSTRIDES 0x0080
#define NPY_ARRAY_ALIGNED 0x0100
#define NPY_ARRAY_WRITEABLE 0x0400
#define NPY_ARRAY_NOTSWAPPED 0x0200
#define NPY_ARRAY_WRITEBACKIFCOPY 0x2000
#define NPY_ARRAY_ENSURENOCOPY 0x4000
#define NPY_ARRAY_BEHAVED (NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE)
#define NPY_ARRAY_BEHAVED_NS \
    (NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE | NPY_ARRAY_NOTSWAPPED)
#define NPY_ARRAY_CARRAY (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_BEHAVED)
#define NPY_ARRAY_CARRAY_RO (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define NPY_ARRAY_FARRAY (NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_BEHAVED)
#define NPY_ARRAY_FARRAY_RO (NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define NPY_ARRAY_DEFAULT NPY_ARRAY_CARRAY
#define NPY_ARRAY_IN_ARRAY NPY_ARRAY_CARRAY_RO
#define NPY_ARRAY_OUT_ARRAY NPY_ARRAY_CARRAY
#define NPY_ARRAY_INOUT_ARRAY NPY_ARRAY_CARRAY

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
