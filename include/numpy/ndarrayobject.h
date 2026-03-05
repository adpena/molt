#ifndef MOLT_NUMPY_NDARRAYOBJECT_H
#define MOLT_NUMPY_NDARRAYOBJECT_H

#include <numpy/ndarraytypes.h>
#include <numpy/dtype_api.h>

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
#define PyArrayMethod_Type (*_molt_numpy_builtin_type_borrowed("object"))
#define PyGenericArrType_Type PyArray_Type
#define PyArrayArrayConverter_Type PyArray_Type
#define PyArrayFlags_Type PyArray_Type
#define PyArrayFunctionDispatcher_Type PyArray_Type
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
#define PyArray_ISINTEGER(arr) PyTypeNum_ISINTEGER(PyArray_TYPE(arr))
#define PyArray_ISCOMPLEX(arr) PyTypeNum_ISCOMPLEX(PyArray_TYPE(arr))
#define PyArray_ISOBJECT(arr) PyTypeNum_ISOBJECT(PyArray_TYPE(arr))
#define PyArray_ISWRITEABLE(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_WRITEABLE)
#define PyArray_ISONESEGMENT(arr) (PyArray_ISCONTIGUOUS(arr) || PyArray_ISFORTRAN(arr))
#define PyArray_HANDLER(arr) ((PyObject *)((PyArrayObject_fields *)(arr))->mem_handler)
#define PyArray_ISNBO(byteorder) ((byteorder) == '=' || (byteorder) == '|')
#define PyArray_ISDATETIME(arr) PyTypeNum_ISDATETIME(PyArray_TYPE(arr))
#define PyArray_ENABLEFLAGS(arr, mask) (((PyArrayObject_fields *)(arr))->flags |= (mask))
#define PyArray_CLEARFLAGS(arr, mask) (((PyArrayObject_fields *)(arr))->flags &= ~(mask))

#define PyDataType_FLAGCHK(descr, flag) (((descr) != NULL) && (((descr)->flags & (flag)) == (flag)))
#define PyDataType_REFCHK(descr) PyDataType_FLAGCHK((descr), NPY_ITEM_REFCOUNT)
#define PyDataType_ISLEGACY(descr) ((descr) != NULL && (descr)->type_num >= 0)
#define PyDataType_NAMES(descr) ((descr) != NULL ? (descr)->names : NULL)
#define PyDataType_FIELDS(descr) ((descr) != NULL ? (descr)->fields : NULL)
#define PyDataType_HASFIELDS(descr) (PyDataType_NAMES((descr)) != NULL || PyDataType_FIELDS((descr)) != NULL)
#define PyDataType_HASSUBARRAY(descr) 0
#define PyDataType_ISUNSIZED(descr) ((descr) != NULL && (descr)->elsize == 0 && !PyDataType_HASFIELDS(descr))
#define PyDataType_C_METADATA(descr) ((PyArray_DatetimeMetaData *)NULL)

#define PyArray_malloc PyMem_Malloc
#define PyArray_free PyMem_Free
#define PyDataMem_NEW(size) PyMem_Malloc((size))
#define PyDataMem_UserNEW(handler, size) ((void)(handler), PyMem_Malloc((size)))
#define PyDataMem_UserFREE(handler, ptr, size) do { (void)(handler); (void)(size); PyMem_Free((ptr)); } while (0)
#define PyArray_MAX(a, b) ((a) > (b) ? (a) : (b))
#define PyArray_FROM_OF(obj, flags) ((void)(flags), PyArray_FromAny((obj), NULL, 0, 0, (flags), NULL))
#define PyArray_DESCR_REPLACE(descr) do { \
    PyArray_Descr *_molt_new_descr = PyArray_DescrNew((descr)); \
    if ((descr) != NULL) { \
        PyMem_Free((descr)); \
    } \
    (descr) = _molt_new_descr; \
} while (0)

#define PyArray_FROM_O(obj) ((PyArrayObject *)(obj))
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

