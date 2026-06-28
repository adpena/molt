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

static inline void _molt_numpy_iter_next(PyArrayIterObject *iter);
static inline void _molt_numpy_iter_reset(PyArrayIterObject *iter);

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
#define PyArray_ArrayFunctionDispatcherType PyArrayFunctionDispatcher_Type
#define PyArrayIdentityHash_Type PyArray_Type
#define PyBoundArrayMethod_Type PyArray_Type
#define PyArrayMapIter_Type PyArray_Type
#define PyArrayNeighborhoodIter_Type PyArray_Type
#define PyBoolArrType_Type PyArray_Type
#define PyByteArrType_Type PyArray_Type
#define PyUByteArrType_Type PyArray_Type
#define PyShortArrType_Type PyArray_Type
#define PyUShortArrType_Type PyArray_Type
#define PyIntArrType_Type PyArray_Type
#define PyUIntArrType_Type PyArray_Type
#define PyLongArrType_Type PyArray_Type
#define PyULongArrType_Type PyArray_Type
#define PyLongLongArrType_Type PyArray_Type
#define PyULongLongArrType_Type PyArray_Type
#define PyFloatArrType_Type PyArray_Type
#define PyDoubleArrType_Type PyArray_Type
#define PyLongDoubleArrType_Type PyArray_Type
#define PyCFloatArrType_Type PyArray_Type
#define PyCDoubleArrType_Type PyArray_Type
#define PyCLongDoubleArrType_Type PyArray_Type
#define PyObjectArrType_Type PyArray_Type
#define PyStringArrType_Type PyArray_Type
#define PyUnicodeArrType_Type PyArray_Type
#define PyVoidArrType_Type PyArray_Type
#define PyDatetimeArrType_Type PyArray_Type
#define PyTimedeltaArrType_Type PyArray_Type
#define PyHalfArrType_Type PyArray_Type
#define PyIntUArrType_Type PyArray_Type
#define PyIntegerArrType_Type PyArray_Type
#define PySignedIntegerArrType_Type PyArray_Type
#define PyUnsignedIntegerArrType_Type PyArray_Type
#define PyFloatingArrType_Type PyArray_Type
#define PyComplexFloatingArrType_Type PyArray_Type
#define PyInexactArrType_Type PyArray_Type
#define PyNumberArrType_Type PyArray_Type
#define PyFlexibleArrType_Type PyArray_Type
#define PyCharacterArrType_Type PyArray_Type
#define PyGenericArrType_Type PyArray_Type

#define PyArray_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_CheckExact(op) PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)
#define PyArray_DescrCheck(op) PyObject_TypeCheck((PyObject *)(op), &PyArrayDescr_Type)

#define PyArray_DATA(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_BYTES(arr) (((PyArrayObject_fields *)(arr))->data)
#define PyArray_NDIM(arr) (((PyArrayObject_fields *)(arr))->nd)
#define PyArray_DIMS(arr) (((PyArrayObject_fields *)(arr))->dimensions)
#define PyArray_SHAPE(arr) PyArray_DIMS(arr)
#define PyArray_STRIDES(arr) (((PyArrayObject_fields *)(arr))->strides)
#define PyArray_STRIDE(arr, i) (((PyArrayObject_fields *)(arr))->strides[(i)])
#define PyArray_DIM(arr, i) (((PyArrayObject_fields *)(arr))->dimensions[(i)])
#define PyArray_DESCR(arr) (((PyArrayObject_fields *)(arr))->descr)
#define PyArray_DTYPE(arr) PyArray_DESCR(arr)
#define PyArray_BASE(arr) (((PyArrayObject_fields *)(arr))->base)
#define PyArray_FLAGS(arr) (((PyArrayObject_fields *)(arr))->flags)
#define PyArray_ITEMSIZE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->elsize : 0)
#define PyArray_SIZE(arr) _molt_pyarray_size((PyArrayObject *)(arr))
#define PyArray_NBYTES(arr) ((npy_intp)(PyArray_SIZE(arr) * (npy_intp)PyArray_ITEMSIZE(arr)))
#define PyArray_TYPE(arr) ((PyArray_DESCR(arr) != NULL) ? PyArray_DESCR(arr)->type_num : NPY_OBJECT)
#define PyArray_CHKFLAGS(arr, flags) (((PyArray_FLAGS(arr)) & (flags)) == (flags))
#define PyArray_ENABLEFLAGS(arr, flags) ((void)(((PyArrayObject_fields *)(arr))->flags |= (flags)))
#define PyArray_CLEARFLAGS(arr, flags) ((void)(((PyArrayObject_fields *)(arr))->flags &= ~(flags)))
#define PyArray_IS_C_CONTIGUOUS(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_C_CONTIGUOUS) != 0)
#define PyArray_ISCONTIGUOUS(arr) PyArray_IS_C_CONTIGUOUS(arr)
#define PyArray_ISCARRAY(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE)
#define PyArray_ISCARRAY_RO(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define PyArray_ISFORTRAN(arr) (((PyArray_FLAGS(arr)) & NPY_ARRAY_F_CONTIGUOUS) != 0)
#define PyArray_IS_F_CONTIGUOUS(arr) PyArray_ISFORTRAN(arr)
#define PyArray_ISFARRAY(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE)
#define PyArray_ISFARRAY_RO(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_F_CONTIGUOUS | NPY_ARRAY_ALIGNED)
#define PyArray_ISALIGNED(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_ALIGNED)
#define PyArray_ISWRITEABLE(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_WRITEABLE)
#define PyArray_ISBYTESWAPPED(arr) (!PyArray_ISNBO(PyArray_DESCR(arr)->byteorder))
#define PyArray_ISNOTSWAPPED(arr) PyArray_ISNBO(PyArray_DESCR(arr)->byteorder)
#define PyArray_ISBEHAVED(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE)
#define PyArray_ISBEHAVED_RO(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_ALIGNED)
#define PyArray_ISNBO(byteorder) ((byteorder) == '=' || (byteorder) == '|')
#define PyArray_ISDATETIME(arr) PyTypeNum_ISDATETIME(PyArray_TYPE(arr))
#define PyArray_ISINTEGER(arr) PyTypeNum_ISINTEGER(PyArray_TYPE(arr))
#define PyArray_HANDLER(arr) (((PyArrayObject_fields *)(arr))->mem_handler)
#define PyArray_GETPTR1(arr, i) (PyArray_BYTES(arr) + ((npy_intp)(i)) * PyArray_STRIDE((arr), 0))
#define PyArray_GETPTR2(arr, i, j) \
    (PyArray_BYTES(arr) + ((npy_intp)(i)) * PyArray_STRIDE((arr), 0) + ((npy_intp)(j)) * PyArray_STRIDE((arr), 1))
#define PyArray_GETPTR3(arr, i, j, k) \
    (PyArray_GETPTR2((arr), (i), (j)) + ((npy_intp)(k)) * PyArray_STRIDE((arr), 2))
#define PyArray_GETPTR4(arr, i, j, k, l) \
    (PyArray_GETPTR3((arr), (i), (j), (k)) + ((npy_intp)(l)) * PyArray_STRIDE((arr), 3))

#define PyDataType_FLAGCHK(descr, flag) (((descr) != NULL) && (((descr)->flags & (flag)) == (flag)))
#define PyDataType_REFCHK(descr) PyDataType_FLAGCHK((descr), NPY_ITEM_REFCOUNT)
#define PyDataType_HASFIELDS(descr) PyDataType_FLAGCHK((descr), NPY_LIST_PICKLE)
#define PyDataType_HASSUBARRAY(descr) 0
#define PyDataType_C_METADATA(descr) ((PyArray_DatetimeMetaData *)NULL)
#define PyDataType_ELSIZE(descr) ((npy_intp)(((PyArray_Descr *)(descr))->elsize))
#define PyDataType_SET_ELSIZE(descr, size) ((void)(((PyArray_Descr *)(descr))->elsize = (int)(size)))
#define PyDataType_FLAGS(descr) ((npy_uint64)(unsigned char)(((PyArray_Descr *)(descr))->flags))
#define PyDataType_ALIGNMENT(descr) ((npy_intp)(((PyArray_Descr *)(descr))->alignment))
#define PyDataType_METADATA(descr) ((void)(descr), (PyObject *)NULL)
#define PyDataType_SUBARRAY(descr) ((void)(descr), (PyArray_ArrayDescr *)NULL)
#define PyDataType_NAMES(descr) ((void)(descr), (PyObject *)NULL)
#define PyDataType_FIELDS(descr) ((void)(descr), Py_None)
#define PyDataType_ISNOTSWAPPED(descr) PyArray_ISNBO(((PyArray_Descr *)(descr))->byteorder)
#define PyDataType_ISBYTESWAPPED(descr) (!PyDataType_ISNOTSWAPPED(descr))

