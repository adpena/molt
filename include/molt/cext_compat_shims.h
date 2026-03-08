#ifndef MOLT_CEXT_COMPAT_SHIMS_H
#define MOLT_CEXT_COMPAT_SHIMS_H

/*
 * Additive source-compatibility shims for common CPython/NumPy extension macros.
 * This header is force-included by Molt extension build/scan flows.
 */

#include <Python.h>
#include <numpy/arrayobject.h>

#ifdef NPY_INT64_FMT
#undef NPY_INT64_FMT
#endif
#ifdef NPY_UINT64_FMT
#undef NPY_UINT64_FMT
#endif
#if NPY_SIZEOF_LONG == 8
#define NPY_INT64_FMT "ld"
#define NPY_UINT64_FMT "lu"
#else
#define NPY_INT64_FMT "lld"
#define NPY_UINT64_FMT "llu"
#endif
#ifndef NPY_INT32_FMT
#define NPY_INT32_FMT "d"
#endif
#ifndef NPY_UINT32_FMT
#define NPY_UINT32_FMT "u"
#endif

#ifndef PyOS_vsnprintf
#define PyOS_vsnprintf vsnprintf
#endif

#ifndef Py_IS_NAN
#define Py_IS_NAN(X) isnan((X))
#endif
#ifndef Py_IS_INFINITY
#define Py_IS_INFINITY(X) isinf((X))
#endif
#ifndef Py_IS_FINITE
#define Py_IS_FINITE(X) isfinite((X))
#endif

#ifndef PyLong_FromSize_t
#define PyLong_FromSize_t(value) PyLong_FromUnsignedLongLong((unsigned long long)(value))
#endif

#ifndef PySequence_ITEM
#define PySequence_ITEM(seq, index) PySequence_GetItem((seq), (index))
#endif

#ifndef PyErr_NewExceptionWithDoc
#define PyErr_NewExceptionWithDoc(name, doc, base, dict) \
    PyErr_NewException((name), (base), (dict))
#endif

#ifndef PyObject_GenericSetDict
#define PyObject_GenericSetDict(obj, value, context) \
    ((void)(context), \
     ((value) == NULL \
          ? PyObject_DelAttrString((obj), "__dict__") \
          : PyObject_SetAttrString((obj), "__dict__", (value))))
#endif

#ifndef PyExc_SystemExit
#define PyExc_SystemExit PyExc_Exception
#endif

#ifndef PyObject_VectorcallDict
#define PyObject_VectorcallDict(callable, args, nargsf, kwargs) \
    PyObject_Vectorcall((callable), (args), (nargsf), NULL)
#endif

#ifndef PyArray_FROM_OTF
#define PyArray_FROM_OTF(obj, typenum, flags) \
    PyArray_FromAny((obj), PyArray_DescrFromType((typenum)), 0, 0, (flags), NULL)
#endif

#ifndef PyArray_SimpleNew
#define PyArray_SimpleNew(nd, dims, typenum) \
    ((PyArrayObject *)PyArray_Empty((nd), (dims), PyArray_DescrFromType((typenum)), 0))
#endif

#ifndef PyArray_SimpleNewFromData
#define PyArray_SimpleNewFromData(nd, dims, typenum, data) \
    ((PyArrayObject *)PyArray_New( \
        &PyArray_Type, (nd), (dims), (typenum), NULL, (data), 0, 0, NULL))
#endif

#ifndef PyArray_FromObject
#define PyArray_FromObject(obj, type, min_depth, max_depth) \
    PyArray_FromAny((obj), PyArray_DescrFromType((type)), (min_depth), (max_depth), 0, NULL)
#endif

#ifndef PyArray_Cast
#define PyArray_Cast(arr, typenum) \
    ((PyArrayObject *)PyArray_CastToType( \
        (PyArrayObject *)(arr), PyArray_DescrFromType((typenum)), 0))
#endif

#ifndef PyArray_CopyFromObject
#define PyArray_CopyFromObject(obj, typenum, min_depth, max_depth) \
    PyArray_FromAny( \
        (obj), PyArray_DescrFromType((typenum)), (min_depth), (max_depth), NPY_ARRAY_ENSURECOPY, NULL)
#endif

#ifndef PyArray_ZEROS
#define PyArray_ZEROS(nd, dims, typenum, fortran) \
    PyArray_Zeros((nd), (dims), PyArray_DescrFromType((typenum)), (fortran))
#endif

#ifndef PyArray_GETPTR1
#define PyArray_GETPTR1(arr, i) \
    ((void *)(PyArray_BYTES(arr) + (npy_intp)(i) * PyArray_STRIDE((arr), 0)))
#endif