static inline PyArray_Descr *PyArray_DescrFromScalar(PyObject *obj) {
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

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

static inline PyArray_DTypeMeta *PyArray_DTypeFromTypeNum(int typenum) {
    return _molt_numpy_dtype_from_typenum(typenum);
}

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

static inline int PyArray_IntpConverter(PyObject *obj, PyArray_Dims *dims_out) {
    (void)obj;
    if (dims_out != NULL) {
        dims_out->ptr = NULL;
        dims_out->len = 0;
    }
    return _molt_numpy_unavailable_i32("PyArray_IntpConverter");
}

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

static inline int PyArray_IntpFromPyIntConverter(PyObject *obj, npy_intp *value_out) {
    if (value_out == NULL) {
        PyErr_SetString(PyExc_TypeError, "integer output pointer must not be NULL");
        return 0;
    }
    *value_out = (npy_intp)PyLong_AsLongLong(obj);
    return PyErr_Occurred() == NULL;
}

static inline PyArrayObject *PyArray_NewCopy(PyArrayObject *array_obj, int order) {
    (void)order;
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return array_obj;
}

static inline npy_intp PyArray_PyIntAsIntp(PyObject *obj) {
    return (npy_intp)PyLong_AsLongLong(obj);
}

static inline int PyArray_PyIntAsInt(PyObject *obj) {
    return (int)PyLong_AsLongLong(obj);
}

static inline PyObject *PyArray_PythonPyIntFromInt(int value) {
    return PyLong_FromLong((long)value);
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

static inline PyObject *PyArray_Return(PyArrayObject *array_obj) {
    if (array_obj == NULL) {
        return NULL;
    }
    Py_INCREF((PyObject *)array_obj);
    return (PyObject *)array_obj;
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

#define PyArray_EMPTY(nd, dims, typenum, isfortran) \
    ((PyArrayObject *)PyArray_Empty((nd), (dims), PyArray_DescrFromType((typenum)), (isfortran)))

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

static inline int PyArray_CopyInto(PyArrayObject *dst, PyArrayObject *src) {
    (void)dst;
    (void)src;
    return _molt_numpy_unavailable_i32("PyArray_CopyInto");
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

static inline int PyArray_AssignArray(
    PyArrayObject *dst,
    PyArrayObject *src,
    PyObject *wheremask,
    int casting
) {
    (void)dst;
    (void)src;
    (void)wheremask;
    (void)casting;
    return _molt_numpy_unavailable_i32("PyArray_AssignArray");
}

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

static inline int PyArray_DescrConverter2(PyObject *obj, PyArray_Descr **out) {
    return PyArray_DescrConverter(obj, out);
}

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

static inline NPY_CASTING PyArray_MinCastSafety(
    NPY_CASTING left,
    NPY_CASTING right
) {
    return left < right ? left : right;
}

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

static inline PyObject *PyArray_TupleFromItems(
    int length,
    PyObject **items,
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

static inline PyArray_DTypeMeta *PyArray_DTypeFromObject(PyObject *obj, int maxdims) {
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

static inline int PyArray_ExtractDTypeAndDescriptor(
    PyObject *obj,
    PyArray_Descr **descr_out,
    PyArray_DTypeMeta **dtype_out
) {
    if (descr_out != NULL) {
        *descr_out = NULL;
    }
    if (dtype_out != NULL) {
        *dtype_out = PyArray_DTypeFromObject(obj, 0);
    }
    if (descr_out != NULL) {
        *descr_out = PyArray_DescrFromScalar(obj);
        return *descr_out != NULL;
    }
    return 1;
}

static inline int PyArray_DTypeOrDescrConverterOptional(
    PyObject *obj,
    PyArray_DTypeMeta **dtype_out,
    PyArray_Descr **descr_out
) {
    if (obj == NULL || obj == Py_None) {
        if (dtype_out != NULL) {
            *dtype_out = NULL;
        }
        if (descr_out != NULL) {
            *descr_out = NULL;
        }
        return 1;
    }
    return PyArray_ExtractDTypeAndDescriptor(obj, descr_out, dtype_out);
}

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

static inline int PyArray_DiscoverDTypeAndShape(PyObject *obj, ...) {
    (void)obj;
    return _molt_numpy_unavailable_i32("PyArray_DiscoverDTypeAndShape");
}

static inline int PyArray_GetClearFunction(PyArray_Descr *descr, ...) {
    (void)descr;
    return _molt_numpy_unavailable_i32("PyArray_GetClearFunction");
}

static inline int PyArray_GetMaskedDTypeTransferFunction(int aligned, ...) {
    (void)aligned;
    return _molt_numpy_unavailable_i32("PyArray_GetMaskedDTypeTransferFunction");
}

static inline PyObject *PyArray_IterAllButAxis(PyObject *obj, int *axis) {
    (void)axis;
    return PyArray_EnsureArray(obj);
}

static inline int PyArray_PrepareOneRawArrayIter(int ndim, ...) {
    (void)ndim;
    return _molt_numpy_unavailable_i32("PyArray_PrepareOneRawArrayIter");
}

static inline PyObject *PyArray_Resize_int(PyArrayObject *array_obj, ...) {
    (void)array_obj;
    return _molt_numpy_unavailable_obj("PyArray_Resize_int");
}

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

static inline double PyArray_GetPriority(PyObject *obj, double default_priority) {
    (void)obj;
    return default_priority;
}

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