#define PyArray_malloc PyMem_Malloc
#define PyArray_free PyMem_Free
#define PyDataMem_NEW(size) PyMem_Malloc((size))
#define PyDataMem_RENEW(ptr, size) PyMem_Realloc((ptr), (size))
#define PyDataMem_FREE(ptr) PyMem_Free((ptr))
#define PyDimMem_FREE(ptr) PyMem_Free((ptr))

#define NPY_MEMALIGN 16

static inline void *PyArray_realloc_aligned(void *ptr, size_t size) {
    void *allocation;
    void **aligned;
    void *base;
    size_t old_offset;
    size_t offset = (size_t)NPY_MEMALIGN - 1 + sizeof(void *);
    if (ptr != NULL) {
        base = *(((void **)ptr) - 1);
        allocation = PyMem_Realloc(base, size + offset);
        if (allocation == NULL) {
            return NULL;
        }
        if (allocation == base) {
            return ptr;
        }
        aligned = (void **)(((Py_uintptr_t)allocation + offset) & ~((Py_uintptr_t)NPY_MEMALIGN - 1));
        old_offset = (size_t)((Py_uintptr_t)ptr - (Py_uintptr_t)base);
        memmove((void *)aligned, ((char *)allocation) + old_offset, size);
    } else {
        allocation = PyMem_Malloc(size + offset);
        if (allocation == NULL) {
            return NULL;
        }
        aligned = (void **)(((Py_uintptr_t)allocation + offset) & ~((Py_uintptr_t)NPY_MEMALIGN - 1));
    }
    *(aligned - 1) = allocation;
    return (void *)aligned;
}

static inline void *PyArray_malloc_aligned(size_t size) {
    return PyArray_realloc_aligned(NULL, size);
}

static inline void *PyArray_calloc_aligned(size_t count, size_t size) {
    void *ptr = PyArray_realloc_aligned(NULL, count * size);
    if (ptr != NULL) {
        memset(ptr, 0, count * size);
    }
    return ptr;
}

static inline void PyArray_free_aligned(void *ptr) {
    if (ptr != NULL) {
        PyMem_Free(*(((void **)ptr) - 1));
    }
}

#define PyArray_FROM_O(obj) ((PyArrayObject *)(obj))
#define PyArray_FROMANY(obj, typenum, min_depth, max_depth, requirements) \
    PyArray_FromAny((PyObject *)(obj), PyArray_DescrFromType((typenum)), (min_depth), (max_depth), (requirements), NULL)
#define PyArray_FROM_OTF(obj, typenum, requirements) \
    PyArray_FROMANY((obj), (typenum), 0, 0, (requirements))
#define PyArray_FROM_OT(obj, typenum) PyArray_FROM_OTF((obj), (typenum), 0)
#define PyArray_FROM_OF(obj, requirements) PyArray_FromAny((PyObject *)(obj), NULL, 0, 0, (requirements), NULL)

static inline void _molt_numpy_multi_iter_next(PyArrayMultiIterObject *multi) {
    int i;
    if (multi == NULL) {
        return;
    }
    multi->index++;
    for (i = 0; i < multi->numiter; i++) {
        _molt_numpy_iter_next(multi->iters[i]);
    }
}

static inline void _molt_numpy_multi_iter_reset(PyArrayMultiIterObject *multi) {
    int i;
    if (multi == NULL) {
        return;
    }
    multi->index = 0;
    for (i = 0; i < multi->numiter; i++) {
        _molt_numpy_iter_reset(multi->iters[i]);
    }
}

#define PyArray_MultiIter_DATA(iter, i) ((void *)(((PyArrayMultiIterObject *)(iter))->iters[(i)]->dataptr))
#define PyArray_MultiIter_RESET(iter) _molt_numpy_multi_iter_reset((PyArrayMultiIterObject *)(iter))
#define PyArray_MultiIter_NEXT(iter) _molt_numpy_multi_iter_next((PyArrayMultiIterObject *)(iter))
#define PyArray_MultiIter_NEXTi(iter, i) _molt_numpy_iter_next(((PyArrayMultiIterObject *)(iter))->iters[(i)])
#define PyArray_MultiIter_NOTDONE(iter) (((PyArrayMultiIterObject *)(iter))->index < ((PyArrayMultiIterObject *)(iter))->size)
#define PyArray_MultiIter_NUMITER(iter) (((PyArrayMultiIterObject *)(iter))->numiter)
#define PyArray_MultiIter_SIZE(iter) (((PyArrayMultiIterObject *)(iter))->size)
#define PyArray_MultiIter_INDEX(iter) (((PyArrayMultiIterObject *)(iter))->index)
#define PyArray_MultiIter_NDIM(iter) (((PyArrayMultiIterObject *)(iter))->nd)
#define PyArray_MultiIter_DIMS(iter) (((PyArrayMultiIterObject *)(iter))->dimensions)
#define PyArray_MultiIter_ITERS(iter) ((void **)(((PyArrayMultiIterObject *)(iter))->iters))
#define PyArray_ITER_DATA(iter) (((PyArrayIterObject *)(iter))->dataptr)
#define PyArray_ITER_NOTDONE(iter) (((PyArrayIterObject *)(iter))->index < ((PyArrayIterObject *)(iter))->size)
#define PyArray_ITER_RESET(iter) _molt_numpy_iter_reset((PyArrayIterObject *)(iter))
#define PyArray_ITER_NEXT(iter) _molt_numpy_iter_next((PyArrayIterObject *)(iter))
#define PyArray_FORTRANIF(arr) (PyArray_ISFORTRAN(arr) ? NPY_ARRAY_F_CONTIGUOUS : 0)

#define PyArray_TRIVIALLY_ITERABLE_OP_NOREAD 0
#define PyArray_TRIVIALLY_ITERABLE_OP_READ 1
#define PyArray_TRIVIALLY_ITERABLE(arr) \
    (PyArray_NDIM(arr) <= 1 || PyArray_IS_C_CONTIGUOUS(arr) || PyArray_IS_F_CONTIGUOUS(arr))
#define PyArray_TRIVIAL_PAIR_ITERATION_STRIDE(size, arr) \
    ((size) == 1 ? 0 : (PyArray_NDIM(arr) == 1 ? PyArray_STRIDE((arr), 0) : (npy_intp)PyArray_ITEMSIZE(arr)))
#define PyArray_EQUIVALENTLY_ITERABLE_BASE(arr1, arr2) \
    (PyArray_NDIM(arr1) == PyArray_NDIM(arr2) \
        && PyArray_CompareLists(PyArray_DIMS(arr1), PyArray_DIMS(arr2), PyArray_NDIM(arr1)) == 0 \
        && ((PyArray_FLAGS(arr1) & (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_F_CONTIGUOUS)) \
            & (PyArray_FLAGS(arr2) & (NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_F_CONTIGUOUS))))
#define PyArray_EQUIVALENTLY_ITERABLE(arr1, arr2, arr1_read, arr2_read) \
    (PyArray_EQUIVALENTLY_ITERABLE_BASE((arr1), (arr2)) \
        && PyArray_EQUIVALENTLY_ITERABLE_OVERLAP_OK((arr1), (arr2), (arr1_read), (arr2_read)))

static inline int PyArray_IsScalar(PyObject *obj, PyTypeObject *cls) {
    if (obj == NULL || cls == NULL) {
        return 0;
    }
    return PyObject_TypeCheck(obj, cls);
}

#define PyArray_CheckScalar(obj) PyArray_IsScalar((obj), &PyGenericArrType_Type)

static inline int PyArray_ImportNumPyAPI(void) {
    return 0;
}