#ifndef PyArray_GETPTR2
#define PyArray_GETPTR2(arr, i, j) \
    ((void *)(PyArray_BYTES(arr) + \
              (npy_intp)(i) * PyArray_STRIDE((arr), 0) + \
              (npy_intp)(j) * PyArray_STRIDE((arr), 1)))
#endif

#ifndef PyArray_ISBEHAVED
#define PyArray_ISBEHAVED(arr) PyArray_CHKFLAGS((arr), NPY_ARRAY_BEHAVED)
#endif

#ifndef PyDimMem_FREE
#define PyDimMem_FREE(ptr) PyMem_Free((ptr))
#endif

#ifndef Py_DTSF_ADD_DOT_0
#define Py_DTSF_ADD_DOT_0 0x02
#endif

#ifndef PyErr_SetFromWindowsErr
#define PyErr_SetFromWindowsErr(err) ((void)(err), PyErr_SetFromErrno(PyExc_OSError))
#endif

#ifndef PyUnicode_DecodeLatin1
#define PyUnicode_DecodeLatin1(data, size, errors) \
    ((void)(errors), PyUnicode_FromStringAndSize((const char *)(data), (Py_ssize_t)(size)))
#endif

#ifndef PyUnicode_EncodeFSDefault
#define PyUnicode_EncodeFSDefault(unicode) \
    PyUnicode_AsEncodedString((unicode), "utf-8", "strict")
#endif

#ifndef PyUnicode_AsWideCharString
static inline wchar_t *_molt_pyunicode_as_wide_char_string_compat(
    PyObject *unicode,
    Py_ssize_t *size_out
) {
    Py_ssize_t len = 0;
    const char *text = PyUnicode_AsUTF8AndSize(unicode, &len);
    wchar_t *out;
    Py_ssize_t i;
    if (text == NULL) {
        return NULL;
    }
    out = (wchar_t *)PyMem_Malloc((size_t)(len + 1) * sizeof(wchar_t));
    if (out == NULL) {
        PyErr_NoMemory();
        return NULL;
    }
    for (i = 0; i < len; i++) {
        out[i] = (wchar_t)(unsigned char)text[i];
    }
    out[len] = L'\0';
    if (size_out != NULL) {
        *size_out = len;
    }
    return out;
}
#define PyUnicode_AsWideCharString(unicode, size_out) \
    _molt_pyunicode_as_wide_char_string_compat((unicode), (size_out))
#endif

#ifndef PyUnicode_ReadChar
static inline Py_UCS4 _molt_pyunicode_read_char_compat(PyObject *unicode, Py_ssize_t index) {
    Py_ssize_t len = 0;
    const char *text = PyUnicode_AsUTF8AndSize(unicode, &len);
    if (text == NULL) {
        return (Py_UCS4)-1;
    }
    if (index < 0 || index >= len) {
        PyErr_SetString(PyExc_IndexError, "string index out of range");
        return (Py_UCS4)-1;
    }
    return (Py_UCS4)(unsigned char)text[index];
}
#define PyUnicode_ReadChar(unicode, index) \
    _molt_pyunicode_read_char_compat((unicode), (index))
#endif

#ifndef PyOS_double_to_string
static inline char *_molt_pyos_double_to_string_compat(
    double value,
    char format_code,
    int precision,
    int flags,
    int *type
) {
    char fmt[32];
    char stack_buf[128];
    int needed;
    char *heap_buf;
    (void)flags;
    if (type != NULL) {
        *type = 0;
    }
    if (precision < 0) {
        precision = 17;
    }
    if (format_code == '\0') {
        format_code = 'g';
    }
    (void)snprintf(fmt, sizeof(fmt), "%%.%d%c", precision, format_code);
    needed = snprintf(stack_buf, sizeof(stack_buf), fmt, value);
    if (needed < 0) {
        PyErr_SetString(PyExc_ValueError, "failed to format floating-point value");
        return NULL;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        heap_buf = (char *)PyMem_Malloc((size_t)needed + 1);
        if (heap_buf == NULL) {
            PyErr_NoMemory();
            return NULL;
        }
        memcpy(heap_buf, stack_buf, (size_t)needed + 1);
        return heap_buf;
    }
    heap_buf = (char *)PyMem_Malloc((size_t)needed + 1);
    if (heap_buf == NULL) {
        PyErr_NoMemory();
        return NULL;
    }
    (void)snprintf(heap_buf, (size_t)needed + 1, fmt, value);
    return heap_buf;
}
#define PyOS_double_to_string(value, format_code, precision, flags, type) \
    _molt_pyos_double_to_string_compat( \
        (value), (format_code), (precision), (flags), (type))
#endif

#endif
