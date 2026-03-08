#ifndef MOLT_NUMPY_NDARRAYOBJECT_H
#define MOLT_NUMPY_NDARRAYOBJECT_H

#include <numpy/ndarraytypes.h>
#include <numpy/dtype_api.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef MOLT_NUMPY_INTERNAL_BUILD
#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
#define MOLT_NUMPY_INTERNAL_BUILD 1
#else
#define MOLT_NUMPY_INTERNAL_BUILD 0
#endif
#endif

static inline int _molt_numpy_unavailable_i32(const char *name) {
    PyErr_Format(
        PyExc_RuntimeError,
        "%s is not yet implemented in Molt's NumPy compatibility layer",
        name);
    return -1;
}

static inline PyObject *_molt_numpy_unavailable_obj(const char *name) {
    PyErr_Format(
        PyExc_RuntimeError,
        "%s is not yet implemented in Molt's NumPy compatibility layer",
        name);
    return NULL;
}

#ifndef MOLT_NUMPY_MULTIARRAY_IMPORT_API
#define MOLT_NUMPY_MULTIARRAY_IMPORT_API 1

static void **PyArray_API = NULL;

static inline int _import_array(void) {
    void *api_ptr = PyCapsule_Import("numpy.core._multiarray_umath._ARRAY_API", 0);
    if (api_ptr == NULL) {
        return -1;
    }
    PyArray_API = (void **)api_ptr;
    return 0;
}

static inline int PyArray_ImportNumPyAPI(void) {
    return _import_array();
}

#define PyArray_RUNTIME_VERSION NPY_FEATURE_VERSION
#define import_array()                                                             \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            return NULL;                                                           \
        }                                                                          \
    } while (0);

#define import_array1(ret)                                                         \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            return (ret);                                                          \
        }                                                                          \
    } while (0);

#define import_array2(msg, ret)                                                    \
    do {                                                                           \
        if (_import_array() < 0) {                                                 \
            PyErr_SetString(PyExc_ImportError, (msg));                             \
            return (ret);                                                          \
        }                                                                          \
    } while (0);

#endif

static inline npy_intp _molt_pyarray_size(const PyArrayObject *array_obj) {
    npy_intp size = 1;
    int i = 0;
    if (array_obj == NULL || array_obj->dimensions == NULL) {
        return 0;
    }
    for (i = 0; i < array_obj->nd; i++) {
        size *= array_obj->dimensions[i];
    }
    return size;
}

#define PyArray_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_CheckExact(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_DescrCheck(op) PyObject_TypeCheck((PyObject *)(op), &PyArrayDescr_Type)
#define PyArray_IsZeroDim(op) (PyArray_Check(op) && (PyArray_NDIM((PyArrayObject *)(op)) == 0))

#define PyArray_DATA(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_BYTES(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_NDIM(arr) (((PyArrayObject_fields *)(arr))->nd)
#define PyArray_DIMS(arr) (((PyArrayObject_fields *)(arr))->dimensions)
#define PyArray_STRIDES(arr) (((PyArrayObject_fields *)(arr))->strides)
#define PyArray_STRIDE(arr, i) (((PyArrayObject_fields *)(arr))->strides[(i)])
#define PyArray_DIM(arr, i) (((PyArrayObject_fields *)(arr))->dimensions[(i)])
#define PyArray_SHAPE(arr) PyArray_DIMS(arr)
#define PyArray_BASE(arr) (((PyArrayObject_fields *)(arr))->base)
#define PyArray_DESCR(arr) (((PyArrayObject_fields *)(arr))->descr)
#define PyArray_DTYPE(arr) PyArray_DESCR(arr)
#define PyArray_FLAGS(arr) (((PyArrayObject_fields *)(arr))->flags)
#define PyArray_ITEMSIZE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->elsize : 0)
#define PyArray_SIZE(arr) _molt_pyarray_size((PyArrayObject *)(arr))
#define PyArray_NBYTES(arr) ((npy_intp)(PyArray_SIZE(arr) * (npy_intp)PyArray_ITEMSIZE(arr)))
#define PyArray_TYPE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->type_num : NPY_OBJECT)
#define PyArray_CHKFLAGS(arr, mask) (((PyArray_FLAGS(arr)) & (mask)) == (mask))
#define PyArray_IS_C_CONTIGUOUS(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_C_CONTIGUOUS) != 0)
#define PyArray_ISCONTIGUOUS(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_C_CONTIGUOUS)
#define PyArray_ISFORTRAN(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_F_CONTIGUOUS) != 0)
#define PyArray_IS_F_CONTIGUOUS(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_F_CONTIGUOUS)
#define PyArray_ISALIGNED(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_ALIGNED)
#define PyArray_ISBOOL(arr) PyTypeNum_ISBOOL(PyArray_TYPE(arr))
#define PyArray_ISINTEGER(arr) PyTypeNum_ISINTEGER(PyArray_TYPE(arr))
#define PyArray_ISFLOAT(arr) PyTypeNum_ISFLOAT(PyArray_TYPE(arr))
#define PyArray_ISCOMPLEX(arr) PyTypeNum_ISCOMPLEX(PyArray_TYPE(arr))
#define PyArray_ISSTRING(arr) PyTypeNum_ISSTRING(PyArray_TYPE(arr))
#define PyArray_ISFLEXIBLE(arr) PyTypeNum_ISFLEXIBLE(PyArray_TYPE(arr))
#define PyArray_ISOBJECT(arr) PyTypeNum_ISOBJECT(PyArray_TYPE(arr))
#define PyArray_ISSIGNED(arr) PyTypeNum_ISSIGNED(PyArray_TYPE(arr))
#define PyArray_ISWRITEABLE(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_WRITEABLE)
#define PyArray_ISONESEGMENT(arr) (PyArray_ISCONTIGUOUS(arr) || PyArray_ISFORTRAN(arr))
#define PyArray_ISCARRAY(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_CARRAY)
#define PyArray_ISCARRAY_RO(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_CARRAY_RO)
#define PyArray_ISFARRAY(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_FARRAY)
#define PyArray_ISFARRAY_RO(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_FARRAY_RO)
#define PyArray_HANDLER(arr) ((PyObject *)((PyArrayObject_fields *)(arr))->mem_handler)
#define PyArray_ISNBO(byteorder) ((byteorder) != NPY_OPPBYTE)
#define PyArray_IsNativeByteOrder PyArray_ISNBO
#define PyArray_ISNOTSWAPPED(arr) PyArray_ISNBO(PyArray_DESCR(arr)->byteorder)
#define PyArray_ISBYTESWAPPED(arr) (!PyArray_ISNOTSWAPPED(arr))
#define PyArray_ISDATETIME(arr) PyTypeNum_ISDATETIME(PyArray_TYPE(arr))
#define PyArray_ENABLEFLAGS(arr, mask) (((PyArrayObject_fields *)(arr))->flags |= (mask))
#define PyArray_CLEARFLAGS(arr, mask) (((PyArrayObject_fields *)(arr))->flags &= ~(mask))

#define PyDataType_FLAGCHK(descr, flag) (((descr) != NULL) && (((descr)->flags & (flag)) == (flag)))
#define PyDataType_REFCHK(descr) PyDataType_FLAGCHK((descr), NPY_ITEM_REFCOUNT)
#define PyDataType_ISLEGACY(descr) ((descr) != NULL && (descr)->type_num >= 0)
#define PyDataType_NAMES(descr) ((descr) != NULL ? (descr)->names : NULL)
#define PyDataType_FIELDS(descr) ((descr) != NULL ? (descr)->fields : NULL)
#define PyDataType_HASFIELDS(descr) (PyDataType_NAMES((descr)) != NULL || PyDataType_FIELDS((descr)) != NULL)
#define PyDataType_HASSUBARRAY(descr) (PyDataType_SUBARRAY(descr) != NULL)
#define PyDataType_ISUNSIZED(descr) ((descr) != NULL && (descr)->elsize == 0 && !PyDataType_HASFIELDS(descr))
#define PyDataType_C_METADATA(descr) ((descr) != NULL ? (descr)->c_metadata : NULL)

#define PyArray_malloc PyMem_Malloc
#define PyArray_free PyMem_Free
#ifndef PyDataMem_NEW
#define PyDataMem_NEW(size) PyMem_Malloc((size))
#endif
#ifndef PyDataMem_FREE
#define PyDataMem_FREE(ptr) PyMem_Free((ptr))
#endif
#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyDataMem_UserNEW(size, handler) ((void)(handler), PyMem_Malloc((size)))
#define PyDataMem_UserNEW_ZEROED(nmemb, size, handler) ((void)(handler), PyMem_Calloc((nmemb), (size)))
#define PyDataMem_UserRENEW(ptr, size, handler) ((void)(handler), PyMem_Realloc((ptr), (size)))
#define PyDataMem_UserFREE(ptr, size, handler) do { (void)(handler); (void)(size); PyMem_Free((ptr)); } while (0)
#endif
#define PyArray_MIN(a, b) ((a) < (b) ? (a) : (b))
#define PyArray_MAX(a, b) ((a) > (b) ? (a) : (b))
#define PyArray_FROM_OF(obj, flags) PyArray_CheckFromAny((obj), NULL, 0, 0, (flags), NULL)
#define PyArray_SimpleNew(nd, dims, typenum) \
    PyArray_New(&PyArray_Type, (nd), (dims), (typenum), NULL, NULL, 0, 0, NULL)