static inline int _molt_numpy_descr_elsize_for_type(int typenum) {
    switch (typenum) {
        case NPY_BOOL:
        case NPY_BYTE:
        case NPY_UBYTE:
            return 1;
        case NPY_SHORT:
        case NPY_USHORT:
        case NPY_HALF:
            return 2;
        case NPY_INT:
        case NPY_UINT:
        case NPY_FLOAT:
            return 4;
        case NPY_LONG:
            return (int)sizeof(long);
        case NPY_ULONG:
            return (int)sizeof(unsigned long);
        case NPY_LONGLONG:
            return (int)sizeof(long long);
        case NPY_ULONGLONG:
            return (int)sizeof(unsigned long long);
        case NPY_DOUBLE:
            return (int)sizeof(double);
        case NPY_LONGDOUBLE:
            return (int)sizeof(long double);
        case NPY_CFLOAT:
            return (int)(2 * sizeof(float));
        case NPY_CDOUBLE:
            return (int)(2 * sizeof(double));
        case NPY_CLONGDOUBLE:
            return (int)(2 * sizeof(long double));
        case NPY_DATETIME:
        case NPY_TIMEDELTA:
            return (int)sizeof(npy_int64);
        case NPY_UNICODE:
            return 4;
        case NPY_STRING:
            return 1;
        case NPY_VOID:
            return 0;
        case NPY_OBJECT:
        default:
            return (int)sizeof(PyObject *);
    }
}

static inline char _molt_numpy_descr_kind_for_type(int typenum) {
    switch (typenum) {
        case NPY_BOOL:
            return 'b';
        case NPY_BYTE:
        case NPY_SHORT:
        case NPY_INT:
        case NPY_LONG:
        case NPY_LONGLONG:
            return 'i';
        case NPY_UBYTE:
        case NPY_USHORT:
        case NPY_UINT:
        case NPY_ULONG:
        case NPY_ULONGLONG:
            return 'u';
        case NPY_FLOAT:
        case NPY_DOUBLE:
        case NPY_LONGDOUBLE:
            return 'f';
        case NPY_CFLOAT:
        case NPY_CDOUBLE:
        case NPY_CLONGDOUBLE:
            return 'c';
        case NPY_STRING:
            return 'S';
        case NPY_UNICODE:
            return 'U';
        case NPY_VOID:
            return 'V';
        case NPY_DATETIME:
            return 'M';
        case NPY_TIMEDELTA:
            return 'm';
        case NPY_OBJECT:
        default:
            return 'O';
    }
}

static inline PyArray_ArrFuncs *PyDataType_GetArrFuncs(const PyArray_Descr *descr) {
    (void)descr;
    static PyArray_ArrFuncs funcs;
    return &funcs;
}

static inline PyArray_Descr *PyArray_DescrFromType(int typenum) {
    PyArray_Descr *descr = (PyArray_Descr *)PyMem_Calloc(1, sizeof(PyArray_Descr));
    if (descr == NULL) {
        return NULL;
    }
    descr->type_num = typenum;
    descr->elsize = _molt_numpy_descr_elsize_for_type(typenum);
    descr->alignment = descr->elsize > 0 ? descr->elsize : 1;
    descr->kind = _molt_numpy_descr_kind_for_type(typenum);
    descr->type = descr->kind;
    descr->byteorder = '=';
    if (typenum == NPY_OBJECT) {
        descr->flags |= NPY_ITEM_REFCOUNT | NPY_NEEDS_PYAPI;
    }
    return descr;
}

static inline PyArray_Descr *PyArray_DescrNewFromType(int typenum) {
    return PyArray_DescrFromType(typenum);
}

