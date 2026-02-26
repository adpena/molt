#ifndef MOLT_NUMPY_NDARRAYOBJECT_H
#define MOLT_NUMPY_NDARRAYOBJECT_H

#include <numpy/ndarraytypes.h>

#ifdef __cplusplus
extern "C" {
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

static inline PyTypeObject *_molt_numpy_builtin_type_borrowed(const char *name) {
    return _molt_builtin_type_object_borrowed(name);
}

#define PyArray_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayDescr_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyArrayDescr_TypeFull PyArrayDescr_Type
#define PyArrayDTypeMeta_Type (*_molt_numpy_builtin_type_borrowed("type"))
#define PyGenericArrType_Type PyArray_Type
#define PyArrayArrayConverter_Type PyArray_Type
#define PyArrayFlags_Type PyArray_Type
#define PyArrayFunctionDispatcher_Type PyArray_Type

#define PyArray_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_CheckExact(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_DescrCheck(op) PyObject_TypeCheck((PyObject *)(op), &PyArrayDescr_Type)

#define PyArray_DATA(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_BYTES(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_NDIM(arr) (((PyArrayObject_fields *)(arr))->nd)
#define PyArray_DIMS(arr) (((PyArrayObject_fields *)(arr))->dimensions)
#define PyArray_STRIDES(arr) (((PyArrayObject_fields *)(arr))->strides)
#define PyArray_STRIDE(arr, i) (((PyArrayObject_fields *)(arr))->strides[(i)])
#define PyArray_DIM(arr, i) (((PyArrayObject_fields *)(arr))->dimensions[(i)])
#define PyArray_DESCR(arr) (((PyArrayObject_fields *)(arr))->descr)
#define PyArray_FLAGS(arr) (((PyArrayObject_fields *)(arr))->flags)
#define PyArray_ITEMSIZE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->elsize : 0)
#define PyArray_SIZE(arr) _molt_pyarray_size((PyArrayObject *)(arr))
#define PyArray_NBYTES(arr) ((npy_intp)(PyArray_SIZE(arr) * (npy_intp)PyArray_ITEMSIZE(arr)))
#define PyArray_TYPE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->type_num : NPY_OBJECT)
#define PyArray_IS_C_CONTIGUOUS(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_C_CONTIGUOUS) != 0)
#define PyArray_ISFORTRAN(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_F_CONTIGUOUS) != 0)
#define PyArray_ISNBO(byteorder) ((byteorder) == '=' || (byteorder) == '|')
#define PyArray_ISDATETIME(arr) PyTypeNum_ISDATETIME(PyArray_TYPE(arr))

#define PyDataType_FLAGCHK(descr, flag) (((descr) != NULL) && (((descr)->flags & (flag)) == (flag)))
#define PyDataType_REFCHK(descr) PyDataType_FLAGCHK((descr), NPY_ITEM_REFCOUNT)
#define PyDataType_HASFIELDS(descr) PyDataType_FLAGCHK((descr), NPY_LIST_PICKLE)
#define PyDataType_HASSUBARRAY(descr) 0
#define PyDataType_C_METADATA(descr) ((PyArray_DatetimeMetaData *)NULL)

#define PyArray_malloc PyMem_Malloc
#define PyArray_free PyMem_Free

#define PyArray_FROM_O(obj) ((PyArrayObject *)(obj))
#define PyArray_EMPTY(nd, dims, typenum, isfortran) \
    ((PyArrayObject *)_molt_numpy_unavailable_obj("PyArray_EMPTY"))
#define PyArray_MultiIter_DATA(iter, i) ((void)(iter), (void)(i), (char *)NULL)
#define PyArray_MultiIter_NEXT(iter) ((void)(iter))
#define PyArray_ITER_NEXT(iter) ((void)(iter))

static inline int PyArray_IsScalar(PyObject *obj, PyTypeObject *cls) {
    if (obj == NULL || cls == NULL) {
        return 0;
    }
    return PyObject_TypeCheck(obj, cls);
}

#define PyArray_CheckScalar(obj) PyArray_IsScalar((obj), &PyGenericArrType_Type)

static inline PyArray_ArrFuncs *PyDataType_GetArrFuncs(const PyArray_Descr *descr) {
    (void)descr;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyDataType_GetArrFuncs is not yet implemented in Molt's NumPy compatibility layer");
    return NULL;
}

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

static inline PyArray_Descr *PyArray_DescrNewFromType(int typenum) {
    return PyArray_DescrFromType(typenum);
}

static inline PyArray_Descr *PyArray_DescrFromScalar(PyObject *obj) {
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

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

static inline PyObject *PyArray_NewFromDescr(
    PyTypeObject *subtype,
    PyArray_Descr *descr,
    int nd,
    npy_intp *dims,
    npy_intp *strides,
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

static inline PyObject *PyArray_IterNew(PyObject *obj) {
    (void)obj;
    return _molt_numpy_unavailable_obj("PyArray_IterNew");
}

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

static inline PyArray_DTypeMeta *PyArrayDTypeMeta_CommonDType(
    PyArray_DTypeMeta *left,
    PyArray_DTypeMeta *right
) {
    (void)left;
    (void)right;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyArrayDTypeMeta_CommonDType is not yet implemented in Molt's NumPy compatibility layer");
    return NULL;
}

static inline PyArray_Descr *PyArrayDTypeMeta_CommonInstance(
    PyArray_DTypeMeta *dtype,
    PyObject *obj
) {
    (void)dtype;
    (void)obj;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyArrayDTypeMeta_CommonInstance is not yet implemented in Molt's NumPy compatibility layer");
    return NULL;
}

static inline PyArray_Descr *PyArrayDTypeMeta_DefaultDescriptor(PyArray_DTypeMeta *dtype) {
    (void)dtype;
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyArray_Descr *PyArrayDTypeMeta_DiscoverDescrFromPyobject(
    PyArray_DTypeMeta *dtype,
    PyObject *obj
) {
    (void)dtype;
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyArray_Descr *PyArrayDTypeMeta_EnsureCanonical(PyArray_Descr *descr) {
    if (descr == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)descr);
    return descr;
}

static inline PyObject *PyArrayDTypeMeta_GetConstant(
    PyArray_DTypeMeta *dtype,
    int constant
) {
    (void)dtype;
    (void)constant;
    return _molt_numpy_unavailable_obj("PyArrayDTypeMeta_GetConstant");
}

static inline PyObject *PyArrayDTypeMeta_GetItem(
    PyArray_DTypeMeta *dtype,
    const char *data
) {
    (void)dtype;
    (void)data;
    return _molt_numpy_unavailable_obj("PyArrayDTypeMeta_GetItem");
}

static inline int PyArrayDTypeMeta_IsKnownScalarType(
    PyArray_DTypeMeta *dtype,
    PyTypeObject *type
) {
    (void)dtype;
    (void)type;
    return 0;
}

static inline int PyArrayDTypeMeta_SetItem(
    PyArray_DTypeMeta *dtype,
    PyObject *obj,
    char *data
) {
    (void)dtype;
    (void)obj;
    (void)data;
    return _molt_numpy_unavailable_i32("PyArrayDTypeMeta_SetItem");
}

static inline PyArray_Descr *PyArrayDTypeMeta_FinalizeDescriptor(
    PyArray_DTypeMeta *dtype,
    PyArray_Descr *descr
) {
    (void)dtype;
    if (descr == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)descr);
    return descr;
}

#ifdef __cplusplus
}
#endif

#endif