#define PyArray_SimpleNewFromDescr(nd, dims, descr) \
    PyArray_NewFromDescr(&PyArray_Type, (descr), (nd), (dims), NULL, NULL, 0, NULL)
#define PyArray_ToScalar(data, arr) PyArray_Scalar((data), PyArray_DESCR(arr), (PyObject *)(arr))
#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyBoundArrayMethod_Type PyArrayMethod_Type
#endif
#define PyArray_DESCR_REPLACE(descr) do { \
    PyArray_Descr *_molt_new_descr = PyArray_DescrNew((descr)); \
    if ((descr) != NULL) { \
        PyMem_Free((descr)); \
    } \
    (descr) = _molt_new_descr; \
} while (0)

#define PyArray_FROM_O(obj) ((PyArrayObject *)(obj))
#define PyArray_FROMANY(obj, type, min_depth, max_depth, flags) \
    PyArray_FromAny((obj), PyArray_DescrFromType((type)), (min_depth), (max_depth), (flags), NULL)
#define PyArray_FROM_OT(obj, type) \
    PyArray_FromAny((obj), PyArray_DescrFromType((type)), 0, 0, 0, NULL)
#define PyArray_FROM_OTF(obj, type, flags)                                        \
    PyArray_FromAny((obj), PyArray_DescrFromType((type)), 0, 0,                   \
                    (((flags) & NPY_ARRAY_ENSURECOPY)                              \
                         ? ((flags) | NPY_ARRAY_DEFAULT)                           \
                         : (flags)),                                               \
                    NULL)
#define PyArray_Cast(mp, type_num) \
    PyArray_CastToType((mp), PyArray_DescrFromType((type_num)), 0)
#define PyArray_ContiguousFromAny(obj, type, min_depth, max_depth) \
    PyArray_FromAny((obj), PyArray_DescrFromType((type)), (min_depth), (max_depth), NPY_ARRAY_DEFAULT, NULL)
#define PyArray_ContiguousFromObject(obj, type, min_depth, max_depth) \
    PyArray_ContiguousFromAny((obj), (type), (min_depth), (max_depth))
#define PyArray_Copy(obj) PyArray_NewCopy((PyArrayObject *)(obj), NPY_CORDER)
#define PyArray_Zeros(...) _molt_numpy_unavailable_obj("PyArray_Zeros")
#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#ifndef PyArray_MultiIter_DATA
#define PyArray_MultiIter_DATA(iter, i) ((void)(iter), (void)(i), (char *)NULL)
#endif
#ifndef PyArray_MultiIter_NEXT
#define PyArray_MultiIter_NEXT(iter) ((void)(iter))
#endif
#ifndef PyArray_ITER_NEXT
#define PyArray_ITER_NEXT(iter) ((void)(iter))
#endif
#endif
#define PyArray_FILLWBYTE(obj, val) memset(PyArray_DATA(obj), (val), (size_t)PyArray_NBYTES(obj))
#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
#define PyArray_INCREF(obj) Py_INCREF((PyObject *)(obj))
#define PyArray_XDECREF(obj) Py_XDECREF((PyObject *)(obj))
#define PyArray_Item_INCREF(obj, ...) Py_INCREF((PyObject *)(obj))
#define PyArray_Item_XDECREF(obj, ...) Py_XDECREF((PyObject *)(obj))
#endif

#define PyArray_IsScalar(obj, cls) PyObject_TypeCheck((obj), &Py##cls##ArrType_Type)
#define PyArray_IsPythonNumber(obj) \
    (PyFloat_Check(obj) || PyComplex_Check(obj) || PyLong_Check(obj) || PyBool_Check(obj))
#define PyArray_IsIntegerScalar(obj) \
    (PyLong_Check(obj) || PyArray_IsScalar((obj), Integer))
#define PyArray_IsPythonScalar(obj) \
    (PyArray_IsPythonNumber(obj) || PyBytes_Check(obj) || PyUnicode_Check(obj))
#define PyArray_IsAnyScalar(obj) \
    (PyArray_IsScalar((obj), Generic) || PyArray_IsPythonScalar(obj))
#define PyArray_CheckScalar(obj) (PyArray_IsScalar((obj), Generic) || PyArray_IsZeroDim((obj)))
#define PyArray_CheckAnyScalar(obj) (PyArray_CheckScalar((obj)) || PyBool_Check(obj) || PyLong_Check(obj) || PyFloat_Check(obj) || PyComplex_Check(obj) || PyBytes_Check(obj) || PyUnicode_Check(obj))
#define PyArray_HASFIELDS(obj) PyDataType_HASFIELDS(PyArray_DESCR(obj))
#define DEPRECATE(msg) PyErr_WarnEx(PyExc_DeprecationWarning, (msg), 1)
#define DEPRECATE_FUTUREWARNING(msg) PyErr_WarnEx(PyExc_FutureWarning, (msg), 1)

static inline int NPY_TITLE_KEY_check(PyObject *key, PyObject *value) {
    PyObject *title;
    if (PyTuple_Size(value) != 3) {
        return 0;
    }
    title = PyTuple_GetItem(value, 2);
    if (key == title) {
        return 1;
    }
#ifdef PYPY_VERSION
    if (PyUnicode_Check(title) && PyUnicode_Check(key)) {
        return PyUnicode_Compare(title, key) == 0 ? 1 : 0;
    }
#endif
    return 0;
}

#define NPY_TITLE_KEY(key, value) (NPY_TITLE_KEY_check((key), (value)))

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_CheckAnyScalarExact(PyObject *obj) {
    return PyArray_CheckAnyScalar(obj);
}
#else
NPY_NO_EXPORT int PyArray_CheckAnyScalarExact(PyObject *obj);
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyArray_ArrFuncs *PyDataType_GetArrFuncs(const PyArray_Descr *descr) {
    (void)descr;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyDataType_GetArrFuncs is not yet implemented in Molt's NumPy compatibility layer");
    return NULL;
}
#else
NPY_NO_EXPORT PyArray_ArrFuncs *PyDataType_GetArrFuncs(const PyArray_Descr *descr);
#endif

static inline PyArray_Descr *PyArray_DescrFromType(int typenum) {
    PyArray_Descr *descr = (PyArray_Descr *)PyMem_Calloc(1, sizeof(PyArray_Descr));
    if (descr == NULL) {
        return NULL;
    }
    descr->type_num = typenum;
    descr->elsize = sizeof(double);
    descr->byteorder = '=';
    return descr;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_DescrNewFromType(int typenum);
#else
static inline PyArray_Descr *PyArray_DescrNewFromType(int typenum) {
    return PyArray_DescrFromType(typenum);
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_DescrNew(PyArray_Descr *descr);
#else
static inline PyArray_Descr *PyArray_DescrNew(PyArray_Descr *descr) {
    PyArray_Descr *copy;
    if (descr == NULL) {
        return NULL;
    }
    copy = (PyArray_Descr *)PyMem_Malloc(sizeof(PyArray_Descr));
    if (copy == NULL) {
        return NULL;
    }
    *copy = *descr;
    return copy;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_DescrFromScalar(PyObject *obj);
NPY_NO_EXPORT PyArray_Descr *PyArray_DescrFromTypeObject(PyObject *type);
NPY_NO_EXPORT PyArray_Descr *PyArray_GetDefaultDescr(PyArray_DTypeMeta *DType);
#else
static inline PyArray_Descr *PyArray_DescrFromScalar(PyObject *obj) {
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyArray_Descr *PyArray_DescrFromTypeObject(PyObject *type) {
    if (type == NULL) {
        return PyArray_DescrFromType(NPY_OBJECT);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("bool"))) {
        return PyArray_DescrFromType(NPY_BOOL);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("int"))) {
        return PyArray_DescrFromType(NPY_LONG);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("float"))) {
        return PyArray_DescrFromType(NPY_DOUBLE);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("complex"))) {
        return PyArray_DescrFromType(NPY_CDOUBLE);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("bytes"))) {
        return PyArray_DescrFromType(NPY_STRING);
    }
    if (PyObject_TypeCheck(type, _molt_numpy_builtin_type_borrowed("str"))) {
        return PyArray_DescrFromType(NPY_UNICODE);
    }
    return PyArray_DescrFromType(NPY_OBJECT);
}
#endif