static inline PyArray_Descr *PyArray_DescrFromScalar(PyObject *obj) {
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline int _molt_numpy_type_rank(int typenum) {
    if (PyTypeNum_ISCOMPLEX(typenum)) {
        return 3000 + _molt_numpy_descr_elsize_for_type(typenum);
    }
    if (PyTypeNum_ISFLOAT(typenum)) {
        return 2000 + _molt_numpy_descr_elsize_for_type(typenum);
    }
    if (PyTypeNum_ISINTEGER(typenum)) {
        return 1000 + _molt_numpy_descr_elsize_for_type(typenum);
    }
    if (PyTypeNum_ISBOOL(typenum)) {
        return 1;
    }
    return 4000 + typenum;
}

static inline int PyArray_EquivTypenums(int typenum1, int typenum2) {
    return typenum1 == typenum2;
}

static inline int PyArray_CanCastSafely(int fromtype, int totype) {
    if (fromtype == totype || fromtype == NPY_BOOL) {
        return 1;
    }
    if (PyTypeNum_ISINTEGER(fromtype) && PyTypeNum_ISINTEGER(totype)) {
        int from_size = _molt_numpy_descr_elsize_for_type(fromtype);
        int to_size = _molt_numpy_descr_elsize_for_type(totype);
        int from_unsigned = PyTypeNum_ISUNSIGNED(fromtype);
        int to_unsigned = PyTypeNum_ISUNSIGNED(totype);
        return (from_unsigned == to_unsigned && to_size >= from_size)
            || (!from_unsigned && !to_unsigned && to_size >= from_size)
            || (from_unsigned && !to_unsigned && to_size > from_size);
    }
    if (PyTypeNum_ISINTEGER(fromtype) && PyTypeNum_ISFLOAT(totype)) {
        return _molt_numpy_descr_elsize_for_type(totype) >= (int)sizeof(double);
    }
    if (PyTypeNum_ISFLOAT(fromtype) && PyTypeNum_ISFLOAT(totype)) {
        return _molt_numpy_descr_elsize_for_type(totype) >= _molt_numpy_descr_elsize_for_type(fromtype);
    }
    if (PyTypeNum_ISNUMBER(fromtype) && PyTypeNum_ISCOMPLEX(totype)) {
        return _molt_numpy_descr_elsize_for_type(totype) >= _molt_numpy_descr_elsize_for_type(fromtype);
    }
    return totype == NPY_OBJECT;
}

static inline PyArray_Descr *PyArray_PromoteTypes(PyArray_Descr *type1, PyArray_Descr *type2) {
    int type_num1 = type1 != NULL ? type1->type_num : NPY_OBJECT;
    int type_num2 = type2 != NULL ? type2->type_num : NPY_OBJECT;
    int promoted = _molt_numpy_type_rank(type_num1) >= _molt_numpy_type_rank(type_num2)
        ? type_num1
        : type_num2;
    return PyArray_DescrFromType(promoted);
}

static inline int PyArray_ObjectType(PyObject *op, int minimum_type) {
    int observed = NPY_OBJECT;
    if (op != NULL && PyArray_Check(op)) {
        observed = PyArray_TYPE((PyArrayObject *)op);
    } else if (PyBool_Check(op)) {
        observed = NPY_BOOL;
    } else if (PyLong_Check(op)) {
        observed = NPY_LONGLONG;
    } else if (PyFloat_Check(op)) {
        observed = NPY_DOUBLE;
    } else if (PyComplex_Check(op)) {
        observed = NPY_CDOUBLE;
    }
    return _molt_numpy_type_rank(observed) >= _molt_numpy_type_rank(minimum_type)
        ? observed
        : minimum_type;
}

static inline int _molt_numpy_store_scalar(
    PyArray_Descr *descr,
    void *ctypeptr,
    PyObject *value
) {
    int typenum = descr != NULL ? descr->type_num : NPY_OBJECT;
    if (ctypeptr == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL destination passed to NumPy scalar store");
        return -1;
    }
    switch (typenum) {
        case NPY_BOOL: {
            npy_bool out = (npy_bool)PyObject_IsTrue(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_BYTE: {
            npy_byte out = (npy_byte)PyLong_AsLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_UBYTE: {
            npy_ubyte out = (npy_ubyte)PyLong_AsUnsignedLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_SHORT: {
            npy_short out = (npy_short)PyLong_AsLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_USHORT: {
            npy_ushort out = (npy_ushort)PyLong_AsUnsignedLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_INT: {
            npy_int out = (npy_int)PyLong_AsLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_UINT: {
            npy_uint out = (npy_uint)PyLong_AsUnsignedLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_LONG: {
            npy_long out = (npy_long)PyLong_AsLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_ULONG: {
            npy_ulong out = (npy_ulong)PyLong_AsUnsignedLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_LONGLONG:
        case NPY_DATETIME:
        case NPY_TIMEDELTA: {
            npy_longlong out = (npy_longlong)PyLong_AsLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_ULONGLONG: {
            npy_ulonglong out = (npy_ulonglong)PyLong_AsUnsignedLongLong(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_FLOAT: {
            npy_float out = (npy_float)PyFloat_AsDouble(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_DOUBLE: {
            npy_double out = (npy_double)PyFloat_AsDouble(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_LONGDOUBLE: {
            npy_longdouble out = (npy_longdouble)PyFloat_AsDouble(value);
            memcpy(ctypeptr, &out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_CFLOAT: {
            Py_complex c = PyComplex_AsCComplex(value);
            npy_float out[2] = {(npy_float)c.real, (npy_float)c.imag};
            memcpy(ctypeptr, out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_CDOUBLE: {
            Py_complex c = PyComplex_AsCComplex(value);
            npy_double out[2] = {(npy_double)c.real, (npy_double)c.imag};
            memcpy(ctypeptr, out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_CLONGDOUBLE: {
            Py_complex c = PyComplex_AsCComplex(value);
            npy_longdouble out[2] = {
                (npy_longdouble)c.real,
                (npy_longdouble)c.imag,
            };
            memcpy(ctypeptr, out, sizeof(out));
            return molt_err_pending() ? -1 : 0;
        }
        case NPY_OBJECT:
        default: {
            PyObject *obj = value != NULL ? value : Py_None;
            Py_INCREF(obj);
            memcpy(ctypeptr, &obj, sizeof(obj));
            return 0;
        }
    }
}

static inline PyObject *_molt_numpy_load_scalar(PyArray_Descr *descr, const void *ctypeptr) {
    int typenum = descr != NULL ? descr->type_num : NPY_OBJECT;
    if (ctypeptr == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL source passed to NumPy scalar load");
        return NULL;
    }
    switch (typenum) {
        case NPY_BOOL: {
            npy_bool value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyBool_FromLong(value != 0);
        }
        case NPY_BYTE: {
            npy_byte value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromLongLong((long long)value);
        }
        case NPY_UBYTE: {
            npy_ubyte value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromUnsignedLongLong((unsigned long long)value);
        }
        case NPY_SHORT: {
            npy_short value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromLongLong((long long)value);
        }
        case NPY_USHORT: {
            npy_ushort value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromUnsignedLongLong((unsigned long long)value);
        }
        case NPY_INT: {
            npy_int value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromLongLong((long long)value);
        }
        case NPY_UINT: {
            npy_uint value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromUnsignedLongLong((unsigned long long)value);
        }
        case NPY_LONG: {
            npy_long value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromLongLong((long long)value);
        }
        case NPY_ULONG: {
            npy_ulong value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromUnsignedLongLong((unsigned long long)value);
        }
        case NPY_LONGLONG:
        case NPY_DATETIME:
        case NPY_TIMEDELTA: {
            npy_longlong value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromLongLong((long long)value);
        }
        case NPY_ULONGLONG: {
            npy_ulonglong value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyLong_FromUnsignedLongLong((unsigned long long)value);
        }
        case NPY_FLOAT: {
            npy_float value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyFloat_FromDouble((double)value);
        }
        case NPY_DOUBLE: {
            npy_double value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyFloat_FromDouble((double)value);
        }
        case NPY_LONGDOUBLE: {
            npy_longdouble value;
            memcpy(&value, ctypeptr, sizeof(value));
            return PyFloat_FromDouble((double)value);
        }
        case NPY_CFLOAT: {
            npy_float value[2];
            memcpy(value, ctypeptr, sizeof(value));
            return PyComplex_FromDoubles((double)value[0], (double)value[1]);
        }
        case NPY_CDOUBLE: {
            npy_double value[2];
            memcpy(value, ctypeptr, sizeof(value));
            return PyComplex_FromDoubles((double)value[0], (double)value[1]);
        }
        case NPY_CLONGDOUBLE: {
            npy_longdouble value[2];
            memcpy(value, ctypeptr, sizeof(value));
            return PyComplex_FromDoubles((double)value[0], (double)value[1]);
        }
        case NPY_OBJECT:
        default: {
            PyObject *obj;
            memcpy(&obj, ctypeptr, sizeof(obj));
            if (obj == NULL) {
                obj = Py_None;
            }
            Py_INCREF(obj);
            return obj;
        }
    }
}

static inline int PyArray_CastScalarToCtype(
    PyObject *scalar,
    void *ctypeptr,
    PyArray_Descr *outcode
) {
    return _molt_numpy_store_scalar(outcode, ctypeptr, scalar);
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

static inline int PyArray_EQUIVALENTLY_ITERABLE_OVERLAP_OK(
    PyArrayObject *arr1,
    PyArrayObject *arr2,
    int arr1_read,
    int arr2_read
) {
    if (arr1_read && arr2_read) {
        return 1;
    }
    if (arr1 == arr2 && PyArray_TRIVIAL_PAIR_ITERATION_STRIDE(PyArray_SIZE(arr1), arr1) != 0) {
        return 1;
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

static inline npy_intp PyArray_Size(PyObject *op) {
    if (op == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyArray_Size");
        return -1;
    }
    if (PyArray_Check(op)) {
        return PyArray_SIZE((PyArrayObject *)op);
    }
    return (npy_intp)PyObject_Length(op);
}

static inline PyObject *PyArray_CheckFromAny(
    PyObject *op,
    PyArray_Descr *descr,
    int min_depth,
    int max_depth,
    int requires,
    PyObject *context
) {
    return PyArray_FromAny(op, descr, min_depth, max_depth, requires, context);
}

static inline PyObject *PyArray_FromObject(
    PyObject *op,
    int typenum,
    int min_depth,
    int max_depth
) {
    return PyArray_FromAny(op, PyArray_DescrFromType(typenum), min_depth, max_depth, 0, NULL);
}

static inline PyObject *PyArray_ContiguousFromAny(
    PyObject *op,
    int typenum,
    int min_depth,
    int max_depth
) {
    return PyArray_FromAny(
        op,
        PyArray_DescrFromType(typenum),
        min_depth,
        max_depth,
        NPY_ARRAY_C_CONTIGUOUS,
        NULL);
}

#define PyArray_ContiguousFromObject(op, typenum, min_depth, max_depth) \
    PyArray_ContiguousFromAny((op), (typenum), (min_depth), (max_depth))

static inline PyObject *PyArray_CopyFromObject(
    PyObject *op,
    int typenum,
    int min_depth,
    int max_depth
) {
    return PyArray_FromAny(
        op,
        PyArray_DescrFromType(typenum),
        min_depth,
        max_depth,
        NPY_ARRAY_ENSURECOPY,
        NULL);
}

static inline int PyArray_IntpConverter(PyObject *obj, PyArray_Dims *seq) {
    Py_ssize_t len;
    Py_ssize_t i;
    if (seq == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL PyArray_Dims output");
        return 0;
    }
    seq->ptr = NULL;
    seq->len = 0;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyArray_IntpConverter");
        return 0;
    }
    if (!PySequence_Check(obj) || PyLong_Check(obj)) {
        seq->ptr = (npy_intp *)PyMem_Calloc(1, sizeof(npy_intp));
        if (seq->ptr == NULL) {
            return 0;
        }
        seq->ptr[0] = (npy_intp)PyLong_AsSsize_t(obj);
        seq->len = 1;
        if (molt_err_pending()) {
            PyMem_Free(seq->ptr);
            seq->ptr = NULL;
            seq->len = 0;
            return 0;
        }
        return 1;
    }
    len = PySequence_Size(obj);
    if (len < 0) {
        return 0;
    }
    seq->ptr = (npy_intp *)PyMem_Calloc((size_t)(len > 0 ? len : 1), sizeof(npy_intp));
    if (seq->ptr == NULL) {
        return 0;
    }
    seq->len = (int)len;
    for (i = 0; i < len; i++) {
        PyObject *item = PySequence_GetItem(obj, i);
        if (item == NULL) {
            PyDimMem_FREE(seq->ptr);
            seq->ptr = NULL;
            seq->len = 0;
            return 0;
        }
        seq->ptr[i] = (npy_intp)PyLong_AsSsize_t(item);
        Py_DECREF(item);
        if (molt_err_pending()) {
            PyDimMem_FREE(seq->ptr);
            seq->ptr = NULL;
            seq->len = 0;
            return 0;
        }
    }
    return 1;
}

static inline PyObject *PyArray_PyIntFromIntp(npy_intp value) {
    return PyLong_FromSsize_t((Py_ssize_t)value);
}

static inline int PyArray_LookupSpecial(
    PyObject *obj,
    PyObject *name_unicode,
    PyObject **result
) {
    if (result == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL result passed to PyArray_LookupSpecial");
        return -1;
    }
    *result = NULL;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyArray_LookupSpecial");
        return -1;
    }
    return PyObject_GetOptionalAttr((PyObject *)Py_TYPE(obj), name_unicode, result);
}

static inline int PyArray_LookupSpecial_OnInstance(
    PyObject *obj,
    PyObject *name_unicode,
    PyObject **result
) {
    if (result == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL result passed to PyArray_LookupSpecial_OnInstance");
        return -1;
    }
    *result = NULL;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyArray_LookupSpecial_OnInstance");
        return -1;
    }
    return PyObject_GetOptionalAttr(obj, name_unicode, result);
}

static inline PyObject *PyArray_TupleFromItems(
    int count,
    PyObject *const *items,
    int make_null_none
) {
    PyObject *tuple = PyTuple_New((Py_ssize_t)count);
    int i;
    if (tuple == NULL) {
        return NULL;
    }
    for (i = 0; i < count; i++) {
        PyObject *item = (!make_null_none || items[i] != NULL) ? items[i] : Py_None;
        Py_INCREF(item);
        PyTuple_SET_ITEM(tuple, (Py_ssize_t)i, item);
    }
    return tuple;
}

static inline int _molt_numpy_copy_dims(
    npy_intp **out,
    const npy_intp *dims,
    int nd
) {
    int i;
    if (nd <= 0) {
        *out = NULL;
        return 0;
    }
    if (dims == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL dimensions for non-scalar NumPy array");
        return -1;
    }
    *out = (npy_intp *)PyMem_Calloc((size_t)nd, sizeof(npy_intp));
    if (*out == NULL) {
        return -1;
    }
    for (i = 0; i < nd; i++) {
        if (dims[i] < 0) {
            PyMem_Free(*out);
            *out = NULL;
            PyErr_SetString(PyExc_ValueError, "negative dimensions are not allowed");
            return -1;
        }
        (*out)[i] = dims[i];
    }
    return 0;
}

static inline npy_intp _molt_numpy_dims_size(const npy_intp *dims, int nd) {
    npy_intp size = 1;
    int i;
    for (i = 0; i < nd; i++) {
        size *= dims[i];
    }
    return size;
}

static inline int _molt_numpy_fill_strides(
    npy_intp *strides_out,
    const npy_intp *dims,
    int nd,
    int itemsize,
    int is_fortran
) {
    int i;
    npy_intp stride = itemsize > 0 ? (npy_intp)itemsize : 1;
    if (nd <= 0) {
        return 0;
    }
    if (strides_out == NULL || dims == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL stride or dimension buffer");
        return -1;
    }
    if (is_fortran) {
        for (i = 0; i < nd; i++) {
            strides_out[i] = stride;
            stride *= dims[i] > 0 ? dims[i] : 1;
        }
    } else {
        for (i = nd - 1; i >= 0; i--) {
            strides_out[i] = stride;
            stride *= dims[i] > 0 ? dims[i] : 1;
        }
    }
    return 0;
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
    PyArrayObject_fields *array_obj;
    npy_intp size;
    int i;
    int owns_data = 0;
    (void)subtype;
    if (nd < 0) {
        PyErr_SetString(PyExc_ValueError, "negative number of dimensions");
        return NULL;
    }
    if (descr == NULL) {
        descr = PyArray_DescrFromType(NPY_OBJECT);
        if (descr == NULL) {
            return NULL;
        }
    }
    array_obj = (PyArrayObject_fields *)PyMem_Calloc(1, sizeof(PyArrayObject_fields));
    if (array_obj == NULL) {
        return NULL;
    }
    array_obj->ob_base = (PyObject *)&PyArray_Type;
    array_obj->nd = nd;
    array_obj->descr = descr;
    array_obj->flags = flags | NPY_ARRAY_ALIGNED | NPY_ARRAY_WRITEABLE;
    if (_molt_numpy_copy_dims(&array_obj->dimensions, dims, nd) < 0) {
        PyMem_Free(array_obj);
        return NULL;
    }
    if (nd > 0) {
        array_obj->strides = (npy_intp *)PyMem_Calloc((size_t)nd, sizeof(npy_intp));
        if (array_obj->strides == NULL) {
            PyMem_Free(array_obj->dimensions);
            PyMem_Free(array_obj);
            return NULL;
        }
        if (strides != NULL) {
            for (i = 0; i < nd; i++) {
                array_obj->strides[i] = strides[i];
            }
        } else if (_molt_numpy_fill_strides(
                array_obj->strides,
                array_obj->dimensions,
                nd,
                descr->elsize,
                (flags & NPY_ARRAY_F_CONTIGUOUS) != 0) < 0) {
            PyMem_Free(array_obj->strides);
            PyMem_Free(array_obj->dimensions);
            PyMem_Free(array_obj);
            return NULL;
        }
    }
    size = _molt_numpy_dims_size(array_obj->dimensions, nd);
    if (data != NULL) {
        array_obj->data = (char *)data;
    } else {
        npy_intp nbytes = size * (descr->elsize > 0 ? descr->elsize : 1);
        array_obj->data = (char *)PyMem_Calloc((size_t)(nbytes > 0 ? nbytes : 1), 1);
        if (array_obj->data == NULL) {
            PyMem_Free(array_obj->strides);
            PyMem_Free(array_obj->dimensions);
            PyMem_Free(array_obj);
            return NULL;
        }
        owns_data = 1;
    }
    if (owns_data) {
        array_obj->flags |= NPY_ARRAY_OWNDATA;
    }
    if (strides == NULL) {
        array_obj->flags |= (flags & NPY_ARRAY_F_CONTIGUOUS) != 0
            ? NPY_ARRAY_F_CONTIGUOUS
            : NPY_ARRAY_C_CONTIGUOUS;
    }
    if (obj != NULL) {
        array_obj->base = obj;
        Py_INCREF(obj);
    }
    return (PyObject *)array_obj;
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
    if (c_order) {
        flags |= NPY_ARRAY_C_CONTIGUOUS;
    }
    (void)ensure_array;
    return PyArray_NewFromDescrAndBase(subtype, descr, nd, dims, strides, data, flags, obj, base);
}

static inline PyObject *PyArray_Empty(
    int nd,
    npy_intp *dims,
    PyArray_Descr *descr,
    int isfortran
) {
    int flags = isfortran ? NPY_ARRAY_F_CONTIGUOUS : NPY_ARRAY_C_CONTIGUOUS;
    return PyArray_NewFromDescr(&PyArray_Type, descr, nd, dims, NULL, NULL, flags, NULL);
}

static inline PyObject *PyArray_Zeros(
    int nd,
    npy_intp *dims,
    PyArray_Descr *descr,
    int isfortran
) {
    PyObject *array_obj = PyArray_Empty(nd, dims, descr, isfortran);
    if (array_obj != NULL) {
        memset(PyArray_DATA((PyArrayObject *)array_obj), 0, (size_t)PyArray_NBYTES((PyArrayObject *)array_obj));
    }
    return array_obj;
}

#define PyArray_EMPTY(nd, dims, typenum, isfortran) \
    ((PyArrayObject *)PyArray_Empty((nd), (dims), PyArray_DescrFromType((typenum)), (isfortran)))
#define PyArray_SimpleNew(nd, dims, typenum) PyArray_EMPTY((nd), (dims), (typenum), 0)
#define PyArray_SimpleNewFromData(nd, dims, typenum, data) \
    ((PyArrayObject *)PyArray_NewFromDescr( \
        &PyArray_Type, \
        PyArray_DescrFromType((typenum)), \
        (nd), \
        (dims), \
        NULL, \
        (data), \
        NPY_ARRAY_C_CONTIGUOUS, \
        NULL))
#define PyArray_ZEROS(nd, dims, typenum, isfortran) \
    ((PyArrayObject *)PyArray_Zeros((nd), (dims), PyArray_DescrFromType((typenum)), (isfortran)))

static inline PyObject *PyArray_NewCopy(PyArrayObject *array_obj, NPY_ORDER order) {
    PyObject *copy_obj;
    npy_intp nbytes;
    int is_fortran = order == NPY_FORTRANORDER;
    if (array_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL array passed to PyArray_NewCopy");
        return NULL;
    }
    copy_obj = PyArray_Empty(
        PyArray_NDIM(array_obj),
        PyArray_DIMS(array_obj),
        PyArray_DescrFromType(PyArray_TYPE(array_obj)),
        is_fortran);
    if (copy_obj == NULL) {
        return NULL;
    }
    nbytes = PyArray_NBYTES(array_obj);
    if (nbytes > 0) {
        memcpy(PyArray_DATA((PyArrayObject *)copy_obj), PyArray_DATA(array_obj), (size_t)nbytes);
    }
    return copy_obj;
}

static inline void PyArray_UpdateFlags(PyArrayObject *array_obj, int flagmask) {
    int nd;
    int i;
    int c_contiguous = 1;
    int f_contiguous = 1;
    npy_intp expected;
    int itemsize;
    if (array_obj == NULL) {
        return;
    }
    nd = PyArray_NDIM(array_obj);
    itemsize = PyArray_ITEMSIZE(array_obj);
    if (itemsize <= 0) {
        itemsize = 1;
    }
    if (nd > 0 && PyArray_STRIDES(array_obj) != NULL && PyArray_DIMS(array_obj) != NULL) {
        expected = itemsize;
        for (i = nd - 1; i >= 0; i--) {
            if (PyArray_DIM(array_obj, i) > 1 && PyArray_STRIDE(array_obj, i) != expected) {
                c_contiguous = 0;
                break;
            }
            expected *= PyArray_DIM(array_obj, i) > 0 ? PyArray_DIM(array_obj, i) : 1;
        }
        expected = itemsize;
        for (i = 0; i < nd; i++) {
            if (PyArray_DIM(array_obj, i) > 1 && PyArray_STRIDE(array_obj, i) != expected) {
                f_contiguous = 0;
                break;
            }
            expected *= PyArray_DIM(array_obj, i) > 0 ? PyArray_DIM(array_obj, i) : 1;
        }
    }
    if ((flagmask & NPY_ARRAY_C_CONTIGUOUS) != 0) {
        ((PyArrayObject_fields *)array_obj)->flags &= ~NPY_ARRAY_C_CONTIGUOUS;
        if (c_contiguous) {
            ((PyArrayObject_fields *)array_obj)->flags |= NPY_ARRAY_C_CONTIGUOUS;
        }
    }
    if ((flagmask & NPY_ARRAY_F_CONTIGUOUS) != 0) {
        ((PyArrayObject_fields *)array_obj)->flags &= ~NPY_ARRAY_F_CONTIGUOUS;
        if (f_contiguous) {
            ((PyArrayObject_fields *)array_obj)->flags |= NPY_ARRAY_F_CONTIGUOUS;
        }
    }
    if ((flagmask & NPY_ARRAY_ALIGNED) != 0) {
        ((PyArrayObject_fields *)array_obj)->flags |= NPY_ARRAY_ALIGNED;
    }
}

static inline PyObject *PyArray_Cast(PyArrayObject *array_obj, int typenum) {
    PyObject *cast_obj;
    PyArray_Descr *out_descr;
    npy_intp i;
    npy_intp count;
    if (array_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL array passed to PyArray_Cast");
        return NULL;
    }
    out_descr = PyArray_DescrFromType(typenum);
    if (out_descr == NULL) {
        return NULL;
    }
    cast_obj = PyArray_Empty(PyArray_NDIM(array_obj), PyArray_DIMS(array_obj), out_descr, 0);
    if (cast_obj == NULL) {
        return NULL;
    }
    count = PyArray_SIZE(array_obj);
    for (i = 0; i < count; i++) {
        PyObject *scalar = _molt_numpy_load_scalar(
            PyArray_DESCR(array_obj),
            PyArray_DATA(array_obj) + i * PyArray_ITEMSIZE(array_obj));
        if (scalar == NULL) {
            Py_DECREF(cast_obj);
            return NULL;
        }
        if (_molt_numpy_store_scalar(
                out_descr,
                PyArray_DATA((PyArrayObject *)cast_obj) + i * PyDataType_ELSIZE(out_descr),
                scalar) < 0) {
            Py_DECREF(scalar);
            Py_DECREF(cast_obj);
            return NULL;
        }
        Py_DECREF(scalar);
    }
    return cast_obj;
}

static inline PyObject *PyArray_Return(PyArrayObject *array_obj) {
    return (PyObject *)array_obj;
}

static inline PyObject *PyArray_Scalar(void *data, PyArray_Descr *descr, PyObject *base) {
    (void)base;
    return _molt_numpy_load_scalar(descr, data);
}

static inline PyObject *PyArray_ToScalar(void *data, PyArrayObject *array_obj) {
    return _molt_numpy_load_scalar(array_obj != NULL ? PyArray_DESCR(array_obj) : NULL, data);
}

static inline PyObject *PyArray_Zero(PyArrayObject *array_obj) {
    char zero_bytes[sizeof(npy_longdouble) * 2];
    memset(zero_bytes, 0, sizeof(zero_bytes));
    return _molt_numpy_load_scalar(
        array_obj != NULL ? PyArray_DESCR(array_obj) : NULL,
        zero_bytes);
}

static inline PyObject *PyArray_Arange(double start, double stop, double step, int typenum) {
    npy_intp dim;
    PyArrayObject *array_obj;
    npy_intp i;
    if (step == 0.0) {
        PyErr_SetString(PyExc_ValueError, "arange step must not be zero");
        return NULL;
    }
    dim = (npy_intp)ceil((stop - start) / step);
    if (dim < 0) {
        dim = 0;
    }
    array_obj = PyArray_SimpleNew(1, &dim, typenum);
    if (array_obj == NULL) {
        return NULL;
    }
    for (i = 0; i < dim; i++) {
        PyObject *value = PyFloat_FromDouble(start + (double)i * step);
        if (value == NULL) {
            Py_DECREF((PyObject *)array_obj);
            return NULL;
        }
        if (_molt_numpy_store_scalar(
                PyArray_DESCR(array_obj),
                PyArray_DATA(array_obj) + i * PyArray_ITEMSIZE(array_obj),
                value) < 0) {
            Py_DECREF(value);
            Py_DECREF((PyObject *)array_obj);
            return NULL;
        }
        Py_DECREF(value);
    }
    return (PyObject *)array_obj;
}

static inline PyObject *PyArray_IterNew(PyObject *obj) {
    PyArrayObject *array_obj = (PyArrayObject *)obj;
    PyArrayIterObject *iter;
    if (array_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL array passed to PyArray_IterNew");
        return NULL;
    }
    iter = (PyArrayIterObject *)PyMem_Calloc(1, sizeof(PyArrayIterObject));
    if (iter == NULL) {
        return NULL;
    }
    iter->ob_base = (PyObject *)&PyArray_Type;
    iter->ao = array_obj;
    iter->index = 0;
    iter->size = PyArray_SIZE(array_obj);
    iter->dataptr = PyArray_DATA(array_obj);
    return (PyObject *)iter;
}

static inline void _molt_numpy_iter_reset(PyArrayIterObject *iter) {
    if (iter == NULL) {
        return;
    }
    iter->index = 0;
    iter->dataptr = iter->ao != NULL ? PyArray_DATA(iter->ao) : NULL;
}

static inline void _molt_numpy_iter_next(PyArrayIterObject *iter) {
    if (iter == NULL || iter->ao == NULL) {
        return;
    }
    iter->index++;
    iter->dataptr += PyArray_ITEMSIZE(iter->ao);
}

static inline PyObject *PyArray_IterAllButAxis(PyObject *obj, int *axis) {
    (void)axis;
    return PyArray_IterNew(obj);
}

static inline PyObject *PyArray_NeighborhoodIterNew(
    PyArrayIterObject *iter,
    npy_intp *bounds,
    int mode,
    PyObject *fill_value
) {
    PyArrayNeighborhoodIterObject *neighborhood;
    (void)bounds;
    (void)mode;
    (void)fill_value;
    if (iter == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL iterator passed to PyArray_NeighborhoodIterNew");
        return NULL;
    }
    neighborhood = (PyArrayNeighborhoodIterObject *)PyMem_Calloc(
        1,
        sizeof(PyArrayNeighborhoodIterObject));
    if (neighborhood == NULL) {
        return NULL;
    }
    neighborhood->ob_base = (PyObject *)&PyArrayNeighborhoodIter_Type;
    neighborhood->ao = iter->ao;
    neighborhood->index = 0;
    neighborhood->size = iter->size;
    neighborhood->dataptr = iter->dataptr;
    return (PyObject *)neighborhood;
}

static inline void PyArrayNeighborhoodIter_Reset(PyArrayNeighborhoodIterObject *iter) {
    if (iter == NULL) {
        return;
    }
    iter->index = 0;
    iter->dataptr = iter->ao != NULL ? PyArray_DATA(iter->ao) : NULL;
}

static inline void PyArrayNeighborhoodIter_Next(PyArrayNeighborhoodIterObject *iter) {
    if (iter == NULL || iter->ao == NULL) {
        return;
    }
    iter->index++;
    iter->dataptr += PyArray_ITEMSIZE(iter->ao);
}

static inline int PyArrayNeighborhoodIter_Next2D(PyArrayNeighborhoodIterObject *iter) {
    if (iter == NULL || iter->ao == NULL) {
        return 0;
    }
    PyArrayNeighborhoodIter_Next(iter);
    return iter->index < PyArray_SIZE(iter->ao);
}

static inline int PyArray_Pack(
    PyArray_Descr *descr,
    void *item,
    PyObject *value
) {
    return _molt_numpy_store_scalar(descr, item, value);
}

static inline PyObject *PyArray_GETITEM(PyArrayObject *array_obj, const char *item_ptr) {
    return _molt_numpy_load_scalar(
        array_obj != NULL ? PyArray_DESCR(array_obj) : NULL,
        item_ptr);
}

static inline int PyArray_SETITEM(PyArrayObject *array_obj, char *item_ptr, PyObject *value) {
    return _molt_numpy_store_scalar(
        array_obj != NULL ? PyArray_DESCR(array_obj) : NULL,
        item_ptr,
        value);
}

static inline int _molt_numpy_strided_copy_loop(
    PyArrayMethod_Context *context,
    char **data,
    npy_intp *dimensions,
    npy_intp *strides,
    void *auxdata
) {
    npy_intp i;
    npy_intp count = dimensions != NULL ? dimensions[0] : 0;
    npy_intp src_stride = strides != NULL ? strides[0] : 0;
    npy_intp dst_stride = strides != NULL ? strides[1] : 0;
    npy_intp itemsize = (npy_intp)(uintptr_t)auxdata;
    char *src = data != NULL ? data[0] : NULL;
    char *dst = data != NULL ? data[1] : NULL;
    (void)context;
    if (src == NULL || dst == NULL || itemsize <= 0) {
        return -1;
    }
    if (src_stride == 0) {
        src_stride = itemsize;
    }
    if (dst_stride == 0) {
        dst_stride = itemsize;
    }
    for (i = 0; i < count; i++) {
        memcpy(dst + i * dst_stride, src + i * src_stride, (size_t)itemsize);
    }
    return 0;
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
    (void)move_references;
    if (out_loop == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL transfer loop output");
        return -1;
    }
    *out_loop = _molt_numpy_strided_copy_loop;
    if (out_transferdata != NULL) {
        int itemsize = dst_dtype != NULL ? dst_dtype->elsize : 1;
        *out_transferdata = (void *)(uintptr_t)(itemsize > 0 ? itemsize : 1);
    }
    if (out_needs_api != NULL) {
        *out_needs_api = 0;
    }
    return 0;
}

static inline PyArrayMethod_StridedLoop PyArray_GetStridedCopyFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    npy_intp itemsize
) {
    (void)aligned;
    (void)src_stride;
    (void)dst_stride;
    (void)itemsize;
    return _molt_numpy_strided_copy_loop;
}

static inline PyArrayMethod_StridedLoop PyArray_GetStridedCopySwapFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    npy_intp itemsize
) {
    return PyArray_GetStridedCopyFn(aligned, src_stride, dst_stride, itemsize);
}

static inline PyArrayMethod_StridedLoop PyArray_GetStridedCopySwapPairFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    npy_intp itemsize
) {
    return PyArray_GetStridedCopyFn(aligned, src_stride, dst_stride, itemsize);
}

static inline PyArrayMethod_StridedLoop PyArray_GetStridedNumericCastFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    int src_type_num,
    int dst_type_num
) {
    (void)src_type_num;
    (void)dst_type_num;
    return PyArray_GetStridedCopyFn(aligned, src_stride, dst_stride, 0);
}

static inline int PyArray_GetDTypeCopySwapFn(
    int aligned,
    npy_intp src_stride,
    npy_intp dst_stride,
    PyArray_Descr *dtype,
    PyArrayMethod_StridedLoop *outstransfer,
    NpyAuxData **outtransferdata
) {
    if (outstransfer == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL dtype copy output");
        return -1;
    }
    *outstransfer = PyArray_GetStridedCopyFn(
        aligned,
        src_stride,
        dst_stride,
        dtype != NULL && dtype->elsize > 0 ? dtype->elsize : 1);
    if (outtransferdata != NULL) {
        *outtransferdata = NULL;
    }
    return 0;
}

static inline npy_intp PyArray_TransferNDimToStrided(
    npy_intp ndim,
    char *dst,
    npy_intp dst_stride,
    char *src,
    npy_intp const *src_strides,
    npy_intp src_strides_inc,
    npy_intp const *coords,
    npy_intp coords_inc,
    npy_intp const *shape,
    npy_intp shape_inc,
    npy_intp count,
    npy_intp src_itemsize,
    NPY_cast_info *cast_info
) {
    npy_intp i;
    npy_intp src_stride = src_strides != NULL ? src_strides[0] : src_itemsize;
    (void)ndim;
    (void)src_strides_inc;
    (void)coords;
    (void)coords_inc;
    (void)shape;
    (void)shape_inc;
    (void)cast_info;
    for (i = 0; i < count; i++) {
        memcpy(dst + i * dst_stride, src + i * src_stride, (size_t)src_itemsize);
    }
    return count;
}

static inline npy_intp PyArray_TransferStridedToNDim(
    npy_intp ndim,
    char *dst,
    npy_intp const *dst_strides,
    npy_intp dst_strides_inc,
    char *src,
    npy_intp src_stride,
    npy_intp const *coords,
    npy_intp coords_inc,
    npy_intp const *shape,
    npy_intp shape_inc,
    npy_intp count,
    npy_intp src_itemsize,
    NPY_cast_info *cast_info
) {
    npy_intp i;
    npy_intp dst_stride = dst_strides != NULL ? dst_strides[0] : src_itemsize;
    (void)ndim;
    (void)dst_strides_inc;
    (void)coords;
    (void)coords_inc;
    (void)shape;
    (void)shape_inc;
    (void)cast_info;
    for (i = 0; i < count; i++) {
        memcpy(dst + i * dst_stride, src + i * src_stride, (size_t)src_itemsize);
    }
    return count;
}

static inline npy_intp PyArray_TransferMaskedStridedToNDim(
    npy_intp ndim,
    char *dst,
    npy_intp const *dst_strides,
    npy_intp dst_strides_inc,
    char *src,
    npy_intp src_stride,
    npy_bool *mask,
    npy_intp mask_stride,
    npy_intp const *coords,
    npy_intp coords_inc,
    npy_intp const *shape,
    npy_intp shape_inc,
    npy_intp count,
    npy_intp src_itemsize,
    NPY_cast_info *cast_info
) {
    npy_intp i;
    npy_intp copied = 0;
    npy_intp dst_stride = dst_strides != NULL ? dst_strides[0] : src_itemsize;
    (void)ndim;
    (void)dst_strides_inc;
    (void)coords;
    (void)coords_inc;
    (void)shape;
    (void)shape_inc;
    (void)cast_info;
    for (i = 0; i < count; i++) {
        if (mask == NULL || *(mask + i * mask_stride)) {
            memcpy(dst + i * dst_stride, src + i * src_stride, (size_t)src_itemsize);
            copied++;
        }
    }
    return copied;
}

static inline void PyArray_CreateSortedStridePerm(
    int ndim,
    npy_intp const *strides,
    npy_stride_sort_item *strideperm
) {
    int i;
    int j;
    if (strideperm == NULL) {
        return;
    }
    for (i = 0; i < ndim; i++) {
        strideperm[i].perm = i;
        strideperm[i].stride = strides != NULL ? strides[i] : 0;
    }
    for (i = 1; i < ndim; i++) {
        npy_stride_sort_item item = strideperm[i];
        npy_intp magnitude = item.stride < 0 ? -item.stride : item.stride;
        j = i - 1;
        while (j >= 0) {
            npy_intp prev = strideperm[j].stride < 0 ? -strideperm[j].stride : strideperm[j].stride;
            if (prev >= magnitude) {
                break;
            }
            strideperm[j + 1] = strideperm[j];
            j--;
        }
        strideperm[j + 1] = item;
    }
}

static inline void PyArray_CreateMultiSortedStridePerm(
    int narrays,
    PyArrayObject **arrays,
    int ndim,
    npy_stride_sort_item *strideperm
) {
    (void)narrays;
    PyArray_CreateSortedStridePerm(
        ndim,
        arrays != NULL && arrays[0] != NULL ? PyArray_STRIDES(arrays[0]) : NULL,
        strideperm);
}

static inline PyObject *PyArray_Newshape(
    PyArrayObject *self,
    PyArray_Dims *newdims,
    NPY_ORDER order
) {
    int flags;
    if (self == NULL || newdims == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument passed to PyArray_Newshape");
        return NULL;
    }
    flags = (order == NPY_FORTRANORDER) ? NPY_ARRAY_F_CONTIGUOUS : NPY_ARRAY_C_CONTIGUOUS;
    return PyArray_NewFromDescr(
        &PyArray_Type,
        PyArray_DESCR(self),
        newdims->len,
        newdims->ptr,
        NULL,
        PyArray_DATA(self),
        flags,
        (PyObject *)self);
}

static inline PyObject *PyArray_Resize(
    PyArrayObject *self,
    PyArray_Dims *newshape,
    int refcheck,
    NPY_ORDER order
) {
    npy_intp *new_dims = NULL;
    npy_intp *new_strides = NULL;
    npy_intp nbytes;
    (void)refcheck;
    if (self == NULL || newshape == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument passed to PyArray_Resize");
        return NULL;
    }
    if (_molt_numpy_copy_dims(&new_dims, newshape->ptr, newshape->len) < 0) {
        return NULL;
    }
    if (newshape->len > 0) {
        new_strides = (npy_intp *)PyMem_Calloc((size_t)newshape->len, sizeof(npy_intp));
        if (new_strides == NULL) {
            PyMem_Free(new_dims);
            return NULL;
        }
        if (_molt_numpy_fill_strides(
                new_strides,
                new_dims,
                newshape->len,
                PyArray_ITEMSIZE(self),
                order == NPY_FORTRANORDER) < 0) {
            PyMem_Free(new_strides);
            PyMem_Free(new_dims);
            return NULL;
        }
    }
    nbytes = _molt_numpy_dims_size(new_dims, newshape->len) * PyArray_ITEMSIZE(self);
    if (PyArray_DATA(self) != NULL && PyArray_CHKFLAGS(self, NPY_ARRAY_OWNDATA)) {
        char *new_data = (char *)PyMem_Realloc(PyArray_DATA(self), (size_t)(nbytes > 0 ? nbytes : 1));
        if (new_data == NULL) {
            PyMem_Free(new_strides);
            PyMem_Free(new_dims);
            return NULL;
        }
        ((PyArrayObject_fields *)self)->data = new_data;
    }
    PyMem_Free(PyArray_DIMS(self));
    PyMem_Free(PyArray_STRIDES(self));
    ((PyArrayObject_fields *)self)->dimensions = new_dims;
    ((PyArrayObject_fields *)self)->strides = new_strides;
    ((PyArrayObject_fields *)self)->nd = newshape->len;
    Py_INCREF((PyObject *)self);
    return (PyObject *)self;
}

static inline PyObject *PyArray_FromArrayAttr_int(
    PyObject *op,
    PyArray_Descr *descr,
    int copy,
    PyObject *context
) {
    (void)copy;
    return PyArray_FromAny(op, descr, 0, 0, 0, context);
}

static inline PyObject *PyArray_FromArray(
    PyArrayObject *op,
    PyArray_Descr *descr,
    int flags
) {
    return PyArray_FromAny((PyObject *)op, descr, 0, 0, flags, NULL);
}

static inline PyObject *PyArray_EnsureArray(PyObject *op) {
    return PyArray_FromAny(op, NULL, 0, 0, NPY_ARRAY_ENSUREARRAY, NULL);
}

static inline int PyArray_SetBaseObject(PyArrayObject *arr, PyObject *base) {
    if (arr == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL array passed to PyArray_SetBaseObject");
        return -1;
    }
    ((PyArrayObject_fields *)arr)->base = base;
    return 0;
}

static inline int PyArray_ResolveWritebackIfCopy(PyArrayObject *arr) {
    if (arr != NULL) {
        ((PyArrayObject_fields *)arr)->flags &= ~NPY_ARRAY_WRITEBACKIFCOPY;
    }
    return 0;
}

static inline int PyArray_DiscardWritebackIfCopy(PyArrayObject *arr) {
    return PyArray_ResolveWritebackIfCopy(arr);
}

static inline int PyArray_CheckLegacyResultType(
    PyArrayObject *arr,
    PyArray_DTypeMeta *dtype
) {
    (void)arr;
    (void)dtype;
    return 0;
}

static inline PyArray_DTypeMeta *PyArrayDTypeMeta_CommonDType(
    PyArray_DTypeMeta *left,
    PyArray_DTypeMeta *right
) {
    PyArray_DTypeMeta *result = left != NULL ? left : right;
    if (result == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL dtype passed to PyArrayDTypeMeta_CommonDType");
        return NULL;
    }
    Py_INCREF((PyObject *)result);
    return result;
}

static inline PyArray_Descr *PyArrayDTypeMeta_CommonInstance(
    PyArray_DTypeMeta *dtype,
    PyObject *obj
) {
    (void)dtype;
    (void)obj;
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyArray_Descr *PyArrayDTypeMeta_DefaultDescriptor(PyArray_DTypeMeta *dtype) {
    if (dtype != NULL && dtype->singleton != NULL) {
        Py_INCREF((PyObject *)dtype->singleton);
        return dtype->singleton;
    }
    return PyArray_DescrFromType(NPY_OBJECT);
}

static inline PyArray_DTypeMeta *PyArray_DTypeFromTypeNum(int typenum) {
    PyArray_DTypeMeta *dtype = (PyArray_DTypeMeta *)PyMem_Calloc(1, sizeof(PyArray_DTypeMeta));
    if (dtype == NULL) {
        return NULL;
    }
    dtype->ob_base = (PyObject *)&PyArrayDTypeMeta_Type;
    dtype->singleton = PyArray_DescrFromType(typenum);
    if (dtype->singleton == NULL) {
        PyMem_Free(dtype);
        return NULL;
    }
    return dtype;
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
    switch (constant) {
        case NPY_CONSTANT_ZERO:
            return PyLong_FromLong(0);
        case NPY_CONSTANT_ONE:
            return PyLong_FromLong(1);
        case NPY_CONSTANT_MINUS_ONE:
            return PyLong_FromLong(-1);
        case NPY_CONSTANT_INFINITY:
            return PyFloat_FromDouble(HUGE_VAL);
        case NPY_CONSTANT_NAN:
            return PyFloat_FromDouble(NAN);
        default:
            Py_INCREF(Py_None);
            return Py_None;
    }
}

static inline PyObject *PyArrayDTypeMeta_GetItem(
    PyArray_DTypeMeta *dtype,
    const char *data
) {
    return _molt_numpy_load_scalar(
        dtype != NULL && dtype->singleton != NULL ? dtype->singleton : NULL,
        data);
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
    return _molt_numpy_store_scalar(
        dtype != NULL && dtype->singleton != NULL ? dtype->singleton : NULL,
        data,
        obj);
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