static inline PyArray_Descr *PyArray_DescrFromObject(PyObject *obj, PyArray_Descr *min_dtype) {
    if (min_dtype != NULL) {
        return PyArray_DescrNew(min_dtype);
    }
    return PyArray_DescrFromScalar(obj);
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT npy_intp PyArray_PyIntAsIntp(PyObject *obj);
#else
static inline npy_intp PyArray_PyIntAsIntp(PyObject *obj);
#endif
static inline PyObject *PyArray_EnsureAnyArray(PyObject *obj);

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyArray_DTypeMeta *_molt_numpy_dtype_from_typenum(int typenum) {
    switch (typenum) {
        case NPY_FLOAT:
        case NPY_DOUBLE:
        case NPY_LONGDOUBLE:
            return PyArray_PyFloatDType;
        case NPY_STRING:
        case NPY_UNICODE:
            return PyArray_StringDType;
        case NPY_BOOL:
            return (PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("bool");
        case NPY_BYTE:
        case NPY_UBYTE:
        case NPY_SHORT:
        case NPY_USHORT:
        case NPY_INT:
        case NPY_UINT:
        case NPY_LONG:
        case NPY_ULONG:
        case NPY_LONGLONG:
        case NPY_ULONGLONG:
            return PyArray_PyLongDType;
        default:
            return (PyArray_DTypeMeta *)_molt_numpy_builtin_type_borrowed("object");
    }
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_BoolConverter(PyObject *obj, npy_bool *out);
#else
static inline int PyArray_BoolConverter(PyObject *obj, npy_bool *out) {
    int truthy = PyObject_IsTrue(obj);
    if (truthy < 0) {
        return 0;
    }
    if (out != NULL) {
        *out = truthy ? (npy_bool)1 : (npy_bool)0;
    }
    return 1;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyArray_DTypeMeta *PyArray_DTypeFromTypeNum(int typenum) {
    return _molt_numpy_dtype_from_typenum(typenum);
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_AsTypeCopyConverter(
    PyObject *obj,
    NPY_ASTYPECOPYMODE *copyflag
) {
    if (copyflag == NULL) {
        PyErr_SetString(PyExc_TypeError, "copyflag output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *copyflag = NPY_AS_TYPE_COPY_IF_NEEDED;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *copyflag = (NPY_ASTYPECOPYMODE)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    if (PyObject_IsTrue(obj) > 0) {
        *copyflag = NPY_AS_TYPE_COPY_ALWAYS;
        return 1;
    }
    if (PyErr_Occurred() != NULL) {
        return 0;
    }
    *copyflag = NPY_AS_TYPE_COPY_IF_NEEDED;
    return 1;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_DeviceConverterOptional(
    PyObject *object,
    NPY_DEVICE *device
) {
    if (device == NULL) {
        PyErr_SetString(PyExc_TypeError, "device output pointer must not be NULL");
        return 0;
    }
    if (object == NULL || object == Py_None) {
        *device = NPY_DEVICE_CPU;
        return 1;
    }
    if (!PyLong_Check(object)) {
        PyErr_SetString(PyExc_TypeError, "device must be an integer or None");
        return 0;
    }
    *device = (NPY_DEVICE)PyLong_AsLongLong(object);
    return PyErr_Occurred() == NULL;
}
#endif

#if MOLT_NUMPY_INTERNAL_BUILD
NPY_NO_EXPORT int PyArray_Converter(PyObject *object, PyObject **address);
NPY_NO_EXPORT int PyArray_OutputConverter(PyObject *object, PyArrayObject **address);
NPY_NO_EXPORT int PyArray_AxisConverter(PyObject *obj, int *axis_out);
NPY_NO_EXPORT int PyArray_OrderConverter(PyObject *obj, NPY_ORDER *order_out);
NPY_NO_EXPORT int PyArray_ClipmodeConverter(PyObject *obj, NPY_CLIPMODE *clipmode_out);
NPY_NO_EXPORT int PyArray_CastingConverter(PyObject *obj, NPY_CASTING *casting_out);
NPY_NO_EXPORT PyObject *PyArray_IntTupleFromIntp(int length, const npy_intp *values);
NPY_NO_EXPORT npy_bool PyArray_CheckStrides(
    int elsize,
    int nd,
    npy_intp numbytes,
    npy_intp offset,
    npy_intp const *dims,
    npy_intp const *newstrides
);
NPY_NO_EXPORT PyObject *PyArray_Reshape(PyArrayObject *self, PyObject *shape);
NPY_NO_EXPORT PyObject *PyArray_Squeeze(PyArrayObject *self);
NPY_NO_EXPORT PyObject *PyArray_SwapAxes(PyArrayObject *ap, int a1, int a2);
NPY_NO_EXPORT PyObject *PyArray_ToList(PyArrayObject *self);
NPY_NO_EXPORT PyObject *PyArray_ToString(PyArrayObject *self, NPY_ORDER order);
NPY_NO_EXPORT int PyArray_ToFile(PyArrayObject *self, FILE *fp, char *sep, char *format);
NPY_NO_EXPORT PyObject *PyArray_Nonzero(PyArrayObject *self);
NPY_NO_EXPORT int PyArray_Sort(PyArrayObject *op, int axis, NPY_SORTKIND kind);
NPY_NO_EXPORT PyObject *PyArray_MultiIterNew(int n, ...);
NPY_NO_EXPORT PyObject *PyArray_IterAllButAxis(PyObject *obj, int *axis);
NPY_NO_EXPORT char *PyArray_Zero(PyArrayObject *arr);
NPY_NO_EXPORT char *PyArray_One(PyArrayObject *arr);
NPY_NO_EXPORT NPY_ARRAYMETHOD_FLAGS NpyIter_GetTransferFlags(NpyIter *iter);
#endif

#if !MOLT_NUMPY_INTERNAL_BUILD
static inline int PyArray_Converter(PyObject *object, PyObject **address) {
    if (address == NULL) {
        PyErr_SetString(PyExc_TypeError, "address output pointer must not be NULL");
        return NPY_FAIL;
    }
    if (object != NULL && PyArray_Check(object)) {
        *address = object;
        Py_INCREF(object);
        return NPY_SUCCEED;
    }
    *address = PyArray_EnsureAnyArray(object);
    return *address != NULL ? NPY_SUCCEED : NPY_FAIL;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_CorrelatemodeConverter(
    PyObject *object,
    NPY_CORRELATEMODE *val
);
#else
static inline int PyArray_CorrelatemodeConverter(
    PyObject *object,
    NPY_CORRELATEMODE *val
) {
    const char *text;
    if (val == NULL) {
        PyErr_SetString(PyExc_TypeError, "mode output pointer must not be NULL");
        return NPY_FAIL;
    }
    if (PyUnicode_Check(object)) {
        text = PyUnicode_AsUTF8(object);
        if (text == NULL) {
            return NPY_FAIL;
        }
        if (strcmp(text, "valid") == 0) {
            *val = NPY_VALID;
            return NPY_SUCCEED;
        }
        if (strcmp(text, "same") == 0) {
            *val = NPY_SAME;
            return NPY_SUCCEED;
        }
        if (strcmp(text, "full") == 0) {
            *val = NPY_FULL;
            return NPY_SUCCEED;
        }
        PyErr_SetString(PyExc_ValueError, "mode must be one of 'valid', 'same', or 'full'");
        return NPY_FAIL;
    }
    if (!PyLong_Check(object)) {
        PyErr_SetString(PyExc_TypeError, "convolve/correlate mode not understood");
        return NPY_FAIL;
    }
    *val = (NPY_CORRELATEMODE)PyLong_AsLongLong(object);
    if (PyErr_Occurred() != NULL) {
        return NPY_FAIL;
    }
    if (*val < NPY_VALID || *val > NPY_FULL) {
        PyErr_SetString(PyExc_ValueError, "integer convolve/correlate mode must be 0, 1, or 2");
        return NPY_FAIL;
    }
    return NPY_SUCCEED;
}
#endif

static inline int PyArray_OrderConverter(PyObject *obj, NPY_ORDER *order_out) {
    const char *text;
    if (order_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "order output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *order_out = NPY_ANYORDER;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *order_out = (NPY_ORDER)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    if (!PyUnicode_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, "order must be a string, integer, or None");
        return 0;
    }
    text = PyUnicode_AsUTF8(obj);
    if (text == NULL || text[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "order string must not be empty");
        return 0;
    }
    switch (text[0]) {
        case 'a':
        case 'A':
            *order_out = NPY_ANYORDER;
            return 1;
        case 'c':
        case 'C':
            *order_out = NPY_CORDER;
            return 1;
        case 'f':
        case 'F':
            *order_out = NPY_FORTRANORDER;
            return 1;
        case 'k':
        case 'K':
            *order_out = NPY_KEEPORDER;
            return 1;
        default:
            PyErr_Format(PyExc_ValueError, "unsupported order specifier: %s", text);
            return 0;
    }
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_SortkindConverter(PyObject *obj, NPY_SORTKIND *sortkind_out);
#else
static inline int PyArray_SortkindConverter(PyObject *obj, NPY_SORTKIND *sortkind_out) {
    if (sortkind_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "sortkind output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *sortkind_out = NPY_QUICKSORT;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *sortkind_out = (NPY_SORTKIND)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    PyErr_SetString(PyExc_TypeError, "sortkind must be an integer or None");
    return 0;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_SearchsideConverter(
    PyObject *obj,
    NPY_SEARCHSIDE *searchside_out
) {
    if (searchside_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "searchside output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *searchside_out = NPY_SEARCHLEFT;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *searchside_out = (NPY_SEARCHSIDE)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    PyErr_SetString(PyExc_TypeError, "searchside must be an integer or None");
    return 0;
}
#endif

static inline int PyArray_ClipmodeConverter(PyObject *obj, NPY_CLIPMODE *clipmode_out) {
    if (clipmode_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "clipmode output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *clipmode_out = NPY_CLIP;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *clipmode_out = (NPY_CLIPMODE)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    PyErr_SetString(PyExc_TypeError, "clipmode must be an integer or None");
    return 0;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_ByteorderConverter(PyObject *obj, char *byteorder_out);
#else
static inline int PyArray_ByteorderConverter(PyObject *obj, char *byteorder_out) {
    const char *text;
    if (byteorder_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "byteorder output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *byteorder_out = '=';
        return 1;
    }
    if (PyLong_Check(obj)) {
        *byteorder_out = (char)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    if (!PyUnicode_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, "byteorder must be a string, integer, or None");
        return 0;
    }
    text = PyUnicode_AsUTF8(obj);
    if (text == NULL || text[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "byteorder string must not be empty");
        return 0;
    }
    *byteorder_out = text[0];
    return 1;
}
#endif

static inline int PyArray_CastingConverter(PyObject *obj, NPY_CASTING *casting_out) {
    const char *text;
    if (casting_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "casting output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *casting_out = NPY_SAFE_CASTING;
        return 1;
    }
    if (PyLong_Check(obj)) {
        *casting_out = (NPY_CASTING)PyLong_AsLongLong(obj);
        return PyErr_Occurred() == NULL;
    }
    if (!PyUnicode_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, "casting must be a string, integer, or None");
        return 0;
    }
    text = PyUnicode_AsUTF8(obj);
    if (text == NULL) {
        return 0;
    }
    if (strcmp(text, "no") == 0) {
        *casting_out = NPY_NO_CASTING;
    }
    else if (strcmp(text, "equiv") == 0) {
        *casting_out = NPY_EQUIV_CASTING;
    }
    else if (strcmp(text, "safe") == 0) {
        *casting_out = NPY_SAFE_CASTING;
    }
    else if (strcmp(text, "same_kind") == 0) {
        *casting_out = NPY_SAME_KIND_CASTING;
    }
    else if (strcmp(text, "unsafe") == 0) {
        *casting_out = NPY_UNSAFE_CASTING;
    }
    else {
        PyErr_Format(PyExc_ValueError, "unsupported casting specifier: %s", text);
        return 0;
    }
    return 1;
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_CastScalarToCtype(
    PyObject *scalar,
    void *ctypeptr,
    PyArray_Descr *outcode
) {
    (void)scalar;
    (void)ctypeptr;
    (void)outcode;
    return _molt_numpy_unavailable_i32("PyArray_CastScalarToCtype");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_BufferConverter(PyObject *obj, PyArray_Chunk *buf);
#else
static inline int PyArray_BufferConverter(PyObject *obj, PyArray_Chunk *buf) {
    (void)obj;
    (void)buf;
    return _molt_numpy_unavailable_i32("PyArray_BufferConverter");
}
#endif

static inline int PyArray_CompareLists(
    const npy_intp *lhs,
    const npy_intp *rhs,
    int length
) {
    int i = 0;
    for (i = 0; i < length; i++) {
        if (lhs[i] < rhs[i]) {
            return -1;
        }
        if (lhs[i] > rhs[i]) {
            return 1;
        }
    }
    return 0;
}

static inline npy_intp PyArray_MultiplyList(const npy_intp *values, int length) {
    npy_intp out = 1;
    int i = 0;
    if (values == NULL || length <= 0) {
        return 0;
    }
    for (i = 0; i < length; i++) {
        out *= values[i];
    }
    return out;
}

static inline int PyArray_EquivTypes(PyArray_Descr *lhs, PyArray_Descr *rhs) {
    if (lhs == NULL || rhs == NULL) {
        return 0;
    }
    return lhs->type_num == rhs->type_num;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_IntpConverter(PyObject *obj, PyArray_Dims *dims_out);
#else
static inline int PyArray_IntpConverter(PyObject *obj, PyArray_Dims *dims_out) {
    (void)obj;
    if (dims_out != NULL) {
        dims_out->ptr = NULL;
        dims_out->len = 0;
    }
    return _molt_numpy_unavailable_i32("PyArray_IntpConverter");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_OptionalIntpConverter(PyObject *obj, PyArray_Dims *dims_out);
#else
static inline int PyArray_OptionalIntpConverter(PyObject *obj, PyArray_Dims *dims_out) {
    if (dims_out != NULL) {
        dims_out->ptr = NULL;
        dims_out->len = -1;
    }
    if (obj == NULL || obj == Py_None) {
        return 1;
    }
    return PyArray_IntpConverter(obj, dims_out);
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_IntpFromPyIntConverter(PyObject *obj, npy_intp *value_out);
#else
static inline int PyArray_IntpFromPyIntConverter(PyObject *obj, npy_intp *value_out) {
    if (value_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "integer output pointer must not be NULL");
        return 0;
    }
    *value_out = (npy_intp)PyLong_AsLongLong(obj);
    return PyErr_Occurred() == NULL;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_IntpFromSequence(PyObject *obj, npy_intp *values, int max_values);
#else
static inline int PyArray_IntpFromSequence(PyObject *obj, npy_intp *values, int max_values) {
    int count;
    int i;
    if (values == NULL || max_values < 0) {
        PyErr_SetString(PyExc_TypeError, "values buffer must be non-NULL and max_values >= 0");
        return -1;
    }
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "sequence must not be NULL");
        return -1;
    }
    count = (int)PySequence_Length(obj);
    if (count < 0) {
        return -1;
    }
    if (count > max_values) {
        PyErr_SetString(PyExc_ValueError, "sequence has more entries than expected");
        return -1;
    }
    for (i = 0; i < count; i++) {
        PyObject *item = PySequence_GetItem(obj, i);
        if (item == NULL) {
            return -1;
        }
        values[i] = (npy_intp)PyLong_AsLongLong(item);
        Py_DECREF(item);
        if (PyErr_Occurred() != NULL) {
            return -1;
        }
    }
    return count;
}
#endif

static inline PyArrayObject *PyArray_NewCopy(PyArrayObject *array_obj, int order) {
    (void)order;
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return array_obj;
}

static inline npy_intp _molt_PyArray_Size(PyObject *obj) {
    if (obj != NULL && PyArray_Check(obj)) {
        return PyArray_SIZE((PyArrayObject *)obj);
    }
    return (npy_intp)PySequence_Size(obj);
}

#define PyArray_Size(obj) _molt_PyArray_Size((PyObject *)(obj))

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArrayObject **PyArray_ConvertToCommonType(PyObject *op, int *retn);
NPY_NO_EXPORT PyObject *PyArray_MultiIterFromObjects(PyObject **mps, int n, int nadd, ...);
#else
static inline PyArrayObject **PyArray_ConvertToCommonType(PyObject *op, int *retn) {
    (void)op;
    if (retn != NULL) {
        *retn = 0;
    }
    PyErr_SetString(PyExc_RuntimeError, "PyArray_ConvertToCommonType is not available");
    return NULL;
}

static inline PyObject *PyArray_MultiIterFromObjects(PyObject **mps, int n, int nadd, ...) {
    (void)mps;
    (void)n;
    (void)nadd;
    PyErr_SetString(PyExc_RuntimeError, "PyArray_MultiIterFromObjects is not available");
    return NULL;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_GetEndianness(void);
#else
static inline int PyArray_GetEndianness(void) {
    return NPY_BYTE_ORDER == NPY_BIG_ENDIAN ? NPY_CPU_BIG : NPY_CPU_LITTLE;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT npy_intp PyArray_PyIntAsIntp(PyObject *obj);
#else
static inline npy_intp PyArray_PyIntAsIntp(PyObject *obj) {
    return (npy_intp)PyLong_AsLongLong(obj);
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_PyIntAsInt(PyObject *obj);
#else
static inline int PyArray_PyIntAsInt(PyObject *obj) {
    return (int)PyLong_AsLongLong(obj);
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_PythonPyIntFromInt(int value) {
    return PyLong_FromLong((long)value);
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_PyIntFromIntp(npy_intp value) {
    return PyLong_FromLongLong((long long)value);
}
#endif

static inline int PyArray_FailUnlessWriteable(PyArrayObject *array_obj, const char *who) {
    (void)who;
    if ((PyArray_FLAGS(array_obj) & NPY_ARRAY_WRITEABLE) == 0) {
        PyErr_SetString(PyExc_RuntimeError, "array is not writeable");
        return -1;
    }
    return 0;
}

static inline PyObject *PyArray_FromAny(
    PyObject *obj,
    PyArray_Descr *descr,
    int min_depth,
    int max_depth,
    int requirements,
    PyObject *context
) {
    (void)descr;
    (void)min_depth;
    (void)max_depth;
    (void)requirements;
    (void)context;
    Py_INCREF(obj);
    return obj;
}

static inline PyObject *PyArray_FromAny_int(
    PyObject *op,
    PyArray_Descr *in_descr,
    PyArray_DTypeMeta *in_DType,
    int min_depth,
    int max_depth,
    int flags,
    PyObject *context,
    int *was_scalar
) {
    (void)in_DType;
    if (was_scalar != NULL) {
        *was_scalar = PyArray_CheckAnyScalar(op) ? 1 : 0;
    }
    return PyArray_FromAny(op, in_descr, min_depth, max_depth, flags, context);
}

static inline PyObject *PyArray_FromInterface(PyObject *obj) {
    return PyArray_EnsureAnyArray(obj);
}

static inline PyObject *PyArray_FromStructInterface(PyObject *obj) {
    return PyArray_EnsureAnyArray(obj);
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_ObjectType(PyObject *obj, int minimum_type);
#else
static inline int PyArray_ObjectType(PyObject *obj, int minimum_type) {
    if (obj == NULL) {
        return minimum_type >= 0 ? minimum_type : NPY_OBJECT;
    }
    if (PyBool_Check(obj)) {
        return NPY_BOOL;
    }
    if (PyLong_Check(obj)) {
        return NPY_LONG;
    }
    if (PyFloat_Check(obj)) {
        return NPY_DOUBLE;
    }
    if (PyComplex_Check(obj)) {
        return NPY_CDOUBLE;
    }
    if (PyBytes_Check(obj)) {
        return NPY_STRING;
    }
    if (PyUnicode_Check(obj)) {
        return NPY_UNICODE;
    }
    return minimum_type >= 0 ? minimum_type : NPY_OBJECT;
}
#endif

static inline int PyArray_SetBaseObject(PyArrayObject *array_obj, PyObject *base) {
    if (array_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "array must not be NULL");
        return -1;
    }
    Py_XINCREF(base);
    Py_XDECREF(((PyArrayObject_fields *)array_obj)->base);
    ((PyArrayObject_fields *)array_obj)->base = base;
    return 0;
}

static inline PyObject *PyArray_EnsureArray(PyObject *obj) {
    if (obj == NULL) {
        return NULL;
    }
    Py_INCREF(obj);
    return obj;
}

static inline PyObject *PyArray_EnsureAnyArray(PyObject *obj) {
    return PyArray_EnsureArray(obj);
}

static inline PyObject *PyArray_FromArray(
    PyArrayObject *array_obj,
    PyArray_Descr *descr,
    int requirements
) {
    (void)descr;
    (void)requirements;
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return (PyObject *)array_obj;
}

static inline PyObject *PyArray_FromArrayAttr_int(
    PyObject *op,
    PyArray_Descr *descr,
    int copy,
    int *was_copied_by__array__
) {
    (void)descr;
    (void)copy;
    if (was_copied_by__array__ != NULL) {
        *was_copied_by__array__ = 0;
    }
    return PyArray_EnsureAnyArray(op);
}

static inline PyObject *PyArray_FromArrayAttr(
    PyObject *op,
    PyArray_Descr *typecode,
    PyObject *context
) {
    (void)typecode;
    (void)context;
    return PyArray_EnsureAnyArray(op);
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyObject *PyArray_Return(PyArrayObject *array_obj);
#else
static inline PyObject *PyArray_Return(PyArrayObject *array_obj) {
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return (PyObject *)array_obj;
}
#endif

static inline PyObject *PyArray_NewFromDescr(
    PyTypeObject *subtype,
    PyArray_Descr *descr,
    int nd,
    const npy_intp *dims,
    const npy_intp *strides,
    void *data,
    int flags,
    PyObject *obj
) {
    (void)subtype;
    (void)descr;
    (void)nd;
    (void)dims;
    (void)strides;
    (void)data;
    (void)flags;
    (void)obj;
    return _molt_numpy_unavailable_obj("PyArray_NewFromDescr");
}

static inline void PyArray_UpdateFlags(PyArrayObject *array_obj, int flagmask) {
    if (array_obj == NULL) {
        return;
    }
    if ((flagmask & NPY_ARRAY_NOTSWAPPED) != 0 && PyArray_DESCR(array_obj) != NULL) {
        if (PyArray_ISNBO(PyArray_DESCR(array_obj)->byteorder)) {
            PyArray_ENABLEFLAGS(array_obj, NPY_ARRAY_NOTSWAPPED);
        }
        else {
            PyArray_CLEARFLAGS(array_obj, NPY_ARRAY_NOTSWAPPED);
        }
    }
    if ((flagmask & NPY_ARRAY_ALIGNED) != 0) {
        PyArray_ENABLEFLAGS(array_obj, NPY_ARRAY_ALIGNED);
    }
}

static inline PyObject *PyArray_New(
    PyTypeObject *subtype,
    int nd,
    const npy_intp *dims,
    int typenum,
    const npy_intp *strides,
    void *data,
    int itemsize,
    int flags,
    PyObject *obj
) {
    (void)subtype;
    (void)nd;
    (void)dims;
    (void)typenum;
    (void)strides;
    (void)data;
    (void)itemsize;
    (void)flags;
    (void)obj;
    return _molt_numpy_unavailable_obj("PyArray_New");
}

static inline PyObject *PyArray_Empty(
    int nd,
    const npy_intp *dims,
    PyArray_Descr *descr,
    int is_fortran
) {
    (void)nd;
    (void)dims;
    (void)descr;
    (void)is_fortran;
    return _molt_numpy_unavailable_obj("PyArray_Empty");
}

static inline PyObject *PyArray_Empty_int(
    int nd,
    npy_intp const *dims,
    PyArray_Descr *descr,
    PyArray_DTypeMeta *dtype,
    int is_f_order
) {
    (void)dtype;
    return PyArray_Empty(nd, dims, descr, is_f_order);
}

#define PyArray_EMPTY(nd, dims, typenum, isfortran) \
    ((PyArrayObject *)PyArray_Empty((nd), (dims), PyArray_DescrFromType((typenum)), (isfortran)))

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_NewFromDescrAndBase(
    PyTypeObject *subtype,
    PyArray_Descr *descr,
    int nd,
    npy_intp *dims,
    npy_intp *strides,
    void *data,
    int flags,
    PyObject *obj,
    PyObject *base
) {
    (void)base;
    return PyArray_NewFromDescr(subtype, descr, nd, dims, strides, data, flags, obj);
}

static inline PyObject *PyArray_NewFromDescr_int(
    PyTypeObject *subtype,
    PyArray_Descr *descr,
    int nd,
    npy_intp *dims,
    npy_intp *strides,
    void *data,
    int flags,
    PyObject *obj,
    PyObject *base,
    int c_order,
    int ensure_array
) {
    (void)c_order;
    (void)ensure_array;
    return PyArray_NewFromDescrAndBase(
        subtype, descr, nd, dims, strides, data, flags, obj, base);
}
#endif

static inline PyObject *PyArray_IterNew(PyObject *obj) {
    (void)obj;
    return _molt_numpy_unavailable_obj("PyArray_IterNew");
}

static inline int PyArray_CopyInto(PyArrayObject *dst, PyArrayObject *src) {
    if (dst == NULL || src == NULL) {
        PyErr_SetString(PyExc_TypeError, "source and destination arrays must not be NULL");
        return -1;
    }
    if (PyArray_SIZE(dst) != PyArray_SIZE(src) || PyArray_ITEMSIZE(dst) != PyArray_ITEMSIZE(src)) {
        PyErr_SetString(PyExc_ValueError, "array copy requires matching size and itemsize");
        return -1;
    }
    if (PyArray_DATA(dst) == NULL || PyArray_DATA(src) == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "array copy requires data pointers");
        return -1;
    }
    (void)memmove(PyArray_DATA(dst), PyArray_DATA(src), (size_t)PyArray_NBYTES(dst));
    return 0;
}

static inline PyObject *PyArray_CastToType(
    PyArrayObject *array_obj,
    PyArray_Descr *descr,
    int is_fortran
) {
    (void)is_fortran;
    return PyArray_FromArray(array_obj, descr, 0);
}

static inline int PyArray_CanCastSafely(int from_type, int to_type) {
    if (from_type == to_type || to_type == NPY_OBJECT) {
        return 1;
    }
    if (from_type >= NPY_BYTE && from_type <= NPY_ULONGLONG &&
        to_type >= NPY_BYTE && to_type <= NPY_ULONGLONG) {
        return from_type <= to_type;
    }
    return 0;
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_AssignArray(
    PyArrayObject *dst,
    PyArrayObject *src,
    PyArrayObject *wheremask,
    NPY_CASTING casting
) {
    (void)dst;
    (void)src;
    (void)wheremask;
    (void)casting;
    return _molt_numpy_unavailable_i32("PyArray_AssignArray");
}
#endif

static inline npy_bool PyArray_CanCastTypeTo(
    PyArray_Descr *from_descr,
    PyArray_Descr *to_descr,
    NPY_CASTING casting
) {
    if (from_descr == NULL || to_descr == NULL) {
        return 0;
    }
    if (casting == NPY_UNSAFE_CASTING) {
        return 1;
    }
    if (from_descr->type_num == to_descr->type_num) {
        return 1;
    }
    if (to_descr->type_num == NPY_OBJECT) {
        return 1;
    }
    return 0;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT npy_bool PyArray_CanCastTo(PyArray_Descr *from_descr, PyArray_Descr *to_descr);
#else
static inline npy_bool PyArray_CanCastTo(PyArray_Descr *from_descr, PyArray_Descr *to_descr) {
    return PyArray_CanCastTypeTo(from_descr, to_descr, NPY_SAFE_CASTING);
}
#endif

static inline npy_bool PyArray_CanCastArrayTo(
    PyArrayObject *array_obj,
    PyArray_Descr *to_descr,
    NPY_CASTING casting
) {
    if (array_obj == NULL) {
        return 0;
    }
    return PyArray_CanCastTypeTo(PyArray_DESCR(array_obj), to_descr, casting);
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_DescrConverter(PyObject *obj, PyArray_Descr **out);
#else
static inline int PyArray_DescrConverter(PyObject *obj, PyArray_Descr **out) {
    if (out == NULL) {
        PyErr_SetString(PyExc_TypeError, "descriptor output pointer must not be NULL");
        return 0;
    }
    if (obj != NULL && PyArray_DescrCheck(obj)) {
        *out = (PyArray_Descr *)obj;
        Py_INCREF(obj);
        return 1;
    }
    *out = PyArray_DescrFromScalar(obj);
    return *out != NULL;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_DescrConverter2(PyObject *obj, PyArray_Descr **out);
#else
static inline int PyArray_DescrConverter2(PyObject *obj, PyArray_Descr **out) {
    return PyArray_DescrConverter(obj, out);
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_DTypeOrDescrConverterRequired(
    PyObject *obj,
    npy_dtype_info *dt_info
) {
    if (dt_info == NULL) {
        PyErr_SetString(PyExc_TypeError, "dtype info output pointer must not be NULL");
        return 0;
    }
    dt_info->dtype = NULL;
    dt_info->descr = NULL;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "dtype or descriptor must not be NULL");
        return 0;
    }
    if (PyArray_DescrCheck(obj)) {
        dt_info->descr = (PyArray_Descr *)obj;
        Py_INCREF(obj);
        dt_info->dtype = PyArray_DTypeFromTypeNum(dt_info->descr->type_num);
        return 1;
    }
    dt_info->descr = PyArray_DescrFromTypeObject(obj);
    if (dt_info->descr == NULL) {
        return 0;
    }
    dt_info->dtype = PyArray_DTypeFromTypeNum(dt_info->descr->type_num);
    return 1;
}

static inline PyArray_DTypeMeta *_molt_numpy_dtypemeta_from_object(
    PyObject *obj,
    int maxdims
) {
    (void)maxdims;
    if (obj == NULL) {
        return PyArray_ObjectDType;
    }
    if (PyBool_Check(obj)) {
        return PyArray_BoolDType;
    }
    if (PyLong_Check(obj)) {
        return PyArray_PyLongDType;
    }
    if (PyFloat_Check(obj)) {
        return PyArray_PyFloatDType;
    }
    if (PyComplex_Check(obj)) {
        return PyArray_PyComplexDType;
    }
    if (PyBytes_Check(obj)) {
        return PyArray_BytesDType;
    }
    if (PyUnicode_Check(obj)) {
        return PyArray_UnicodeDType;
    }
    return PyArray_ObjectDType;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_DTypeFromObjectStringDiscovery(
    PyObject *obj,
    PyArray_Descr *last_dtype,
    int string_type
);
#else
static inline PyArray_Descr *PyArray_DTypeFromObjectStringDiscovery(
    PyObject *obj,
    PyArray_Descr *last_dtype,
    int string_type
) {
    (void)string_type;
    if (last_dtype != NULL) {
        return PyArray_DescrNew(last_dtype);
    }
    return PyArray_DescrFromScalar(obj);
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_DescrNewByteorder(
    PyArray_Descr *descr,
    char neworder
);
#else
static inline PyArray_Descr *PyArray_DescrNewByteorder(
    PyArray_Descr *descr,
    char neworder
) {
    PyArray_Descr *copy;
    if (descr == NULL) {
        return NULL;
    }
    copy = (PyArray_Descr *)PyMem_Malloc(sizeof(PyArray_Descr));
    if (copy == NULL) {
        return NULL;
    }
    *copy = *descr;
    copy->byteorder = neworder;
    return copy;
}
#endif

static inline PyArray_Descr *PyArray_PromoteTypes(
    PyArray_Descr *left,
    PyArray_Descr *right
) {
    if (left == NULL && right == NULL) {
        return NULL;
    }
    if (left == NULL) {
        return PyArray_DescrNew(right);
    }
    if (right == NULL) {
        return PyArray_DescrNew(left);
    }
    if (left->type_num == right->type_num) {
        return PyArray_DescrNew(left);
    }
    if (PyTypeNum_ISOBJECT(left->type_num) || PyTypeNum_ISOBJECT(right->type_num)) {
        return PyArray_DescrFromType(NPY_OBJECT);
    }
    if (left->type_num > right->type_num) {
        return PyArray_DescrNew(left);
    }
    return PyArray_DescrNew(right);
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT NPY_CASTING PyArray_MinCastSafety(
    NPY_CASTING left,
    NPY_CASTING right
);
#else
static inline NPY_CASTING PyArray_MinCastSafety(
    NPY_CASTING left,
    NPY_CASTING right
) {
    return left < right ? left : right;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_IntTupleFromIntp(int length, const npy_intp *values) {
    PyObject *out;
    int i;
    out = PyTuple_New(length);
    if (out == NULL) {
        return NULL;
    }
    for (i = 0; i < length; i++) {
        PyObject *item = PyLong_FromLongLong((long long)values[i]);
        if (item == NULL) {
            Py_DECREF(out);
            return NULL;
        }
        PyTuple_SET_ITEM(out, i, item);
    }
    return out;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_TupleFromItems(
    int length,
    PyObject *const *items,
    int incref_items
) {
    PyObject *out;
    int i;
    out = PyTuple_New(length);
    if (out == NULL) {
        return NULL;
    }
    for (i = 0; i < length; i++) {
        PyObject *item = items != NULL ? items[i] : NULL;
        if (item == NULL) {
            item = Py_None;
        }
        if (incref_items) {
            Py_INCREF(item);
        }
        PyTuple_SET_ITEM(out, i, item);
    }
    return out;
}
#endif

static inline npy_intp PyArray_OverflowMultiplyList(
    const npy_intp *values,
    int length
) {
    return PyArray_MultiplyList(values, length);
}

static inline int PyArray_ResolveWritebackIfCopy(PyArrayObject *array_obj) {
    if (array_obj == NULL) {
        return 0;
    }
    if (PyArray_CHKFLAGS(array_obj, NPY_ARRAY_WRITEBACKIFCOPY)) {
        PyArray_CLEARFLAGS(array_obj, NPY_ARRAY_WRITEBACKIFCOPY);
    }
    return 0;
}

static inline int PyArray_SetWritebackIfCopyBase(
    PyArrayObject *array_obj,
    PyArrayObject *base
) {
    if (array_obj == NULL) {
        return -1;
    }
    ((PyArrayObject_fields *)array_obj)->base = (PyObject *)base;
    if (base != NULL) {
        Py_INCREF((PyObject *)base);
        PyArray_ENABLEFLAGS(array_obj, NPY_ARRAY_WRITEBACKIFCOPY);
    }
    return 0;
}

static inline void PyArray_DiscardWritebackIfCopy(PyArrayObject *array_obj) {
    if (array_obj == NULL) {
        return;
    }
    if (PyArray_CHKFLAGS(array_obj, NPY_ARRAY_WRITEBACKIFCOPY)) {
        PyArray_CLEARFLAGS(array_obj, NPY_ARRAY_WRITEBACKIFCOPY);
        ((PyArrayObject_fields *)array_obj)->base = NULL;
    }
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_Pack(
    PyArray_Descr *descr,
    void *item,
    PyObject *value
);
#else
static inline int PyArray_Pack(
    PyArray_Descr *descr,
    void *item,
    PyObject *value
) {
    (void)descr;
    (void)item;
    (void)value;
    return _molt_numpy_unavailable_i32("PyArray_Pack");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_GETITEM(PyArrayObject *array_obj, const char *item_ptr) {
    (void)array_obj;
    (void)item_ptr;
    return _molt_numpy_unavailable_obj("PyArray_GETITEM");
}

static inline int PyArray_SETITEM(PyArrayObject *array_obj, char *item_ptr, PyObject *value) {
    (void)array_obj;
    (void)item_ptr;
    (void)value;
    return _molt_numpy_unavailable_i32("PyArray_SETITEM");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_GetDTypeTransferFunction(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    PyArray_Descr *src_dtype,
    PyArray_Descr *dst_dtype,
    int move_references,
    PyArrayMethod_StridedLoop *out_loop,
    void **out_transferdata,
    int *out_needs_api
) {
    (void)aligned;
    (void)src_stride;
    (void)dst_stride;
    (void)src_dtype;
    (void)dst_dtype;
    (void)move_references;
    (void)out_loop;
    (void)out_transferdata;
    (void)out_needs_api;
    return _molt_numpy_unavailable_i32("PyArray_GetDTypeTransferFunction");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT npy_intp PyArray_SafeCast(
    PyArray_Descr *from_descr,
    PyArray_Descr *to_descr,
    npy_intp *view_offset,
    NPY_CASTING minimum_safety,
    npy_intp ignore_error
);
#else
static inline npy_intp PyArray_SafeCast(
    PyArray_Descr *from_descr,
    PyArray_Descr *to_descr,
    npy_intp *view_offset,
    NPY_CASTING minimum_safety,
    npy_intp ignore_error
) {
    (void)ignore_error;
    if (view_offset != NULL) {
        *view_offset = 0;
    }
    return PyArray_CanCastTypeTo(from_descr, to_descr, minimum_safety) ? 1 : 0;
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_AdaptDescriptorToArray(
    PyArrayObject *array_obj,
    PyArray_DTypeMeta *dtype,
    PyArray_Descr *descr
);
#else
static inline PyArray_Descr *PyArray_AdaptDescriptorToArray(
    PyArrayObject *array_obj,
    PyArray_DTypeMeta *dtype,
    PyArray_Descr *descr
) {
    (void)dtype;
    if (descr != NULL) {
        return PyArray_DescrNew(descr);
    }
    if (array_obj != NULL && PyArray_DESCR(array_obj) != NULL) {
        return PyArray_DescrNew(PyArray_DESCR(array_obj));
    }
    return PyArray_DescrFromType(NPY_OBJECT);
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int _molt_numpy_extract_dtypemeta_and_descriptor(
    PyObject *obj,
    PyArray_Descr **descr_out,
    PyArray_DTypeMeta **dtype_out
) {
    if (descr_out != NULL) {
        *descr_out = NULL;
    }
    if (dtype_out != NULL) {
        *dtype_out = _molt_numpy_dtypemeta_from_object(obj, 0);
    }
    if (descr_out != NULL) {
        *descr_out = PyArray_DescrFromScalar(obj);
        return *descr_out != NULL;
    }
    return 1;
}

static inline int PyArray_DTypeOrDescrConverterOptional(
    PyObject *obj,
    npy_dtype_info *dt_info
) {
    if (dt_info == NULL) {
        PyErr_SetString(PyExc_TypeError, "dtype info output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        dt_info->dtype = NULL;
        dt_info->descr = NULL;
        return 1;
    }
    return PyArray_DTypeOrDescrConverterRequired(obj, dt_info);
}
#endif

static inline PyObject *PyArray_Newshape(
    PyArrayObject *array_obj,
    PyArray_Dims *newshape,
    NPY_ORDER order
) {
    (void)newshape;
    (void)order;
    return PyArray_FromArray(array_obj, NULL, 0);
}

static inline void PyArray_CreateSortedStridePerm(
    int ndim,
    const npy_intp *strides,
    npy_intp *perm
) {
    int i;
    (void)strides;
    if (perm == NULL) {
        return;
    }
    for (i = 0; i < ndim; i++) {
        perm[i] = i;
    }
}

static inline int PyArray_AxisConverter(PyObject *obj, int *axis_out) {
    if (axis_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "axis output pointer must not be NULL");
        return 0;
    }
    *axis_out = PyArray_PyIntAsInt(obj);
    return PyErr_Occurred() == NULL;
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_CopyConverter(PyObject *obj, NPY_COPYMODE *copy_out) {
    if (copy_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "copy output pointer must not be NULL");
        return 0;
    }
    if (obj == NULL || obj == Py_None) {
        *copy_out = NPY_COPY_IF_NEEDED;
        return 1;
    }
    *copy_out = PyObject_IsTrue(obj) ? NPY_COPY_ALWAYS : NPY_COPY_NEVER;
    return PyErr_Occurred() == NULL;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_DiscoverDTypeAndShape(PyObject *obj, ...) {
    (void)obj;
    return _molt_numpy_unavailable_i32("PyArray_DiscoverDTypeAndShape");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_GetClearFunction(PyArray_Descr *descr, ...) {
    (void)descr;
    return _molt_numpy_unavailable_i32("PyArray_GetClearFunction");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_GetMaskedDTypeTransferFunction(int aligned, ...) {
    (void)aligned;
    return _molt_numpy_unavailable_i32("PyArray_GetMaskedDTypeTransferFunction");
}
#endif

static inline PyObject *PyArray_IterAllButAxis(PyObject *obj, int *axis) {
    (void)axis;
    return PyArray_EnsureArray(obj);
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_PrepareOneRawArrayIter(int ndim, ...) {
    (void)ndim;
    return _molt_numpy_unavailable_i32("PyArray_PrepareOneRawArrayIter");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyObject *PyArray_Resize_int(PyArrayObject *array_obj, ...) {
    (void)array_obj;
    return _molt_numpy_unavailable_obj("PyArray_Resize_int");
}
#endif

static inline PyObject *PyArray_NewLikeArray(
    PyArrayObject *prototype,
    NPY_ORDER order,
    PyArray_Descr *descr,
    int subok
) {
    (void)order;
    (void)subok;
    return PyArray_FromArray(prototype, descr, 0);
}

static inline PyObject *PyArray_View(
    PyArrayObject *array_obj,
    PyArray_Descr *descr,
    PyTypeObject *subtype
) {
    (void)subtype;
    return PyArray_FromArray(array_obj, descr, 0);
}

static inline PyObject *PyArray_Transpose(PyArrayObject *array_obj, PyArray_Dims *permute) {
    (void)permute;
    return PyArray_View(array_obj, NULL, NULL);
}

static inline int PyArray_CopyAnyInto(PyArrayObject *dst, PyArrayObject *src) {
    return PyArray_CopyInto(dst, src);
}

static inline PyObject *PyArray_CheckFromAny(
    PyObject *obj,
    PyArray_Descr *descr,
    int min_depth,
    int max_depth,
    int requirements,
    PyObject *context
) {
    return PyArray_FromAny(obj, descr, min_depth, max_depth, requirements, context);
}

static inline PyObject *PyArray_Conjugate(
    PyArrayObject *array_obj,
    PyArrayObject *out
) {
    (void)out;
    return PyArray_View(array_obj, NULL, NULL);
}

static inline PyObject *PyArray_CheckAxis(
    PyArrayObject *array_obj,
    int *axis,
    int flags
) {
    (void)axis;
    (void)flags;
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return (PyObject *)array_obj;
}

static inline PyArray_Descr *PyArray_ResultType(
    npy_intp narrs,
    PyArrayObject *arrs[],
    npy_intp ndescrs,
    PyArray_Descr *descrs[]
) {
    if (ndescrs > 0 && descrs != NULL && descrs[0] != NULL) {
        return PyArray_DescrNew(descrs[0]);
    }
    if (narrs > 0 && arrs != NULL && arrs[0] != NULL) {
        return PyArray_DescrNew(PyArray_DESCR(arrs[0]));
    }
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyObject *PyArray_Ravel(PyArrayObject *array_obj, NPY_ORDER order) {
    (void)order;
    return PyArray_View(array_obj, NULL, NULL);
}

static inline npy_bool PyArray_EquivTypenums(int left, int right) {
    return left == right;
}

static inline PyObject *PyArray_Flatten(PyArrayObject *a, NPY_ORDER order) {
    return PyArray_Ravel(a, order);
}

static inline PyArray_DTypeMeta *PyArray_CommonDType(
    PyArray_DTypeMeta *dtype1,
    PyArray_DTypeMeta *dtype2
) {
    if (dtype1 == NULL || dtype2 == NULL) {
        PyErr_SetString(PyExc_TypeError, "dtype pointers must not be NULL");
        return NULL;
    }
    if (dtype1 == dtype2) {
        Py_INCREF((PyObject *)dtype1);
        return dtype1;
    }
    return (PyArray_DTypeMeta *)_molt_numpy_unavailable_obj("PyArray_CommonDType");
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_CastToDTypeAndPromoteDescriptors(
    npy_intp ndescr,
    PyArray_Descr *descrs[],
    PyArray_DTypeMeta *DType
);
#else
static inline PyArray_Descr *PyArray_CastToDTypeAndPromoteDescriptors(
    npy_intp ndescr,
    PyArray_Descr *descrs[],
    PyArray_DTypeMeta *DType
) {
    (void)ndescr;
    (void)descrs;
    (void)DType;
    return (PyArray_Descr *)_molt_numpy_unavailable_obj(
        "PyArray_CastToDTypeAndPromoteDescriptors");
}
#endif

static inline PyObject *PyArray_Any(
    PyArrayObject *self,
    int axis,
    PyArrayObject *out
) {
    (void)self;
    (void)axis;
    (void)out;
    return _molt_numpy_unavailable_obj("PyArray_Any");
}

static inline PyObject *PyArray_CumSum(
    PyArrayObject *self,
    int axis,
    int rtype,
    PyArrayObject *out
) {
    (void)self;
    (void)axis;
    (void)rtype;
    (void)out;
    return _molt_numpy_unavailable_obj("PyArray_CumSum");
}

static inline PyObject *PyArray_CumProd(
    PyArrayObject *self,
    int axis,
    int rtype,
    PyArrayObject *out
) {
    (void)self;
    (void)axis;
    (void)rtype;
    (void)out;
    return _molt_numpy_unavailable_obj("PyArray_CumProd");
}

static inline PyObject *PyArray_GenericAccumulateFunction(
    PyArrayObject *m1,
    PyObject *op,
    int axis,
    int rtype,
    PyArrayObject *out
) {
    (void)m1;
    (void)op;
    (void)axis;
    (void)rtype;
    (void)out;
    return _molt_numpy_unavailable_obj("PyArray_GenericAccumulateFunction");
}

static inline PyObject *PyArray_Diagonal(
    PyArrayObject *self,
    int offset,
    int axis1,
    int axis2
) {
    (void)self;
    (void)offset;
    (void)axis1;
    (void)axis2;
    return _molt_numpy_unavailable_obj("PyArray_Diagonal");
}

static inline PyObject *PyArray_CheckFromAny_int(
    PyObject *op,
    PyArray_Descr *in_descr,
    PyArray_DTypeMeta *in_DType,
    int min_depth,
    int max_depth,
    int requirements,
    PyObject *context
) {
    (void)in_DType;
    return PyArray_CheckFromAny(
        op, in_descr, min_depth, max_depth, requirements, context);
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_ClearArray(PyArrayObject *arr) {
    (void)arr;
    return _molt_numpy_unavailable_i32("PyArray_ClearArray");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_ConvertMultiAxis(
    PyObject *axis_in,
    int ndim,
    npy_bool *out_axis_flags
);
#else
static inline int PyArray_ConvertMultiAxis(
    PyObject *axis_in,
    int ndim,
    npy_bool *out_axis_flags
) {
    (void)axis_in;
    (void)ndim;
    (void)out_axis_flags;
    return _molt_numpy_unavailable_i32("PyArray_ConvertMultiAxis");
}
#endif

static inline int PyArray_AssignZero(
    PyArrayObject *dst,
    PyArrayObject *wheremask
) {
    (void)dst;
    (void)wheremask;
    return _molt_numpy_unavailable_i32("PyArray_AssignZero");
}

static inline int PyArray_CopyAsFlat(
    PyArrayObject *dst,
    PyArrayObject *src,
    NPY_ORDER order
) {
    (void)dst;
    (void)src;
    (void)order;
    return _molt_numpy_unavailable_i32("PyArray_CopyAsFlat");
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_AssignFromCache(PyArrayObject *self, void *cache) {
    (void)self;
    (void)cache;
    return _molt_numpy_unavailable_i32("PyArray_AssignFromCache");
}
#endif
static inline double PyArray_GetPriority(PyObject *obj, double default_priority) {
    (void)obj;
    return default_priority;
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyObject *PyArray_Scalar(
    void *data,
    PyArray_Descr *descr,
    PyObject *base
);
#else
static inline PyObject *PyArray_Scalar(
    void *data,
    PyArray_Descr *descr,
    PyObject *base
) {
    (void)data;
    (void)descr;
    if (base != NULL) {
        Py_INCREF(base);
        return base;
    }
    return _molt_numpy_unavailable_obj("PyArray_Scalar");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_ClearBuffer(
    PyArray_Descr *descr,
    void *data,
    npy_intp stride,
    npy_intp length,
    int aligned
) {
    (void)descr;
    (void)stride;
    (void)aligned;
    if (data != NULL && length > 0) {
        memset(data, 0, (size_t)length);
    }
    return 0;
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_AddCastingImplementation_FromSpec(
    PyArrayMethod_Spec *spec,
    int private_api
) {
    (void)spec;
    (void)private_api;
    return _molt_numpy_unavailable_i32("PyArray_AddCastingImplementation_FromSpec");
}

static inline PyBoundArrayMethodObject *PyArrayMethod_FromSpec_int(
    PyArrayMethod_Spec *spec,
    int private_api
) {
    (void)spec;
    (void)private_api;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyArrayMethod_FromSpec_int is not yet implemented in Molt's NumPy compatibility layer");
    return NULL;
}
#endif

/*
 * Internal NumPy build helpers used while compiling NumPy itself. These
 * declarations centralize source compatibility for libmolt without claiming
 * that ndarray kernel/runtime semantics are implemented yet.
 */
static inline int PyArray_CopyObject(PyArrayObject *dst, PyObject *src_object) {
    (void)dst;
    (void)src_object;
    return _molt_numpy_unavailable_i32("PyArray_CopyObject");
}

static inline PyArray_DTypeMeta *PyArray_PromoteDTypeSequence(
    npy_intp n,
    PyArray_DTypeMeta **dtypes
) {
    (void)n;
    (void)dtypes;
    return (PyArray_DTypeMeta *)_molt_numpy_unavailable_obj("PyArray_PromoteDTypeSequence");
}

static inline PyObject *PyArray_GenericBinaryFunction(
    PyObject *lhs,
    PyObject *rhs,
    PyObject *op
) {
    PyObject *args;
    PyObject *result;
    if (op == NULL) {
        PyErr_SetString(PyExc_TypeError, "PyArray_GenericBinaryFunction requires an operator");
        return NULL;
    }
    args = PyTuple_Pack(2, lhs, rhs);
    if (args == NULL) {
        return NULL;
    }
    result = PyObject_CallObject(op, args);
    Py_DECREF(args);
    return result;
}

static inline PyObject *PyArray_GenericReduceFunction(
    PyArrayObject *arr,
    PyObject *op,
    int axis,
    int rtype,
    PyArrayObject *out
) {
    (void)arr;
    (void)op;
    (void)axis;
    (void)rtype;
    (void)out;
    return _molt_numpy_unavailable_obj("PyArray_GenericReduceFunction");
}

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyObject *PyArray_GetCastingImpl(
    PyArray_DTypeMeta *from,
    PyArray_DTypeMeta *to
);
#else
static inline PyObject *PyArray_GetCastingImpl(
    PyArray_DTypeMeta *from,
    PyArray_DTypeMeta *to
) {
    (void)from;
    (void)to;
    return _molt_numpy_unavailable_obj("PyArray_GetCastingImpl");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT int PyArray_CheckLegacyResultType(
    PyArray_Descr **new_result,
    npy_intp narrs,
    PyArrayObject **arr,
    npy_intp ndtypes,
    PyArray_Descr **dtypes
);
#else
static inline int PyArray_CheckLegacyResultType(
    PyArray_Descr **new_result,
    npy_intp narrs,
    PyArrayObject **arr,
    npy_intp ndtypes,
    PyArray_Descr **dtypes
) {
    (void)new_result;
    (void)narrs;
    (void)arr;
    (void)ndtypes;
    (void)dtypes;
    return _molt_numpy_unavailable_i32("PyArray_CheckLegacyResultType");
}
#endif

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE) || defined(NPY_INTERNAL_BUILD)
NPY_NO_EXPORT PyArray_Descr *PyArray_CastDescrToDType(
    PyArray_Descr *descr,
    PyArray_DTypeMeta *given_DType
);
#else
static inline PyArray_Descr *PyArray_CastDescrToDType(
    PyArray_Descr *descr,
    PyArray_DTypeMeta *given_DType
) {
    (void)descr;
    (void)given_DType;
    return (PyArray_Descr *)_molt_numpy_unavailable_obj("PyArray_CastDescrToDType");
}
#endif

static inline int PyArray_AssignRawScalar(
    PyArrayObject *dst,
    PyArray_Descr *src_dtype,
    char *src_data,
    PyArrayObject *wheremask,
    NPY_CASTING casting
) {
    (void)dst;
    (void)src_dtype;
    (void)src_data;
    (void)wheremask;
    (void)casting;
    return _molt_numpy_unavailable_i32("PyArray_AssignRawScalar");
}

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline PyArrayMethod_StridedLoop *PyArray_GetStridedCopyFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    npy_intp itemsize
) {
    (void)aligned;
    (void)src_stride;
    (void)dst_stride;
    (void)itemsize;
    return (PyArrayMethod_StridedLoop *)_molt_numpy_unavailable_obj("PyArray_GetStridedCopyFn");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_CastRawArrays(
    npy_intp count,
    char *src,
    char *dst,
    npy_intp src_stride,
    npy_intp dst_stride,
    PyArray_Descr *src_dtype,
    PyArray_Descr *dst_dtype,
    int move_references
) {
    (void)count;
    (void)src;
    (void)dst;
    (void)src_stride;
    (void)dst_stride;
    (void)src_dtype;
    (void)dst_dtype;
    (void)move_references;
    return _molt_numpy_unavailable_i32("PyArray_CastRawArrays");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_PrepareTwoRawArrayIter(
    int ndim,
    npy_intp const *shape,
    char *dataA,
    npy_intp const *stridesA,
    char *dataB,
    npy_intp const *stridesB,
    int *out_ndim,
    npy_intp *out_shape,
    char **out_dataA,
    npy_intp *out_stridesA,
    char **out_dataB,
    npy_intp *out_stridesB
) {
    (void)ndim;
    (void)shape;
    (void)dataA;
    (void)stridesA;
    (void)dataB;
    (void)stridesB;
    (void)out_ndim;
    (void)out_shape;
    (void)out_dataA;
    (void)out_stridesA;
    (void)out_dataB;
    (void)out_stridesB;
    return _molt_numpy_unavailable_i32("PyArray_PrepareTwoRawArrayIter");
}
#endif

#if !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE) && !defined(NPY_INTERNAL_BUILD)
static inline int PyArray_LookupSpecial(
    PyObject *obj,
    PyObject *name_unicode,
    PyObject **res
) {
    return PyObject_GetOptionalAttr((PyObject *)Py_TYPE(obj), name_unicode, res);
}

static inline int PyArray_LookupSpecial_OnInstance(
    PyObject *obj,
    PyObject *name_unicode,
    PyObject **res
) {
    return PyObject_GetOptionalAttr(obj, name_unicode, res);
}
#endif

#endif  /* !MOLT_NUMPY_INTERNAL_BUILD */

#ifdef __cplusplus
}
#endif

#endif
