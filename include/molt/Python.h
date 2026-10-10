/*
 * Python.h — Molt's canonical CPython-compatible C API header.
 *
 * THIS IS THE HEADER TO USE for compiling C extensions against Molt.
 *
 *   cc -O2 -shared -fPIC -I include myext.c -o _myext.so
 *
 * CPython-layout objects, numeric scalars, module definitions, and C-callable
 * entry points are shared with the linked ABI header. This file adds
 * source-transport helpers only; it does not define a second representation.
 */
#ifndef MOLT_C_API_PYTHON_H
#define MOLT_C_API_PYTHON_H

#include <assert.h>
#include <stdarg.h>
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <molt/molt.h>
#include "shared/_c_data_model.h"
#include "shared/_c_api_linkage.h"

#ifdef __cplusplus
extern "C" {
#endif

// Direct runtime intrinsics for zero-overhead C API dispatch
extern uint64_t molt_add(uint64_t, uint64_t);
extern uint64_t molt_sub(uint64_t, uint64_t);
extern uint64_t molt_mul(uint64_t, uint64_t);
extern uint64_t molt_mod(uint64_t, uint64_t);
extern uint64_t molt_pow(uint64_t, uint64_t);
extern uint64_t molt_div(uint64_t, uint64_t);
extern uint64_t molt_floordiv(uint64_t, uint64_t);
extern uint64_t molt_neg(uint64_t);
extern uint64_t molt_invert(uint64_t);
extern uint64_t molt_abs_builtin(uint64_t);
extern uint64_t molt_lshift(uint64_t, uint64_t);
extern uint64_t molt_rshift(uint64_t, uint64_t);
extern uint64_t molt_bit_and(uint64_t, uint64_t);
extern uint64_t molt_bit_or(uint64_t, uint64_t);
extern uint64_t molt_bit_xor(uint64_t, uint64_t);
extern uint64_t molt_matmul(uint64_t, uint64_t);
extern uint64_t molt_lt(uint64_t, uint64_t);
extern uint64_t molt_contains(uint64_t, uint64_t);
extern uint64_t molt_divmod_builtin(uint64_t, uint64_t);
extern uint64_t molt_inplace_add(uint64_t, uint64_t);
extern uint64_t molt_inplace_sub(uint64_t, uint64_t);
extern uint64_t molt_inplace_mul(uint64_t, uint64_t);
extern uint64_t molt_inplace_div(uint64_t, uint64_t);
extern uint64_t molt_inplace_floordiv(uint64_t, uint64_t);
extern uint64_t molt_inplace_mod(uint64_t, uint64_t);
extern uint64_t molt_inplace_lshift(uint64_t, uint64_t);
extern uint64_t molt_inplace_rshift(uint64_t, uint64_t);
extern uint64_t molt_inplace_bit_and(uint64_t, uint64_t);
extern uint64_t molt_inplace_bit_or(uint64_t, uint64_t);
extern uint64_t molt_inplace_bit_xor(uint64_t, uint64_t);
extern uint64_t molt_inplace_matmul(uint64_t, uint64_t);
// Borrowed reference API (zero refcount overhead)
#ifndef MOLT_EXTENSION_HOST_ABI
extern uint64_t molt_dict_getitem_borrowed(uint64_t, uint64_t);
#endif
extern uint64_t molt_list_getitem_borrowed(uint64_t, uint64_t);
extern uint64_t molt_tuple_getitem_borrowed(uint64_t, uint64_t);

#ifdef MOLT_EXTENSION_HOST_ABI
#define molt_add ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_add"))
#define molt_sub ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_sub"))
#define molt_mul ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_mul"))
#define molt_mod ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_mod"))
#define molt_pow ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_pow"))
#define molt_div ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_div"))
#define molt_floordiv ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_floordiv"))
#define molt_neg ((uint64_t (*)(uint64_t))_molt_host_abi_symbol("molt_neg"))
#define molt_invert ((uint64_t (*)(uint64_t))_molt_host_abi_symbol("molt_invert"))
#define molt_abs_builtin ((uint64_t (*)(uint64_t))_molt_host_abi_symbol("molt_abs_builtin"))
#define molt_lshift ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_lshift"))
#define molt_rshift ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_rshift"))
#define molt_bit_and ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_bit_and"))
#define molt_bit_or ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_bit_or"))
#define molt_bit_xor ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_bit_xor"))
#define molt_matmul ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_matmul"))
#define molt_lt ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_lt"))
#define molt_contains ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_contains"))
#define molt_divmod_builtin ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_divmod_builtin"))
#define molt_inplace_add ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_add"))
#define molt_inplace_sub ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_sub"))
#define molt_inplace_mul ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_mul"))
#define molt_inplace_div ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_div"))
#define molt_inplace_floordiv ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_floordiv"))
#define molt_inplace_mod ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_mod"))
#define molt_inplace_lshift ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_lshift"))
#define molt_inplace_rshift ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_rshift"))
#define molt_inplace_bit_and ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_bit_and"))
#define molt_inplace_bit_or ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_bit_or"))
#define molt_inplace_bit_xor ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_bit_xor"))
#define molt_inplace_matmul ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_inplace_matmul"))
#define molt_dict_getitem_borrowed ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_dict_getitem_borrowed"))
#define molt_list_getitem_borrowed ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_list_getitem_borrowed"))
#define molt_tuple_getitem_borrowed ((uint64_t (*)(uint64_t, uint64_t))_molt_host_abi_symbol("molt_tuple_getitem_borrowed"))
#endif

typedef intptr_t Py_ssize_t;
typedef Py_ssize_t Py_hash_t;
typedef intptr_t Py_intptr_t;
typedef uintptr_t Py_uintptr_t;
typedef uintptr_t Py_uhash_t;
typedef uint32_t digit;
typedef struct {
    double real;
    double imag;
} Py_complex;
#include "shared/_numeric_scalar_abi.h"
#include "shared/_gil_state_abi.h"

/* Source transport treats type objects as opaque identities. Numeric scalar
 * type identities are the canonical linked ABI symbols declared below. */
struct _typeobject {
    PyObject ob_base;
};
typedef struct {
    PyTypeObject ht_type;
} PyHeapTypeObject;
typedef uint32_t Py_UCS4;
typedef uint8_t Py_UCS1;
typedef uint16_t Py_UCS2;
#include "shared/_buffer_abi.h"

/* Buffer protocol flags. Keep these before PyObject_GetBuffer so header users
   cannot compile a policy-less buffer acquisition path. */
#ifndef PyBUF_SIMPLE
#define PyBUF_SIMPLE 0
#endif
#ifndef PyBUF_WRITABLE
#define PyBUF_WRITABLE 0x0001
#endif
#ifndef PyBUF_WRITEABLE
#define PyBUF_WRITEABLE PyBUF_WRITABLE
#endif
#ifndef PyBUF_READ
#define PyBUF_READ 0x0100
#endif
#ifndef PyBUF_WRITE
#define PyBUF_WRITE 0x0200
#endif
#ifndef PyBUF_FORMAT
#define PyBUF_FORMAT 0x0004
#endif
#ifndef PyBUF_ND
#define PyBUF_ND 0x0008
#endif
#ifndef PyBUF_STRIDES
#define PyBUF_STRIDES (0x0010 | PyBUF_ND)
#endif
#ifndef PyBUF_C_CONTIGUOUS
#define PyBUF_C_CONTIGUOUS (0x0020 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_F_CONTIGUOUS
#define PyBUF_F_CONTIGUOUS (0x0040 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_ANY_CONTIGUOUS
#define PyBUF_ANY_CONTIGUOUS (0x0080 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_INDIRECT
#define PyBUF_INDIRECT (0x0100 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_CONTIG_RO
#define PyBUF_CONTIG_RO PyBUF_ND
#endif
#ifndef PyBUF_CONTIG
#define PyBUF_CONTIG (PyBUF_ND | PyBUF_WRITABLE)
#endif
#ifndef PyBUF_RECORDS_RO
#define PyBUF_RECORDS_RO (PyBUF_STRIDES | PyBUF_FORMAT)
#endif
#ifndef PyBUF_RECORDS
#define PyBUF_RECORDS (PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE)
#endif
#ifndef PyBUF_FULL_RO
#define PyBUF_FULL_RO (PyBUF_INDIRECT | PyBUF_FORMAT)
#endif
#ifndef PyBUF_FULL
#define PyBUF_FULL (PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE)
#endif
#define _MOLT_PYBUF_C_CONTIGUOUS_BIT 0x0020
#define _MOLT_PYBUF_F_CONTIGUOUS_BIT 0x0040
#define _MOLT_PYBUF_ANY_CONTIGUOUS_BIT 0x0080

typedef struct _molt_pyinterpreterstate {
    int _molt_reserved;
} PyInterpreterState;

typedef struct _molt_pyerr_stackitem {
    PyObject *exc_type;
    PyObject *exc_value;
    PyObject *exc_traceback;
    struct _molt_pyerr_stackitem *previous_item;
} _PyErr_StackItem;

typedef struct _molt_pythreadstate {
    PyInterpreterState *interp;
    PyObject *current_exception;
    _PyErr_StackItem *exc_info;
    _PyErr_StackItem exc_state;
    int _molt_reserved;
} PyThreadState;

typedef void (*PyCapsule_Destructor)(PyObject *);

typedef PyObject *(*getter)(PyObject *, void *);
typedef int (*setter)(PyObject *, PyObject *, void *);
typedef Py_ssize_t (*lenfunc)(PyObject *);
typedef PyObject *(*binaryfunc)(PyObject *, PyObject *);
typedef int (*objobjargproc)(PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)(PyObject *, PyObject *);
typedef PyObject *(*vectorcallfunc)(PyObject *callable, PyObject *const *args,
                                    size_t nargsf, PyObject *kwnames);
#include "shared/_cfunction_abi.h"

typedef struct PyMappingMethods {
    lenfunc mp_length;
    binaryfunc mp_subscript;
    objobjargproc mp_ass_subscript;
} PyMappingMethods;

typedef struct PySequenceMethods {
    lenfunc sq_length;
    binaryfunc sq_concat;
    binaryfunc sq_repeat;
    PyObject *(*sq_item)(PyObject *, Py_ssize_t);
    void *was_sq_slice;
    int (*sq_ass_item)(PyObject *, Py_ssize_t, PyObject *);
    void *was_sq_ass_slice;
    objobjproc sq_contains;
    binaryfunc sq_inplace_concat;
    binaryfunc sq_inplace_repeat;
} PySequenceMethods;

typedef struct PyNumberMethods {
    binaryfunc nb_add;
    binaryfunc nb_subtract;
    binaryfunc nb_multiply;
} PyNumberMethods;

typedef struct PyBufferProcs {
    int (*bf_getbuffer)(PyObject *, Py_buffer *, int);
    void (*bf_releasebuffer)(PyObject *, Py_buffer *);
} PyBufferProcs;

typedef PyObject PyBytesObject;
#include "shared/_sequence_abi.h"
typedef PyObject PyDictProxyObject;

typedef struct PyMutex {
    int _molt_reserved;
} PyMutex;

typedef PyMutex *PyThread_type_lock;

typedef struct PyCriticalSection {
    PyObject *object;
} PyCriticalSection;

#include "shared/_module_definition_abi.h"

#include "shared/_module_callable_exports.h"
#include "shared/_exception_attribute_exports.h"

/* Host-loaded source extensions resolve the same linkable CPython ABI image
 * that owns the bridge. Missing symbols fail closed in _molt_host_abi_symbol;
 * no inline module or callable implementation is a fallback. */
#ifdef MOLT_EXTENSION_HOST_ABI
#define PyModule_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyModule_Type"))
#define PyModuleDef_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyModuleDef_Type"))
#define PyCFunction_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyCFunction_Type"))
#define PyCMethod_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyCMethod_Type"))
#define PyModule_New ((PyObject *(*)(const char *))_molt_host_abi_symbol("PyModule_New"))
#define PyModule_NewObject ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyModule_NewObject"))
#define PyModule_Create2 ((PyObject *(*)(PyModuleDef *, int))_molt_host_abi_symbol("PyModule_Create2"))
#define PyModuleDef_Init ((PyObject *(*)(PyModuleDef *))_molt_host_abi_symbol("PyModuleDef_Init"))
#define PyModule_FromDefAndSpec2 ((PyObject *(*)(PyModuleDef *, PyObject *, int))_molt_host_abi_symbol("PyModule_FromDefAndSpec2"))
#define PyModule_FromDefAndSpec ((PyObject *(*)(PyModuleDef *, PyObject *))_molt_host_abi_symbol("PyModule_FromDefAndSpec"))
#define PyModule_ExecDef ((int (*)(PyObject *, PyModuleDef *))_molt_host_abi_symbol("PyModule_ExecDef"))
#define PyUnstable_Module_SetGIL ((int (*)(PyObject *, void *))_molt_host_abi_symbol("PyUnstable_Module_SetGIL"))
#define PyModule_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyModule_Check"))
#define PyModule_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyModule_CheckExact"))
#define PyModule_GetDict ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetDict"))
#define PyModule_GetDef ((PyModuleDef *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetDef"))
#define PyModule_GetState ((void *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetState"))
#define PyModule_GetName ((const char *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetName"))
#define PyModule_GetNameObject ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetNameObject"))
#define PyModule_GetFilename ((const char *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetFilename"))
#define PyModule_GetFilenameObject ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyModule_GetFilenameObject"))
#define PyModule_SetDocString ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyModule_SetDocString"))
#define PyModule_GetObject ((PyObject *(*)(PyObject *, const char *))_molt_host_abi_symbol("PyModule_GetObject"))
#define PyModule_AddFunctions ((int (*)(PyObject *, PyMethodDef *))_molt_host_abi_symbol("PyModule_AddFunctions"))
#define PyModule_AddObjectRef ((int (*)(PyObject *, const char *, PyObject *))_molt_host_abi_symbol("PyModule_AddObjectRef"))
#define PyModule_AddObject ((int (*)(PyObject *, const char *, PyObject *))_molt_host_abi_symbol("PyModule_AddObject"))
#define PyModule_Add ((int (*)(PyObject *, const char *, PyObject *))_molt_host_abi_symbol("PyModule_Add"))
#define PyModule_AddType ((int (*)(PyObject *, PyTypeObject *))_molt_host_abi_symbol("PyModule_AddType"))
#define PyModule_AddIntConstant ((int (*)(PyObject *, const char *, long))_molt_host_abi_symbol("PyModule_AddIntConstant"))
#define PyModule_AddStringConstant ((int (*)(PyObject *, const char *, const char *))_molt_host_abi_symbol("PyModule_AddStringConstant"))
#define PyState_AddModule ((int (*)(PyObject *, PyModuleDef *))_molt_host_abi_symbol("PyState_AddModule"))
#define PyState_FindModule ((PyObject *(*)(PyModuleDef *))_molt_host_abi_symbol("PyState_FindModule"))
#define PyState_RemoveModule ((int (*)(PyModuleDef *))_molt_host_abi_symbol("PyState_RemoveModule"))
#define PyMethod_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyMethod_Type"))
#define PyMethod_New ((PyObject *(*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyMethod_New"))
#define PyMethod_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyMethod_Check"))
#define PyMethod_GET_FUNCTION ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyMethod_GET_FUNCTION"))
#define PyMethod_GET_SELF ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyMethod_GET_SELF"))
#define PyCFunction_New ((PyObject *(*)(PyMethodDef *, PyObject *))_molt_host_abi_symbol("PyCFunction_New"))
#define PyCFunction_NewEx ((PyObject *(*)(PyMethodDef *, PyObject *, PyObject *))_molt_host_abi_symbol("PyCFunction_NewEx"))
#define PyCMethod_New ((PyObject *(*)(PyMethodDef *, PyObject *, PyObject *, PyTypeObject *))_molt_host_abi_symbol("PyCMethod_New"))
#define PyCFunction_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyCFunction_Check"))
#define PyCFunction_GetFunction ((PyCFunction (*)(PyObject *))_molt_host_abi_symbol("PyCFunction_GetFunction"))
#define PyCFunction_GetSelf ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyCFunction_GetSelf"))
#define PyCFunction_GetFlags ((int (*)(PyObject *))_molt_host_abi_symbol("PyCFunction_GetFlags"))
#endif

#include "shared/_type_spec_abi.h"

#include "shared/_descriptor_abi.h"
#include "shared/_descriptor_exports.h"

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyMethodDescr_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyMethodDescr_Type"))
#define PyClassMethodDescr_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyClassMethodDescr_Type"))
#define PyMemberDescr_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyMemberDescr_Type"))
#define PyGetSetDescr_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyGetSetDescr_Type"))
#define PyWrapperDescr_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyWrapperDescr_Type"))
#define _PyMethodWrapper_Type (*(PyTypeObject *)_molt_host_abi_symbol("_PyMethodWrapper_Type"))
#define PyClassMethod_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyClassMethod_Type"))
#define PyStaticMethod_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyStaticMethod_Type"))
#define PyDescr_IsData ((int (*)(PyObject *))_molt_host_abi_symbol("PyDescr_IsData"))
#define PyDescr_NAME ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyDescr_NAME"))
#define PyDescr_NewMethod ((PyObject *(*)(PyTypeObject *, PyMethodDef *))_molt_host_abi_symbol("PyDescr_NewMethod"))
#define PyDescr_NewClassMethod ((PyObject *(*)(PyTypeObject *, PyMethodDef *))_molt_host_abi_symbol("PyDescr_NewClassMethod"))
#define PyDescr_NewMember ((PyObject *(*)(PyTypeObject *, PyMemberDef *))_molt_host_abi_symbol("PyDescr_NewMember"))
#define PyDescr_NewGetSet ((PyObject *(*)(PyTypeObject *, PyGetSetDef *))_molt_host_abi_symbol("PyDescr_NewGetSet"))
#define PyDescr_NewWrapper ((PyObject *(*)(PyTypeObject *, struct wrapperbase *, void *))_molt_host_abi_symbol("PyDescr_NewWrapper"))
#define PyWrapper_New ((PyObject *(*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyWrapper_New"))
#define PyMember_GetOne ((PyObject *(*)(const char *, PyMemberDef *))_molt_host_abi_symbol("PyMember_GetOne"))
#define PyMember_SetOne ((int (*)(char *, PyMemberDef *, PyObject *))_molt_host_abi_symbol("PyMember_SetOne"))
#define PyClassMethod_New ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyClassMethod_New"))
#define PyStaticMethod_New ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyStaticMethod_New"))
#endif

extern const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out);
extern PyObject *PyUnicode_AsEncodedString(PyObject *unicode,
                                                    const char *encoding,
                                                    const char *errors);
#include "shared/_typeobject_exports.h"
static inline PyObject *_molt_builtin_class_lookup_utf8(const char *name);
PyAPI_DATA(PyTypeObject) PyType_Type;
PyAPI_DATA(PyTypeObject) PyTuple_Type;
extern PyObject *PyErr_Format(PyObject *exc, const char *fmt, ...);
extern PyObject *PyErr_FormatV(PyObject *exc, const char *fmt, va_list vargs);
extern PyObject *PyUnicode_FromFormat(const char *format, ...);
extern PyObject *PyUnicode_FromFormatV(const char *format, va_list vargs);
extern PyObject *PyUnicode_FromString(const char *value);
extern const char *PyUnicode_AsUTF8(PyObject *value);
extern int PyUnicode_Check(PyObject *obj);
static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size);
extern PyObject *PyLong_FromLong(long value);
extern PyObject *PyLong_FromLongLong(long long value);
extern PyObject *PyLong_FromSsize_t(Py_ssize_t value);
extern PyObject *PyLong_FromSize_t(size_t value);
extern PyObject *PyLong_FromUnsignedLong(unsigned long value);
extern PyObject *PyLong_FromUnsignedLongLong(unsigned long long value);
extern PyObject *PyLong_FromDouble(double value);
extern PyObject *PyLong_FromString(const char *str, char **pend, int base);
extern PyObject *PyLong_FromUnicodeObject(PyObject *unicode, int base);
extern PyObject *PyLong_FromVoidPtr(void *ptr);
extern long PyLong_AsLong(PyObject *obj);
extern long long PyLong_AsLongLong(PyObject *obj);
extern long long PyLong_AsLongLongAndOverflow(PyObject *obj, int *overflow);
extern Py_ssize_t PyLong_AsSsize_t(PyObject *obj);
extern size_t PyLong_AsSize_t(PyObject *obj);
extern unsigned long PyLong_AsUnsignedLong(PyObject *obj);
extern unsigned long long PyLong_AsUnsignedLongLong(PyObject *obj);
extern unsigned long PyLong_AsUnsignedLongMask(PyObject *obj);
extern unsigned long long PyLong_AsUnsignedLongLongMask(PyObject *obj);
extern void *PyLong_AsVoidPtr(PyObject *obj);
extern double PyLong_AsDouble(PyObject *obj);
extern int PyLong_AsInt(PyObject *obj);
extern int PyLong_Check(PyObject *obj);
extern int PyUnstable_Long_IsCompact(const PyLongObject *obj);
extern Py_ssize_t PyUnstable_Long_CompactValue(const PyLongObject *obj);
extern size_t _PyLong_NumBits(PyObject *obj);
extern int _PyLong_Sign(PyObject *obj);
extern int _PyLong_Size_t_Converter(PyObject *obj, void *out);
extern int _PyLong_UnsignedShort_Converter(PyObject *obj, void *out);
extern int _PyLong_UnsignedInt_Converter(PyObject *obj, void *out);
extern int _PyLong_UnsignedLong_Converter(PyObject *obj, void *out);
extern int _PyLong_UnsignedLongLong_Converter(PyObject *obj, void *out);
extern PyObject *_PyLong_FromByteArray(const unsigned char *bytes, size_t n, int little_endian, int is_signed);
extern int _PyLong_AsByteArray(PyLongObject *obj, unsigned char *bytes, size_t n, int little_endian, int is_signed);
extern PyObject *PyLong_GetInfo(void);
static inline PyObject *PyNumber_Long(PyObject *obj);
#include "shared/_sequence_exports.h"
#include "shared/_call_exports.h"
#include "shared/_container_exports.h"
static inline double PyOS_string_to_double(
    const char *text, char **endptr, PyObject *overflow_exception);
static inline PyObject *PyImport_ImportModule(const char *name);
#ifdef MOLT_EXTENSION_HOST_ABI
#define PyImport_GetModuleDict ((PyObject *(*)(void))_molt_host_abi_symbol("PyImport_GetModuleDict"))
#define PyEval_GetBuiltins ((PyObject *(*)(void))_molt_host_abi_symbol("PyEval_GetBuiltins"))
#define PySys_GetObject ((PyObject *(*)(const char *))_molt_host_abi_symbol("PySys_GetObject"))
#else
/* Borrowed namespace results belong to the linked dictionary/frame authority. */
extern PyObject *PyImport_GetModuleDict(void);
extern PyObject *PyEval_GetBuiltins(void);
extern PyObject *PySys_GetObject(const char *name);
#endif
static inline void *PyCapsule_Import(const char *name, int no_block);
static inline PyThreadState *PyThreadState_Get(void);
static inline PyGILState_STATE PyGILState_Ensure(void);
static inline void PyGILState_Release(PyGILState_STATE state);

#ifndef PYTHON_API_VERSION
#define PYTHON_API_VERSION 1013
#endif

#define Py_LT 0
#define Py_LE 1
#define Py_EQ 2
#define Py_NE 3
#define Py_GT 4
#define Py_GE 5

#define Py_TPFLAGS_DEFAULT 0UL
#define Py_TPFLAGS_BASETYPE (1UL << 10)
#define Py_TPFLAGS_HAVE_VECTORCALL (1UL << 11)
#define _Py_TPFLAGS_HAVE_VECTORCALL Py_TPFLAGS_HAVE_VECTORCALL
#define Py_TPFLAGS_HAVE_GC (1UL << 14)
#define Py_TPFLAGS_METHOD_DESCRIPTOR (1UL << 17)
#define Py_TPFLAGS_HEAPTYPE (1UL << 9)
#define Py_TPFLAGS_LONG_SUBCLASS (1UL << 24)
#define Py_TPFLAGS_READY (1UL << 12)

#define Py_SUCCESS 0
#define Py_FAILURE -1

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0
#define Py_USING_UNICODE 1

#define PyOS_snprintf snprintf

#define NOWAIT_LOCK 0
#define WAIT_LOCK 1

#define Py_MOD_GIL_USED ((void *)0)
#define Py_MOD_GIL_NOT_USED ((void *)1)
#define Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED ((void *)0)
#define Py_MOD_MULTIPLE_INTERPRETERS_SUPPORTED ((void *)1)
#define Py_MOD_PER_INTERPRETER_GIL_SUPPORTED ((void *)2)
#define Py_mod_create 1
#define Py_mod_exec 2
#define Py_mod_multiple_interpreters 3
#define Py_mod_gil 4

#define Py_CLEANUP_SUPPORTED 0x20000

/* Both header transports observe the runtime-owned process flag. */
PyAPI_DATA(int) Py_OptimizeFlag;

#ifndef Py_LIMITED_API
#define Py_LIMITED_API 0x030C0000
#endif

#ifndef PyAPI_FUNC
#define PyAPI_FUNC(RTYPE) RTYPE
#endif

#ifndef PyMODINIT_FUNC
#include "shared/_module_init_export.h"
#define PyMODINIT_FUNC MOLT_PYMODINIT_FUNC
#endif

#if SIZEOF_VOID_P > 4
#define _Py_IMMORTAL_REFCNT ((Py_ssize_t)UINT32_MAX)
#else
#define _Py_IMMORTAL_REFCNT ((Py_ssize_t)(UINT32_MAX >> 2))
#endif
#define PyObject_HEAD_INIT(type) _Py_IMMORTAL_REFCNT, (type),
#define PyVarObject_HEAD_INIT(type, size) PyObject_HEAD_INIT(type) (size),

PyAPI_DATA(PyTypeObject) PyDictProxy_Type;

#if defined(__GNUC__) || defined(__clang__)
#define Py_UNUSED(name) name __attribute__((unused))
#else
#define Py_UNUSED(name) name
#endif

typedef void (*_MoltCHeapDealloc)(PyObject *);

typedef struct _MoltCHeapObject {
    uint64_t magic;
    uint32_t refcnt;
    uint32_t kind;
    PyTypeObject *type;
    _MoltCHeapDealloc dealloc;
} _MoltCHeapObject;

#define _MOLT_C_HEAP_MAGIC UINT64_C(0x4d4f4c54434f424a)
#define _MOLT_C_HEAP_REFCNT_IMMORTAL UINT32_MAX

static inline int _molt_c_heap_object_is(const PyObject *obj) {
    return obj != NULL && molt_c_heap_contains((uintptr_t)obj) != 0;
}

static inline void _molt_c_heap_fatal(const char *message) {
    fprintf(stderr, "Fatal Python error: %s\n", message != NULL ? message : "(null)");
    abort();
}

static inline _MoltCHeapObject *_molt_c_heap_header_from_object(const PyObject *obj) {
    return (_MoltCHeapObject *)obj;
}

static inline PyObject *_molt_c_heap_object_from_header(_MoltCHeapObject *header) {
    return (PyObject *)header;
}

static inline void *_molt_c_heap_payload_maybe(const void *obj) {
    const PyObject *pyobj = (const PyObject *)obj;
    if (_molt_c_heap_object_is(pyobj)) {
        return (void *)_molt_c_heap_header_from_object(pyobj);
    }
    return (void *)obj;
}

static inline void _molt_c_heap_init(
    _MoltCHeapObject *header,
    uint32_t kind,
    PyTypeObject *type,
    _MoltCHeapDealloc dealloc
) {
    header->magic = _MOLT_C_HEAP_MAGIC;
    header->refcnt = 1;
    header->kind = kind;
    header->type = type;
    header->dealloc = dealloc;
    (void)molt_c_heap_register((uintptr_t)header);
}

static inline PyObject *_molt_c_heap_static_type_init(
    _MoltCHeapObject *header,
    uint32_t kind
) {
    uintptr_t canonical;
    header->magic = _MOLT_C_HEAP_MAGIC;
    header->refcnt = _MOLT_C_HEAP_REFCNT_IMMORTAL;
    header->kind = kind;
    header->type = (PyTypeObject *)_molt_c_heap_object_from_header(header);
    header->dealloc = NULL;
    canonical = molt_c_heap_type_canonicalize(kind, (uintptr_t)header);
    return canonical != 0 ? (PyObject *)canonical : _molt_c_heap_object_from_header(header);
}

extern MoltHandle molt_capi_pyobj_to_handle(PyObject *obj);
extern PyObject *molt_capi_handle_to_pyobj(MoltHandle bits);
extern PyObject *molt_capi_handle_to_borrowed_pyobj(MoltHandle bits);
extern PyObject *molt_capi_result_to_pyobj(MoltHandle bits);
extern int molt_capi_pyobj_is_bridge_managed(PyObject *obj);
extern void molt_capi_any_incref(PyObject *obj);
extern void molt_capi_any_decref(PyObject *obj);
extern void molt_capi_set_refcnt(PyObject *obj, Py_ssize_t refcnt);
extern PyTypeObject *molt_capi_semantic_type(PyObject *obj);
extern int molt_capi_set_semantic_type(PyObject *obj, PyTypeObject *type_obj);
extern int PyUnstable_Object_IsUniqueReferencedTemporary(PyObject *obj);
extern int PyUnstable_Object_IsUniquelyReferenced(PyObject *obj);
extern int PyUnstable_Object_EnableDeferredRefcount(PyObject *obj);
extern int PyUnstable_SetImmortal(PyObject *obj);
PyAPI_DATA(PyTypeObject) MoltManaged_Type;
extern void _Py_Dealloc(PyObject *obj);

#ifdef MOLT_EXTENSION_HOST_ABI
#define molt_capi_set_refcnt ((void (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("molt_capi_set_refcnt"))
#endif

static inline MoltHandle _molt_py_handle(const PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        return 0;
    }
    return molt_capi_pyobj_to_handle((PyObject *)obj);
}

static inline PyObject *_molt_pyobject_from_borrowed_handle(MoltHandle bits) {
    return molt_capi_handle_to_borrowed_pyobj(bits);
}

static inline PyObject *_molt_pyobject_from_result(MoltHandle bits) {
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return molt_capi_result_to_pyobj(bits);
}

static inline MoltHandle _molt_string_from_utf8(const char *text) {
    if (text == NULL) {
        return 0;
    }
    return molt_string_from((const uint8_t *)text, (uint64_t)strlen(text));
}

static inline size_t _molt_strnlen(const char *text, size_t limit) {
    size_t n = 0;
    if (text == NULL) {
        return 0;
    }
    while (n < limit && text[n] != '\0') {
        n++;
    }
    return n;
}

/* Builtin exception symbols use the shared linked/host data exports above. */

#define PyExceptionInstance_Class(obj) ((PyObject *)Py_TYPE(obj))
/* ---------------------------------------------------- */

static inline double PyOS_string_to_double(
    const char *text,
    char **endptr,
    PyObject *overflow_exception
) {
    char *local_end = NULL;
    double value;
    if (endptr != NULL) {
        *endptr = NULL;
    }
    if (text == NULL) {
        PyErr_SetString(PyExc_TypeError, "text must not be NULL");
        return -1.0;
    }
    errno = 0;
    value = strtod(text, &local_end);
    if (endptr != NULL) {
        *endptr = local_end;
    }
    if (local_end == text) {
        PyErr_SetString(PyExc_ValueError, "could not convert string to float");
        return -1.0;
    }
    if (errno == ERANGE) {
        if (overflow_exception != NULL) {
            PyErr_SetString(overflow_exception, "floating-point conversion overflow");
        } else {
            PyErr_SetString(PyExc_OverflowError, "floating-point conversion overflow");
        }
        return -1.0;
    }
    return value;
}

static inline int Py_IsInitialized(void) {
    return 1;
}

static inline void Py_Initialize(void) {
    (void)molt_init();
}

static inline void Py_Finalize(void) {
    (void)molt_shutdown();
}

static inline PyThreadState *PyThreadState_Get(void) {
    static PyThreadState state = {0};
    if (molt_gil_is_held() == 0) {
        PyErr_SetString(PyExc_RuntimeError, "PyThreadState_Get requires the GIL");
        return NULL;
    }
    return &state;
}

static inline PyGILState_STATE PyGILState_Ensure(void) {
    PyGILState_STATE state = molt_gil_is_held() != 0 ? PyGILState_LOCKED : PyGILState_UNLOCKED;
    if (state == PyGILState_UNLOCKED) {
        (void)molt_gil_acquire();
    }
    return state;
}

static inline void PyGILState_Release(PyGILState_STATE state) {
    if (state == PyGILState_UNLOCKED) {
        (void)molt_gil_release();
    }
}

static inline void Py_IncRef(PyObject *obj) {
    if (obj != NULL) {
        if (_molt_c_heap_object_is(obj)) {
            _MoltCHeapObject *header = _molt_c_heap_header_from_object(obj);
            if (header->magic == _MOLT_C_HEAP_MAGIC
                    && header->refcnt != _MOLT_C_HEAP_REFCNT_IMMORTAL) {
                header->refcnt++;
            }
            return;
        }
        molt_capi_any_incref(obj);
    }
}

static inline void Py_DecRef(PyObject *obj) {
    if (obj != NULL) {
        if (_molt_c_heap_object_is(obj)) {
            _MoltCHeapObject *header = _molt_c_heap_header_from_object(obj);
            if (header->magic == _MOLT_C_HEAP_MAGIC
                    && header->refcnt != _MOLT_C_HEAP_REFCNT_IMMORTAL
                    && header->refcnt > 0) {
                header->refcnt--;
                if (header->refcnt == 0) {
                    _MoltCHeapDealloc dealloc = header->dealloc;
                    header->magic = 0;
                    (void)molt_c_heap_unregister((uintptr_t)header);
                    if (dealloc != NULL) {
                        dealloc(obj);
                    } else {
                        free(header);
                    }
                }
            }
            return;
        }
        molt_capi_any_decref(obj);
    }
}

#define Py_INCREF(op) Py_IncRef((PyObject *)(op))
#define Py_DECREF(op) Py_DecRef((PyObject *)(op))
#define Py_XINCREF(op)                                                             \
    do {                                                                           \
        if ((op) != NULL) {                                                        \
            Py_INCREF((op));                                                       \
        }                                                                          \
    } while (0)
#define Py_XDECREF(op)                                                             \
    do {                                                                           \
        if ((op) != NULL) {                                                        \
            Py_DECREF((op));                                                       \
        }                                                                          \
    } while (0)
#define Py_CLEAR(op)                                                               \
    do {                                                                           \
        PyObject *_molt_tmp = (PyObject *)(op);                                    \
        (op) = NULL;                                                                \
        Py_XDECREF(_molt_tmp);                                                      \
    } while (0)
#define Py_SETREF(dst, src)                                                        \
    do {                                                                           \
        PyObject *_molt_tmp = (PyObject *)(dst);                                   \
        (dst) = (src);                                                              \
        Py_DECREF(_molt_tmp);                                                       \
    } while (0)
#define Py_XSETREF(dst, src)                                                       \
    do {                                                                           \
        PyObject *_molt_tmp = (PyObject *)(dst);                                   \
        (dst) = (src);                                                              \
        Py_XDECREF(_molt_tmp);                                                      \
    } while (0)

#define Py_None _molt_pyobject_from_borrowed_handle(molt_none())
PyAPI_DATA(PyLongObject) _Py_TrueStruct;
PyAPI_DATA(PyLongObject) _Py_FalseStruct;
#define Py_True ((PyObject *)&_Py_TrueStruct)
#define Py_False ((PyObject *)&_Py_FalseStruct)

PyAPI_DATA(PyObject) Py_NotImplementedSentinel;
PyAPI_DATA(PyObject) Py_EllipsisObject;
#define Py_NotImplemented (&Py_NotImplementedSentinel)
#define Py_Ellipsis (&Py_EllipsisObject)

static inline PyTypeObject *_molt_py_typeof(PyObject *obj) {
    if (obj == NULL) {
        return NULL;
    }
    if (_molt_c_heap_object_is(obj)) {
        _MoltCHeapObject *header = _molt_c_heap_header_from_object(obj);
        if (header->type == (PyTypeObject *)obj) {
            return &PyType_Type;
        }
        return header->type;
    }
    /* Physical carriers preserve layout; the bridge owns Python class identity. */
    return molt_capi_semantic_type(obj);
}

static inline void _molt_py_set_type(PyObject *obj, PyTypeObject *type_obj) {
    if (obj == NULL || type_obj == NULL) {
        return;
    }
    if (_molt_c_heap_object_is(obj)) {
        _molt_c_heap_header_from_object(obj)->type = type_obj;
        return;
    }
    (void)molt_capi_set_semantic_type(obj, type_obj);
}

static inline Py_ssize_t _molt_py_refcnt(PyObject *obj) {
    if (obj == NULL) {
        return 0;
    }
    if (_molt_c_heap_object_is(obj)) {
        return (Py_ssize_t)_molt_c_heap_header_from_object(obj)->refcnt;
    }
    return obj->ob_refcnt;
}

static inline void _molt_py_set_refcnt(PyObject *obj, Py_ssize_t refcnt) {
    if (obj == NULL) {
        return;
    }
    if (_molt_c_heap_object_is(obj)) {
        _MoltCHeapObject *header = _molt_c_heap_header_from_object(obj);
        if (header->refcnt != _MOLT_C_HEAP_REFCNT_IMMORTAL) {
            header->refcnt = (uint32_t)refcnt;
        }
        return;
    }
    molt_capi_set_refcnt(obj, refcnt);
}

#define Py_TYPE(ob) _molt_py_typeof((PyObject *)(ob))
#define Py_SET_TYPE(ob, type_obj) _molt_py_set_type((PyObject *)(ob), (PyTypeObject *)(type_obj))
#define Py_REFCNT(ob) _molt_py_refcnt((PyObject *)(ob))
#define Py_SET_REFCNT(ob, refcnt) _molt_py_set_refcnt((PyObject *)(ob), (Py_ssize_t)(refcnt))
#define PyThreadState_GET() PyThreadState_Get()
#define PyObject_New(type, typeobj) ((type *)_PyObject_New((PyTypeObject *)(typeobj)))

#define Py_RETURN_NONE                                                             \
    do {                                                                           \
        Py_INCREF(Py_None);                                                        \
        return Py_None;                                                            \
    } while (0)
#define Py_RETURN_TRUE                                                             \
    do {                                                                           \
        Py_INCREF(Py_True);                                                        \
        return Py_True;                                                            \
    } while (0)
#define Py_RETURN_FALSE                                                            \
    do {                                                                           \
        Py_INCREF(Py_False);                                                       \
        return Py_False;                                                           \
    } while (0)
#define Py_RETURN_NOTIMPLEMENTED                                                   \
    do {                                                                           \
        Py_INCREF(Py_NotImplemented);                                              \
        return Py_NotImplemented;                                                  \
    } while (0)

static inline PyObject *Py_NewRef(PyObject *obj) {
    Py_INCREF(obj);
    return obj;
}

static inline PyObject *Py_XNewRef(PyObject *obj) {
    Py_XINCREF(obj);
    return obj;
}

#define PyErr_WarnEx_noerr PyErr_WarnEx

extern void PyErr_WriteUnraisable(PyObject *obj);
extern void PyErr_FormatUnraisable(const char *format, ...);

static inline void *PyMem_Malloc(size_t size) {
    void *ptr = malloc(size == 0 ? (size_t)1 : size);
    if (ptr == NULL) {
        (void)PyErr_NoMemory();
    }
    return ptr;
}

static inline void *PyMem_Calloc(size_t nelem, size_t elsize) {
    void *ptr;
    if (nelem == 0 || elsize == 0) {
        nelem = 1;
        elsize = 1;
    }
    ptr = calloc(nelem, elsize);
    if (ptr == NULL) {
        (void)PyErr_NoMemory();
    }
    return ptr;
}

static inline void *PyMem_Realloc(void *ptr, size_t new_size) {
    void *out = realloc(ptr, new_size == 0 ? (size_t)1 : new_size);
    if (out == NULL) {
        (void)PyErr_NoMemory();
    }
    return out;
}

static inline void PyMem_Free(void *ptr) {
    free(ptr);
}

#define PyMem_RawMalloc PyMem_Malloc
#define PyMem_RawCalloc PyMem_Calloc
#define PyMem_RawRealloc PyMem_Realloc
#define PyMem_RawFree PyMem_Free
#define PyMem_MALLOC PyMem_Malloc
#define PyMem_FREE PyMem_Free
#define PyObject_Malloc PyMem_Malloc
#define PyObject_Free PyMem_Free
#define PyObject_Del PyObject_Free
#define PyObject_FREE PyObject_Free

static inline PyThread_type_lock PyThread_allocate_lock(void) {
    return (PyThread_type_lock)PyMem_Calloc(1, sizeof(PyMutex));
}

static inline int PyThread_acquire_lock(PyThread_type_lock lock, int waitflag) {
    (void)lock;
    (void)waitflag;
    return 1;
}

static inline void PyThread_release_lock(PyThread_type_lock lock) {
    (void)lock;
}

static inline void PyThread_free_lock(PyThread_type_lock lock) {
    PyMem_Free(lock);
}

static inline void PyMutex_Lock(PyMutex *mutex) {
    (void)mutex;
}

static inline void PyMutex_Unlock(PyMutex *mutex) {
    (void)mutex;
}

static inline void PyCriticalSection_Begin(PyCriticalSection *section, PyObject *obj) {
    if (section != NULL) {
        section->object = obj;
    }
}

static inline void PyCriticalSection_End(PyCriticalSection *section) {
    if (section != NULL) {
        section->object = NULL;
    }
}

#define Py_BEGIN_CRITICAL_SECTION(op) do { (void)(op)
#define Py_END_CRITICAL_SECTION() } while (0)

static inline int PyTraceMalloc_Track(unsigned int domain, uintptr_t ptr, size_t size) {
    (void)domain;
    (void)ptr;
    (void)size;
    return 0;
}

static inline int PyTraceMalloc_Untrack(unsigned int domain, uintptr_t ptr) {
    (void)domain;
    (void)ptr;
    return 0;
}

#define PyObject_GetAttrStr PyObject_GetAttrString

/* ---- Vectorcall protocol (PEP 590) ---- */

#define PY_VECTORCALL_ARGUMENTS_OFFSET ((size_t)1 << (8 * sizeof(size_t) - 1))

static inline Py_ssize_t PyVectorcall_NARGS(size_t nargsf) {
    return (Py_ssize_t)(nargsf & ~PY_VECTORCALL_ARGUMENTS_OFFSET);
}

extern int PyObject_Print(PyObject *obj, FILE *fp, int flags);

static inline PyObject *_molt_builtin_class_lookup_utf8(const char *name) {
    MoltHandle name_bits;
    MoltHandle class_bits;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_TypeError, "builtin class name must not be empty");
        return NULL;
    }
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    class_bits = molt_builtin_class_lookup(name_bits);
    molt_handle_decref(name_bits);
    return _molt_pyobject_from_result(class_bits);
}

/* Module lifecycle, state, and publication live in the linked CPython ABI. */

/* Private CHeap buffers retain their existing representation. Their count is
 * observable here, but explicit immortality is unsupported for that transport.
 * All ordinary PyObjects use the linked ABI's ownership authority. */
static inline int _molt_py_is_unique_referenced_temporary(PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        /* A private count does not establish temporary argument provenance. */
        return 0;
    }
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((int (*)(PyObject *))_molt_host_abi_symbol("PyUnstable_Object_IsUniqueReferencedTemporary"))(obj);
#else
    return PyUnstable_Object_IsUniqueReferencedTemporary(obj);
#endif
}

static inline int _molt_py_is_uniquely_referenced(PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        return _molt_c_heap_header_from_object(obj)->refcnt == 1;
    }
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((int (*)(PyObject *))_molt_host_abi_symbol("PyUnstable_Object_IsUniquelyReferenced"))(obj);
#else
    return PyUnstable_Object_IsUniquelyReferenced(obj);
#endif
}

static inline int _molt_py_enable_deferred_refcount(PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        return 0;
    }
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((int (*)(PyObject *))_molt_host_abi_symbol("PyUnstable_Object_EnableDeferredRefcount"))(obj);
#else
    return PyUnstable_Object_EnableDeferredRefcount(obj);
#endif
}

static inline int _molt_py_set_immortal(PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        return 0;
    }
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((int (*)(PyObject *))_molt_host_abi_symbol("PyUnstable_SetImmortal"))(obj);
#else
    return PyUnstable_SetImmortal(obj);
#endif
}

#define PyUnstable_Object_IsUniqueReferencedTemporary(obj) _molt_py_is_unique_referenced_temporary((PyObject *)(obj))
#define PyUnstable_Object_IsUniquelyReferenced(obj) _molt_py_is_uniquely_referenced((PyObject *)(obj))
#define PyUnstable_Object_EnableDeferredRefcount(obj) _molt_py_enable_deferred_refcount((PyObject *)(obj))
#define PyUnstable_SetImmortal(obj) _molt_py_set_immortal((PyObject *)(obj))

extern PyObject *PyBool_FromLong(long value);
extern PyObject *PyFloat_FromDouble(double value);
extern double PyFloat_AsDouble(PyObject *obj);
extern PyObject *PyFloat_FromString(PyObject *str);
extern double PyFloat_GetMax(void);
extern double PyFloat_GetMin(void);
extern PyObject *PyFloat_GetInfo(void);
extern int PyFloat_Pack2(double x, char *p, int le);
extern int PyFloat_Pack4(double x, char *p, int le);
extern int PyFloat_Pack8(double x, char *p, int le);
extern double PyFloat_Unpack2(const char *p, int le);
extern double PyFloat_Unpack4(const char *p, int le);
extern double PyFloat_Unpack8(const char *p, int le);

extern PyObject *PyNumber_Add(PyObject *, PyObject *);
extern PyObject *PyNumber_Subtract(PyObject *, PyObject *);
extern PyObject *PyNumber_Multiply(PyObject *, PyObject *);
extern PyObject *PyNumber_TrueDivide(PyObject *, PyObject *);
extern PyObject *PyNumber_FloorDivide(PyObject *, PyObject *);
extern PyObject *PyNumber_Long(PyObject *);

extern PyObject *PyUnicode_FromString(const char *value);

extern const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out);

extern const char *PyUnicode_AsUTF8(PyObject *value);

extern unsigned int molt_capi_unicode_kind(PyObject *value);
extern void *molt_capi_unicode_data(PyObject *value);
extern Py_UCS4 molt_capi_unicode_maxchar(PyObject *value);
#define PyUnicode_MAX_CHAR_VALUE(op) molt_capi_unicode_maxchar((PyObject *)(op))
extern Py_UCS4 PyUnicode_ReadChar(PyObject *value, Py_ssize_t index);
extern void _PyUnicode_FastCopyCharacters(PyObject *, Py_ssize_t, PyObject *, Py_ssize_t, Py_ssize_t);

static inline Py_UCS1 *PyUnicode_1BYTE_DATA(PyObject *value) {
    return (Py_UCS1 *)molt_capi_unicode_data(value);
}

static inline Py_UCS2 *PyUnicode_2BYTE_DATA(PyObject *value) {
    return (Py_UCS2 *)molt_capi_unicode_data(value);
}

static inline Py_UCS4 *PyUnicode_4BYTE_DATA(PyObject *value) {
    return (Py_UCS4 *)molt_capi_unicode_data(value);
}

static inline Py_UCS4 PyUnicode_READ_CHAR(PyObject *value, Py_ssize_t index) {
    return PyUnicode_ReadChar(value, index);
}

#define Py_UNICODE_ISALPHA(ch) \
    ((((Py_UCS4)(ch)) >= (Py_UCS4)'A' && ((Py_UCS4)(ch)) <= (Py_UCS4)'Z') \
        || (((Py_UCS4)(ch)) >= (Py_UCS4)'a' && ((Py_UCS4)(ch)) <= (Py_UCS4)'z'))
#define Py_UNICODE_ISDIGIT(ch) (((Py_UCS4)(ch)) >= (Py_UCS4)'0' && ((Py_UCS4)(ch)) <= (Py_UCS4)'9')
#define Py_UNICODE_ISDECIMAL(ch) Py_UNICODE_ISDIGIT(ch)
#define Py_UNICODE_ISNUMERIC(ch) Py_UNICODE_ISDIGIT(ch)
#define Py_UNICODE_ISALNUM(ch) (Py_UNICODE_ISALPHA(ch) || Py_UNICODE_ISDIGIT(ch))
#define Py_UNICODE_ISLOWER(ch) (((Py_UCS4)(ch)) >= (Py_UCS4)'a' && ((Py_UCS4)(ch)) <= (Py_UCS4)'z')
#define Py_UNICODE_ISUPPER(ch) (((Py_UCS4)(ch)) >= (Py_UCS4)'A' && ((Py_UCS4)(ch)) <= (Py_UCS4)'Z')
#define Py_UNICODE_ISTITLE(ch) Py_UNICODE_ISUPPER(ch)
#define Py_UNICODE_ISSPACE(ch) \
    (((Py_UCS4)(ch)) == (Py_UCS4)' ' || ((Py_UCS4)(ch)) == (Py_UCS4)'\t' \
        || ((Py_UCS4)(ch)) == (Py_UCS4)'\n' || ((Py_UCS4)(ch)) == (Py_UCS4)'\r' \
        || ((Py_UCS4)(ch)) == (Py_UCS4)'\f' || ((Py_UCS4)(ch)) == (Py_UCS4)'\v')

extern Py_ssize_t PyUnicode_GetLength(PyObject *value);

extern PyObject *PyUnicode_AsUTF8String(PyObject *value);

extern PyObject *PyUnicode_AsASCIIString(PyObject *value);

extern PyObject *PyUnicode_AsLatin1String(PyObject *value);

#define PyUnicode_1BYTE_KIND 1
#define PyUnicode_2BYTE_KIND 2
#define PyUnicode_4BYTE_KIND 4
extern int PyUnicode_IS_ASCII(PyObject *value);
#define PyUnicode_READ(kind, data, index) \
    ((kind) == 1 ? (Py_UCS4)((const Py_UCS1 *)(data))[(index)] : \
     (kind) == 2 ? (Py_UCS4)((const Py_UCS2 *)(data))[(index)] : \
                   ((const Py_UCS4 *)(data))[(index)])
static inline void PyUnicode_WRITE(int kind, void *data, Py_ssize_t index, Py_UCS4 value) {
    if (kind == 1) ((Py_UCS1 *)data)[index] = (Py_UCS1)value;
    else if (kind == 2) ((Py_UCS2 *)data)[index] = (Py_UCS2)value;
    else ((Py_UCS4 *)data)[index] = value;
}

/* PyUnicode_READY — no-op on 3.12+ (PEP 393 compact is always ready) */
#define PyUnicode_READY(op) (0)

static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size) {
    if (value == NULL && size > 0) {
        PyErr_SetString(PyExc_TypeError, "bytes source must not be NULL when size > 0");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_bytes_from((const uint8_t *)value, size < 0 ? 0u : (uint64_t)size));
}

static inline PyObject *PyBytes_FromString(const char *s) {
    return PyBytes_FromStringAndSize(s, (Py_ssize_t)strlen(s));
}

static inline int PyBytes_AsStringAndSize(PyObject *value, char **buf, Py_ssize_t *len_out) {
    uint64_t len = 0;
    const uint8_t *ptr = molt_bytes_as_ptr(_molt_py_handle(value), &len);
    if (ptr == NULL || molt_err_pending() != 0) {
        return -1;
    }
    if (buf != NULL) {
        *buf = (char *)ptr;
    }
    if (len_out != NULL) {
        *len_out = (Py_ssize_t)len;
    }
    return 0;
}

#include "shared/_buffer_exports.h"

static inline char *PyBytes_AsString(PyObject *value) {
    char *buf = NULL;
    if (PyBytes_AsStringAndSize(value, &buf, NULL) < 0) {
        return NULL;
    }
    return buf;
}

#define PyBytes_AS_STRING(op)                                                      \
    ((char *)molt_bytes_as_ptr(_molt_py_handle((PyObject *)(op)), NULL))
static inline Py_ssize_t _molt_pybytes_get_size(PyObject *value) {
    uint64_t len = 0;
    (void)molt_bytes_as_ptr(_molt_py_handle(value), &len);
    if (molt_err_pending() != 0) {
        return -1;
    }
    return (Py_ssize_t)len;
}
#define PyBytes_GET_SIZE(op) _molt_pybytes_get_size((PyObject *)(op))

extern PyObject *PyUnicode_FromStringAndSize(const char *value, Py_ssize_t size);

extern PyObject *PyUnicode_FromEncodedObject(
    PyObject *obj,
    const char *encoding,
    const char *errors
);

PyAPI_DATA(PyTypeObject) PyLong_Type;
PyAPI_DATA(PyTypeObject) PyFloat_Type;
PyAPI_DATA(PyTypeObject) PyBool_Type;
PyAPI_DATA(PyTypeObject) PyComplex_Type;
PyAPI_DATA(PyTypeObject) PyBytes_Type;
PyAPI_DATA(PyTypeObject) PyUnicode_Type;
PyAPI_DATA(PyTypeObject) PyType_Type;
PyAPI_DATA(PyTypeObject) PyByteArray_Type;
PyAPI_DATA(PyTypeObject) PyMemoryView_Type;
PyAPI_DATA(PyTypeObject) PyBaseObject_Type;
PyAPI_DATA(PyTypeObject) PyRange_Type;
#define PyFloat_AS_DOUBLE(op) PyFloat_AsDouble((PyObject *)(op))

extern int PyUnicode_Check(PyObject *obj);

extern int PyBytes_Check(PyObject *obj);

extern int PyBool_Check(PyObject *obj);
extern int PyFloat_Check(PyObject *obj);
extern int PyComplex_Check(PyObject *obj);

extern int PyLong_CheckExact(PyObject *obj);
extern int PyFloat_CheckExact(PyObject *obj);
extern int PyComplex_CheckExact(PyObject *obj);
extern int PyUnicode_CheckExact(PyObject *obj);
extern int PyBytes_CheckExact(PyObject *obj);
extern int PyByteArray_CheckExact(PyObject *obj);

PyAPI_DATA(PyTypeObject) PyNone_Type;

#include "shared/_object_observation_exports.h"

static inline PyObject *Py_GenericAlias(PyObject *origin, PyObject *args) {
    PyObject *types_mod;
    PyObject *generic_alias;
    PyObject *result;
    if (origin == NULL || args == NULL) {
        PyErr_SetString(PyExc_TypeError, "origin and args must not be NULL");
        return NULL;
    }
    types_mod = PyImport_ImportModule("types");
    if (types_mod == NULL) {
        return NULL;
    }
    generic_alias = PyObject_GetAttrString(types_mod, "GenericAlias");
    Py_DECREF(types_mod);
    if (generic_alias == NULL) {
        return NULL;
    }
    result = PyObject_CallFunctionObjArgs(generic_alias, origin, args, NULL);
    Py_DECREF(generic_alias);
    return result;
}

static inline PyObject *PyImport_ImportModule(const char *name) {
    MoltHandle name_bits;
    MoltHandle module_bits;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "module name must not be empty");
        return NULL;
    }
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    module_bits = molt_module_import(name_bits);
    molt_handle_decref(name_bits);
    return _molt_pyobject_from_result(module_bits);
}

#define _MOLT_CAPSULE_PTR_KEY "__molt_capsule_ptr__"
#define _MOLT_CAPSULE_NAME_KEY "__molt_capsule_name__"
#define _MOLT_CAPSULE_DESTRUCTOR_KEY "__molt_capsule_destructor__"

static inline PyObject *PyCapsule_New(
    void *pointer,
    const char *name,
    PyCapsule_Destructor destructor
) {
    PyObject *capsule;
    PyObject *ptr_value;
    PyObject *name_value;
    if (pointer == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_New called with NULL pointer");
        return NULL;
    }
    capsule = PyDict_New();
    if (capsule == NULL) {
        return NULL;
    }
    ptr_value = PyLong_FromLongLong((long long)(uintptr_t)pointer);
    if (ptr_value == NULL) {
        Py_DECREF(capsule);
        return NULL;
    }
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_PTR_KEY, ptr_value) < 0) {
        Py_DECREF(ptr_value);
        Py_DECREF(capsule);
        return NULL;
    }
    Py_DECREF(ptr_value);
    if (name != NULL) {
        name_value = PyUnicode_FromString(name);
    } else {
        name_value = Py_None;
        Py_INCREF(name_value);
    }
    if (name_value == NULL) {
        Py_DECREF(capsule);
        return NULL;
    }
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_NAME_KEY, name_value) < 0) {
        Py_DECREF(name_value);
        Py_DECREF(capsule);
        return NULL;
    }
    Py_DECREF(name_value);
    if (destructor != NULL) {
        PyObject *destructor_value = PyLong_FromLongLong((long long)(uintptr_t)destructor);
        if (destructor_value == NULL) {
            Py_DECREF(capsule);
            return NULL;
        }
        if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_DESTRUCTOR_KEY, destructor_value) < 0) {
            Py_DECREF(destructor_value);
            Py_DECREF(capsule);
            return NULL;
        }
        Py_DECREF(destructor_value);
    }
    return capsule;
}

static inline const char *PyCapsule_GetName(PyObject *capsule) {
    PyObject *name_obj;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return NULL;
    }
    name_obj = PyDict_GetItemString(capsule, _MOLT_CAPSULE_NAME_KEY);
    if (name_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object is not a valid capsule");
        return NULL;
    }
    if (_molt_py_handle(name_obj) == molt_none()) {
        return NULL;
    }
    return PyUnicode_AsUTF8(name_obj);
}

static inline void *PyCapsule_GetPointer(PyObject *capsule, const char *name) {
    PyObject *ptr_obj;
    const char *capsule_name;
    long long raw_ptr;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return NULL;
    }
    ptr_obj = PyDict_GetItemString(capsule, _MOLT_CAPSULE_PTR_KEY);
    if (ptr_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object is not a valid capsule");
        return NULL;
    }
    capsule_name = PyCapsule_GetName(capsule);
    if (molt_err_pending() != 0) {
        return NULL;
    }
    if (name != NULL) {
        if (capsule_name == NULL || strcmp(capsule_name, name) != 0) {
            PyErr_SetString(PyExc_ValueError, "capsule name mismatch");
            return NULL;
        }
    }
    raw_ptr = PyLong_AsLongLong(ptr_obj);
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)(uintptr_t)raw_ptr;
}

static inline int PyCapsule_IsValid(PyObject *capsule, const char *name) {
    void *ptr = PyCapsule_GetPointer(capsule, name);
    if (ptr == NULL) {
        PyErr_Clear();
        return 0;
    }
    return 1;
}

static inline int PyCapsule_CheckExact(PyObject *capsule) {
    return PyCapsule_IsValid(capsule, NULL);
}

static inline void *PyCapsule_Import(const char *name, int no_block) {
    const char *last_dot;
    size_t module_len;
    char *module_name;
    const char *attr_name;
    PyObject *module_obj;
    PyObject *capsule_obj;
    void *ptr;
    (void)no_block;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "capsule import name must not be empty");
        return NULL;
    }
    last_dot = strrchr(name, '.');
    if (last_dot == NULL || last_dot == name || last_dot[1] == '\0') {
        PyErr_SetString(
            PyExc_ValueError,
            "capsule import name must contain module and attribute");
        return NULL;
    }
    module_len = (size_t)(last_dot - name);
    module_name = (char *)PyMem_Malloc(module_len + 1);
    if (module_name == NULL) {
        return NULL;
    }
    memcpy(module_name, name, module_len);
    module_name[module_len] = '\0';
    attr_name = last_dot + 1;

    module_obj = PyImport_ImportModule(module_name);
    PyMem_Free(module_name);
    if (module_obj == NULL) {
        return NULL;
    }
    capsule_obj = PyObject_GetAttrString(module_obj, attr_name);
    Py_DECREF(module_obj);
    if (capsule_obj == NULL) {
        return NULL;
    }
    ptr = PyCapsule_GetPointer(capsule_obj, name);
    Py_DECREF(capsule_obj);
    return ptr;
}

#define PyCObject_Import(name) PyCapsule_Import((name), 0)

/*
 * Returns:
 *  1 on inserted
 *  0 on duplicate keyword
 * -1 on missing table capacity
 */
/*
 * Minimal O(n) parser for common extension fast paths.
 * Supported format units: O, O!, b, B, h, H, i, I, l, k, L, K, n, c, d, f, p,
 * s, s#, z, z#, y#, and markers '|', '$', ':', ';'.
 */
/* Forward declaration -- defined later in this header (dict helpers section). */

/* =========================================================================
 * Type Object, Object Init, PyLong completions, Abstract Protocol,
 * Weakref, Set/FrozenSet, Descriptor protocol
 * ========================================================================= */

/* ---- Type Object functions ---------------------------------------------- */

#define PyType_IS_GC(type) PyType_HasFeature((type), Py_TPFLAGS_HAVE_GC)

/* ---- Object creation / initialisation ----------------------------------- */

extern PyObject *PyObject_Init(PyObject *op, PyTypeObject *type);
extern PyVarObject *PyObject_InitVar(PyVarObject *op, PyTypeObject *type, Py_ssize_t size);
extern PyObject *_PyObject_New(PyTypeObject *type);
extern PyVarObject *_PyObject_NewVar(PyTypeObject *type, Py_ssize_t size);
extern PyObject *PyType_GenericAlloc(PyTypeObject *type, Py_ssize_t nitems);
extern PyObject *PyType_GenericNew(PyTypeObject *type, PyObject *args, PyObject *kwds);

static inline void _molt_require_canonical_allocation_type(PyTypeObject *type) {
    if (_molt_c_heap_object_is((PyObject *)type)) {
        _molt_c_heap_fatal("object allocation requires a canonical PyTypeObject");
    }
}

static inline PyObject *_molt_object_init(PyObject *op, PyTypeObject *type) {
    if (_molt_c_heap_object_is(op)) {
        _molt_c_heap_fatal("PyObject_Init cannot initialize a private C-heap object");
    }
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyObject *(*)(PyObject *, PyTypeObject *))_molt_host_abi_symbol("PyObject_Init"))(op, type);
#else
    return PyObject_Init(op, type);
#endif
}

static inline PyVarObject *_molt_object_init_var(PyVarObject *op, PyTypeObject *type, Py_ssize_t size) {
    if (_molt_c_heap_object_is((PyObject *)op)) {
        _molt_c_heap_fatal("PyObject_InitVar cannot initialize a private C-heap object");
    }
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyVarObject *(*)(PyVarObject *, PyTypeObject *, Py_ssize_t))_molt_host_abi_symbol("PyObject_InitVar"))(op, type, size);
#else
    return PyObject_InitVar(op, type, size);
#endif
}

static inline PyObject *_molt_object_new(PyTypeObject *type) {
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("_PyObject_New"))(type);
#else
    return _PyObject_New(type);
#endif
}

static inline PyVarObject *_molt_object_new_var(PyTypeObject *type, Py_ssize_t size) {
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyVarObject *(*)(PyTypeObject *, Py_ssize_t))_molt_host_abi_symbol("_PyObject_NewVar"))(type, size);
#else
    return _PyObject_NewVar(type, size);
#endif
}

#define PyObject_Init _molt_object_init
#define PyObject_InitVar _molt_object_init_var
#define _PyObject_New _molt_object_new
#define _PyObject_NewVar _molt_object_new_var

#ifndef PyObject_NewVar
#define PyObject_NewVar(type, typeobj, n) ((type *)_PyObject_NewVar((PyTypeObject *)(typeobj), (Py_ssize_t)(n)))
#endif

/* ---- Set / FrozenSet protocol ------------------------------------------- */

/* ---- Weakref protocol --------------------------------------------------- */

static inline int PyWeakref_Check(PyObject *ob) {
    PyObject *weakref_mod;
    PyObject *ref_type;
    int result;
    if (ob == NULL) {
        return 0;
    }
    weakref_mod = PyImport_ImportModule("weakref");
    if (weakref_mod == NULL) {
        PyErr_Clear();
        return 0;
    }
    ref_type = PyObject_GetAttrString(weakref_mod, "ref");
    Py_DECREF(weakref_mod);
    if (ref_type == NULL) {
        PyErr_Clear();
        return 0;
    }
    result = PyObject_TypeCheck(ob, (PyTypeObject *)ref_type);
    Py_DECREF(ref_type);
    return result;
}

static inline PyObject *PyWeakref_NewRef(PyObject *ob, PyObject *callback) {
    PyObject *weakref_mod;
    PyObject *ref_callable;
    MoltHandle args_arr[2];
    uint64_t nargs;
    MoltHandle args_bits;
    PyObject *result;
    if (ob == NULL) {
        PyErr_SetString(PyExc_TypeError, "cannot create weak reference to NULL");
        return NULL;
    }
    weakref_mod = PyImport_ImportModule("weakref");
    if (weakref_mod == NULL) {
        return NULL;
    }
    ref_callable = PyObject_GetAttrString(weakref_mod, "ref");
    Py_DECREF(weakref_mod);
    if (ref_callable == NULL) {
        return NULL;
    }
    args_arr[0] = _molt_py_handle(ob);
    nargs = 1;
    if (callback != NULL && callback != Py_None) {
        args_arr[1] = _molt_py_handle(callback);
        nargs = 2;
    }
    args_bits = molt_tuple_from_array(args_arr, nargs);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(ref_callable);
        return NULL;
    }
    result = PyObject_CallObject(ref_callable, _molt_pyobject_from_borrowed_handle(args_bits));
    molt_handle_decref(args_bits);
    Py_DECREF(ref_callable);
    return result;
}

static inline PyObject *PyWeakref_NewProxy(PyObject *ob, PyObject *callback) {
    PyObject *weakref_mod;
    PyObject *proxy_callable;
    MoltHandle args_arr[2];
    uint64_t nargs;
    MoltHandle args_bits;
    PyObject *result;
    if (ob == NULL) {
        PyErr_SetString(PyExc_TypeError, "cannot create weak reference proxy to NULL");
        return NULL;
    }
    weakref_mod = PyImport_ImportModule("weakref");
    if (weakref_mod == NULL) {
        return NULL;
    }
    proxy_callable = PyObject_GetAttrString(weakref_mod, "proxy");
    Py_DECREF(weakref_mod);
    if (proxy_callable == NULL) {
        return NULL;
    }
    args_arr[0] = _molt_py_handle(ob);
    nargs = 1;
    if (callback != NULL && callback != Py_None) {
        args_arr[1] = _molt_py_handle(callback);
        nargs = 2;
    }
    args_bits = molt_tuple_from_array(args_arr, nargs);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(proxy_callable);
        return NULL;
    }
    result = PyObject_CallObject(proxy_callable, _molt_pyobject_from_borrowed_handle(args_bits));
    molt_handle_decref(args_bits);
    Py_DECREF(proxy_callable);
    return result;
}

static inline PyObject *PyWeakref_GetObject(PyObject *ref) {
    PyObject *result;
    if (ref == NULL) {
        return Py_None;
    }
    result = PyObject_CallObject(ref, NULL);
    if (result == NULL) {
        PyErr_Clear();
        return Py_None;
    }
    return result;
}

static inline int PyWeakref_GetRef(PyObject *ref, PyObject **pobj) {
    PyObject *result;
    if (ref == NULL) {
        if (pobj) *pobj = NULL;
        return -1;
    }
    result = PyObject_CallObject(ref, NULL);
    if (result == NULL) {
        if (molt_err_pending() != 0) {
            if (pobj) *pobj = NULL;
            return -1;
        }
        if (pobj) *pobj = NULL;
        return 0;
    }
    if (result == Py_None) {
        Py_DECREF(result);
        if (pobj) *pobj = NULL;
        return 0;
    }
    if (pobj) *pobj = result;
    return 1;
}

/* ---- PyLong completions ------------------------------------------------- */

/* ---- Abstract Object protocol ------------------------------------------- */

extern PyObject *PyObject_ASCII(PyObject *o);

/* Descriptor constructors and layouts come from the shared ABI headers. */

/* ========================================================================
 * PY_SSIZE_T_MAX (needed by Slice API below)
 * ======================================================================== */

#ifndef PY_SSIZE_T_MAX
#define PY_SSIZE_T_MAX ((Py_ssize_t)(((size_t)-1) >> 1))
#endif

/* ========================================================================
 * Import C API
 * ======================================================================== */

static inline PyObject *PyImport_ImportModuleNoBlock(const char *name) {
    return PyImport_ImportModule(name);
}

static inline PyObject *PyImport_Import(PyObject *name) {
    const char *name_utf8;
    if (name == NULL) {
        PyErr_SetString(PyExc_ValueError, "module name must not be NULL");
        return NULL;
    }
    name_utf8 = PyUnicode_AsUTF8(name);
    if (name_utf8 == NULL) {
        return NULL;
    }
    return PyImport_ImportModule(name_utf8);
}

static inline PyObject *PyImport_GetModule(PyObject *name) {
    return PyImport_Import(name);
}

static inline PyObject *PyImport_AddModule(const char *name) {
    PyObject *modules_dict;
    PyObject *module;
    MoltHandle name_bits;
    MoltHandle module_bits;
    MoltHandle key_bits;
    MoltHandle existing;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "module name must not be empty");
        return NULL;
    }
    /* Look up the module via sys.modules dict with a borrowed reference,
       avoiding the refcount leak that PyImport_ImportModule would cause. */
    modules_dict = PyImport_GetModuleDict();
    if (modules_dict == NULL) {
        return NULL;
    }
    key_bits = _molt_string_from_utf8(name);
    if (key_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    existing = molt_dict_getitem_borrowed(_molt_py_handle(modules_dict), key_bits);
    molt_handle_decref(key_bits);
    if (existing != 0) {
        /* Module already in sys.modules — return borrowed reference. */
        return _molt_pyobject_from_borrowed_handle(existing);
    }
    /* Module not found — create a new one. */
    PyErr_Clear();
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    module_bits = molt_module_create(name_bits);
    molt_handle_decref(name_bits);
    if (module_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    module = _molt_pyobject_from_borrowed_handle(module_bits);
    /* Return borrowed reference — module stays alive in sys.modules. */
    return module;
}

static inline int PyImport_ImportFrozenModule(const char *name) {
    (void)name;
    return 0;
}

/* ========================================================================
 * Thread State C API
 * ======================================================================== */

static inline PyThreadState *PyThreadState_Swap(PyThreadState *tstate) {
    PyThreadState *old = PyThreadState_Get();
    (void)tstate;
    return old;
}

static inline PyObject *PyThreadState_GetDict(void) {
    static MoltHandle tstate_dict = 0;
    if (tstate_dict == 0) {
        tstate_dict = molt_dict_from_pairs(NULL, NULL, 0);
        if (tstate_dict == 0 || molt_err_pending() != 0) {
            tstate_dict = 0;
            return NULL;
        }
    }
    return _molt_pyobject_from_borrowed_handle(tstate_dict);
}

static inline void PyThreadState_Clear(PyThreadState *tstate) {
    (void)tstate;
}

static inline int PyGILState_Check(void) {
    return molt_gil_is_held() != 0 ? 1 : 0;
}

/* ========================================================================
 * Interpreter State C API (single-interpreter stubs)
 * ======================================================================== */

static inline PyInterpreterState *PyInterpreterState_Get(void) {
    static PyInterpreterState interp = {0};
    return &interp;
}

static inline PyInterpreterState *PyInterpreterState_Main(void) {
    return PyInterpreterState_Get();
}

static inline PyThreadState *PyInterpreterState_ThreadHead(PyInterpreterState *interp) {
    (void)interp;
    return PyThreadState_Get();
}

static inline PyThreadState *PyThreadState_Next(PyThreadState *tstate) {
    (void)tstate;
    return NULL;
}

/* ========================================================================
 * Eval C API
 * ======================================================================== */

static inline PyObject *PyEval_GetGlobals(void) {
    return PyEval_GetBuiltins();
}

static inline PyObject *PyEval_GetLocals(void) {
    return NULL;
}

static inline void PyEval_InitThreads(void) {
}

static inline int PyEval_ThreadsInitialized(void) {
    return 1;
}

static inline PyObject *PyEval_CallObjectWithKeywords(PyObject *func, PyObject *args, PyObject *kwargs) {
    return PyObject_Call(func, args, kwargs);
}

/* ========================================================================
 * PySys C API
 * ======================================================================== */

static inline int PySys_SetObject(const char *name, PyObject *v) {
    PyObject *sys_mod = PyImport_ImportModule("sys");
    int rc;
    if (sys_mod == NULL) {
        return -1;
    }
    rc = PyObject_SetAttrString(sys_mod, name, v);
    Py_DECREF(sys_mod);
    return rc;
}

static inline void PySys_WriteStdout(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    (void)vfprintf(stdout, format != NULL ? format : "", ap);
    va_end(ap);
}

static inline void PySys_WriteStderr(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    (void)vfprintf(stderr, format != NULL ? format : "", ap);
    va_end(ap);
}

static inline void PySys_FormatStdout(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    (void)vfprintf(stdout, format != NULL ? format : "", ap);
    va_end(ap);
}

static inline void PySys_FormatStderr(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    (void)vfprintf(stderr, format != NULL ? format : "", ap);
    va_end(ap);
}

/* ========================================================================
 * PyOS C API
 * ======================================================================== */

static inline char *PyOS_double_to_string(
    double val, char format_code, int precision,
    int flags, int *ptype)
{
    char buf[128];
    char fmt[16];
    char *result;
    size_t len;
    (void)flags;
    if (ptype != NULL) {
        *ptype = 0;
    }
    (void)snprintf(fmt, sizeof(fmt), "%%.%d%c", precision, format_code);
    (void)snprintf(buf, sizeof(buf), fmt, val);
    len = strlen(buf);
    result = (char *)PyMem_Malloc(len + 1);
    if (result != NULL) {
        memcpy(result, buf, len + 1);
    }
    return result;
}

static inline int PyOS_stricmp(const char *a, const char *b) {
    if (a == NULL && b == NULL) return 0;
    if (a == NULL) return -1;
    if (b == NULL) return 1;
    while (*a && *b) {
        int ca = (*a >= 'A' && *a <= 'Z') ? (*a + 32) : *a;
        int cb = (*b >= 'A' && *b <= 'Z') ? (*b + 32) : *b;
        if (ca != cb) return ca - cb;
        a++;
        b++;
    }
    {
        int ca = (*a >= 'A' && *a <= 'Z') ? (*a + 32) : *a;
        int cb = (*b >= 'A' && *b <= 'Z') ? (*b + 32) : *b;
        return ca - cb;
    }
}

static inline int PyOS_strnicmp(const char *a, const char *b, Py_ssize_t n) {
    Py_ssize_t i;
    if (a == NULL && b == NULL) return 0;
    if (a == NULL) return -1;
    if (b == NULL) return 1;
    for (i = 0; i < n && *a && *b; i++, a++, b++) {
        int ca = (*a >= 'A' && *a <= 'Z') ? (*a + 32) : *a;
        int cb = (*b >= 'A' && *b <= 'Z') ? (*b + 32) : *b;
        if (ca != cb) return ca - cb;
    }
    if (i == n) return 0;
    {
        int ca = (*a >= 'A' && *a <= 'Z') ? (*a + 32) : *a;
        int cb = (*b >= 'A' && *b <= 'Z') ? (*b + 32) : *b;
        return ca - cb;
    }
}

/* ========================================================================
 * Slice C API
 * ======================================================================== */

/* ========================================================================
 * Complex C API
 * ======================================================================== */

extern PyObject *PyComplex_FromDoubles(double real, double imag);
extern double PyComplex_RealAsDouble(PyObject *op);
extern double PyComplex_ImagAsDouble(PyObject *op);
extern PyObject *PyComplex_FromCComplex(Py_complex value);
extern Py_complex PyComplex_AsCComplex(PyObject *op);
extern Py_complex _Py_c_sum(Py_complex a, Py_complex b);
extern Py_complex _Py_c_diff(Py_complex a, Py_complex b);
extern Py_complex _Py_c_neg(Py_complex a);
extern Py_complex _Py_c_prod(Py_complex a, Py_complex b);
extern Py_complex _Py_c_quot(Py_complex a, Py_complex b);
extern Py_complex _Py_c_pow(Py_complex a, Py_complex b);
extern double _Py_c_abs(Py_complex a);

static inline PyObject *_molt_datetime_attr(const char *name) {
    PyObject *datetime_mod = PyImport_ImportModule("datetime");
    PyObject *attr;
    if (datetime_mod == NULL) {
        return NULL;
    }
    attr = PyObject_GetAttrString(datetime_mod, name);
    Py_DECREF(datetime_mod);
    return attr;
}

static inline PyObject *_molt_call_datetime_attr(
    const char *name, MoltHandle *args, uint64_t argc) {
    PyObject *callable = _molt_datetime_attr(name);
    MoltHandle args_tuple;
    MoltHandle result;
    if (callable == NULL) {
        return NULL;
    }
    args_tuple = molt_tuple_from_array(args, argc);
    if (args_tuple == 0 || molt_err_pending() != 0) {
        Py_DECREF(callable);
        return NULL;
    }
    result = molt_object_call(_molt_py_handle(callable), args_tuple, molt_none());
    Py_DECREF(callable);
    molt_handle_decref(args_tuple);
    return _molt_pyobject_from_result(result);
}

static inline PyObject *PyDate_FromDate(int year, int month, int day) {
    MoltHandle args[3];
    PyObject *result;
    args[0] = molt_int_from_i64(year);
    args[1] = molt_int_from_i64(month);
    args[2] = molt_int_from_i64(day);
    result = _molt_call_datetime_attr("date", args, 3);
    molt_handle_decref(args[0]);
    molt_handle_decref(args[1]);
    molt_handle_decref(args[2]);
    return result;
}

static inline PyObject *PyDateTime_FromDateAndTime(
    int year, int month, int day, int hour, int minute, int second, int usecond) {
    MoltHandle args[7];
    PyObject *result;
    args[0] = molt_int_from_i64(year);
    args[1] = molt_int_from_i64(month);
    args[2] = molt_int_from_i64(day);
    args[3] = molt_int_from_i64(hour);
    args[4] = molt_int_from_i64(minute);
    args[5] = molt_int_from_i64(second);
    args[6] = molt_int_from_i64(usecond);
    result = _molt_call_datetime_attr("datetime", args, 7);
    for (int i = 0; i < 7; i++) {
        molt_handle_decref(args[i]);
    }
    return result;
}

static inline PyObject *PyDelta_FromDSU(int days, int seconds, int useconds) {
    MoltHandle args[3];
    PyObject *result;
    args[0] = molt_int_from_i64(days);
    args[1] = molt_int_from_i64(seconds);
    args[2] = molt_int_from_i64(useconds);
    result = _molt_call_datetime_attr("timedelta", args, 3);
    molt_handle_decref(args[0]);
    molt_handle_decref(args[1]);
    molt_handle_decref(args[2]);
    return result;
}

static inline PyObject *_molt_datetime_timezone_utc(void) {
    PyObject *timezone = _molt_datetime_attr("timezone");
    PyObject *utc;
    if (timezone == NULL) {
        return NULL;
    }
    utc = PyObject_GetAttrString(timezone, "utc");
    Py_DECREF(timezone);
    return utc;
}

#define PyDateTime_TimeZone_UTC _molt_datetime_timezone_utc()

/* ========================================================================
 * Context Variables C API
 * ======================================================================== */

#include "shared/_context_exports.h"

/* ========================================================================
 * Marshal C API — delegates to pickle for serialization
 * ======================================================================== */

static inline PyObject *PyMarshal_WriteObjectToString(PyObject *value, int version) {
    PyObject *mod, *fn, *args, *result;
    (void)version;
    if (value == NULL) {
        PyErr_SetString(PyExc_TypeError, "cannot marshal NULL object");
        return NULL;
    }
    mod = PyImport_ImportModule("pickle");
    if (mod == NULL) return NULL;
    fn = PyObject_GetAttrString(mod, "dumps");
    Py_DECREF(mod);
    if (fn == NULL) return NULL;
    args = PyTuple_Pack(1, value);
    if (args == NULL) { Py_DECREF(fn); return NULL; }
    result = PyObject_CallObject(fn, args);
    Py_DECREF(args);
    Py_DECREF(fn);
    return result;
}

static inline PyObject *PyMarshal_ReadObjectFromString(const char *data, Py_ssize_t len) {
    PyObject *mod, *fn, *bytes_obj, *args, *result;
    if (data == NULL || len <= 0) {
        PyErr_SetString(PyExc_ValueError, "cannot unmarshal empty data");
        return NULL;
    }
    mod = PyImport_ImportModule("pickle");
    if (mod == NULL) return NULL;
    fn = PyObject_GetAttrString(mod, "loads");
    Py_DECREF(mod);
    if (fn == NULL) return NULL;
    bytes_obj = PyBytes_FromStringAndSize(data, len);
    if (bytes_obj == NULL) { Py_DECREF(fn); return NULL; }
    args = PyTuple_Pack(1, bytes_obj);
    Py_DECREF(bytes_obj);
    if (args == NULL) { Py_DECREF(fn); return NULL; }
    result = PyObject_CallObject(fn, args);
    Py_DECREF(args);
    Py_DECREF(fn);
    return result;
}

/* ========================================================================
 * Dunder-dispatch helper (internal)
 * ======================================================================== */

static inline PyObject *_molt_call_dunder_unary(PyObject *o, const char *dunder) {
    MoltHandle method;
    MoltHandle out;
    MoltHandle args;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)dunder, (uint64_t)strlen(dunder));
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *_molt_call_dunder_binary(PyObject *o1, PyObject *o2,
                                                  const char *dunder) {
    MoltHandle method;
    MoltHandle out;
    MoltHandle arg;
    MoltHandle args;
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(o1),
        (const uint8_t *)dunder, (uint64_t)strlen(dunder));
    if (method == 0 || molt_err_pending() != 0) return NULL;
    arg = _molt_py_handle(o2);
    args = molt_tuple_from_array(&arg, 1);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *_molt_call_dunder_ternary(PyObject *o1, PyObject *o2,
                                                   PyObject *o3, const char *dunder) {
    MoltHandle method;
    MoltHandle out;
    MoltHandle call_args[2];
    MoltHandle args;
    if (o1 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(o1),
        (const uint8_t *)dunder, (uint64_t)strlen(dunder));
    if (method == 0 || molt_err_pending() != 0) return NULL;
    call_args[0] = _molt_py_handle(o2);
    call_args[1] = _molt_py_handle(o3);
    args = molt_tuple_from_array(call_args, 2);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * Number Protocol: one external ABI authority
 * ======================================================================== */

extern PyObject *PyNumber_Remainder(PyObject *, PyObject *);
extern PyObject *PyNumber_Power(PyObject *, PyObject *, PyObject *);
extern PyObject *PyNumber_Negative(PyObject *);
extern PyObject *PyNumber_Positive(PyObject *);
extern PyObject *PyNumber_Absolute(PyObject *);
extern PyObject *PyNumber_Invert(PyObject *);
extern PyObject *PyNumber_Lshift(PyObject *, PyObject *);
extern PyObject *PyNumber_Rshift(PyObject *, PyObject *);
extern PyObject *PyNumber_And(PyObject *, PyObject *);
extern PyObject *PyNumber_Or(PyObject *, PyObject *);
extern PyObject *PyNumber_Xor(PyObject *, PyObject *);
extern PyObject *PyNumber_Float(PyObject *);
extern PyObject *PyNumber_Index(PyObject *);
extern PyObject *PyNumber_InPlaceAdd(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceSubtract(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceMultiply(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceTrueDivide(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceFloorDivide(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceRemainder(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceLshift(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceRshift(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceAnd(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceOr(PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceXor(PyObject *);

/* ========================================================================
 * Object Protocol (remaining)
 * ======================================================================== */

/* ========================================================================
 * Sequence Protocol (remaining)
 * ======================================================================== */

/* ========================================================================
 * Unicode (remaining)
 * ======================================================================== */

extern PyObject *PyUnicode_FromObject(PyObject *obj);

extern PyObject *PyUnicode_Substring(PyObject *str,
                                             Py_ssize_t start,
                                             Py_ssize_t end);

extern PyObject *PyUnicode_Concat(PyObject *left, PyObject *right);

extern PyObject *PyUnicode_Join(PyObject *separator, PyObject *seq);

static inline PyObject *PyUnicode_Split(PyObject *s, PyObject *sep,
                                         Py_ssize_t maxsplit) {
    MoltHandle method;
    MoltHandle call_args[2];
    MoltHandle args;
    MoltHandle out;
    if (s == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(s),
        (const uint8_t *)"split", 5);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    if (sep == NULL || sep == Py_None) {
        if (maxsplit < 0) {
            args = molt_tuple_from_array(NULL, 0);
        } else {
            call_args[0] = molt_none();
            call_args[1] = molt_int_from_i64((int64_t)maxsplit);
            args = molt_tuple_from_array(call_args, 2);
            molt_handle_decref(call_args[1]);
        }
    } else {
        call_args[0] = _molt_py_handle(sep);
        if (maxsplit < 0) {
            args = molt_tuple_from_array(call_args, 1);
        } else {
            call_args[1] = molt_int_from_i64((int64_t)maxsplit);
            args = molt_tuple_from_array(call_args, 2);
            molt_handle_decref(call_args[1]);
        }
    }
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

extern PyObject *PyUnicode_Replace(PyObject *str, PyObject *substr,
                                           PyObject *replstr, Py_ssize_t maxcount);

static inline Py_ssize_t PyUnicode_Find(PyObject *str, PyObject *substr,
                                          Py_ssize_t start, Py_ssize_t end,
                                          int direction) {
    MoltHandle method;
    MoltHandle call_args[3];
    MoltHandle args;
    MoltHandle out;
    PyObject *result;
    Py_ssize_t idx;
    const char *mname = (direction >= 0) ? "find" : "rfind";
    if (str == NULL || substr == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -2; }
    method = molt_object_getattr_bytes(_molt_py_handle(str),
        (const uint8_t *)mname, (uint64_t)strlen(mname));
    if (method == 0 || molt_err_pending() != 0) return -2;
    call_args[0] = _molt_py_handle(substr);
    call_args[1] = molt_int_from_i64((int64_t)start);
    call_args[2] = molt_int_from_i64((int64_t)end);
    args = molt_tuple_from_array(call_args, 3);
    /* Decref temporaries before the call — tuple holds its own refs */
    molt_handle_decref(call_args[2]);
    molt_handle_decref(call_args[1]);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) return -2;
    idx = (Py_ssize_t)PyLong_AsLongLong(result);
    Py_DECREF(result);
    return idx;
}

static inline Py_ssize_t PyUnicode_Count(PyObject *str, PyObject *substr,
                                           Py_ssize_t start, Py_ssize_t end) {
    MoltHandle method;
    MoltHandle call_args[3];
    MoltHandle args;
    MoltHandle out;
    PyObject *result;
    Py_ssize_t cnt;
    if (str == NULL || substr == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(str),
        (const uint8_t *)"count", 5);
    if (method == 0 || molt_err_pending() != 0) return -1;
    call_args[0] = _molt_py_handle(substr);
    call_args[1] = molt_int_from_i64((int64_t)start);
    call_args[2] = molt_int_from_i64((int64_t)end);
    args = molt_tuple_from_array(call_args, 3);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(call_args[1]);
    molt_handle_decref(call_args[2]);
    molt_handle_decref(method);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) return -1;
    cnt = (Py_ssize_t)PyLong_AsLongLong(result);
    Py_DECREF(result);
    return cnt;
}

extern int PyUnicode_Contains(PyObject *container, PyObject *element);

extern int PyUnicode_Compare(PyObject *left, PyObject *right);

extern int PyUnicode_CompareWithASCIIString(PyObject *uni, const char *str);

extern PyObject *PyUnicode_DecodeUTF8(const char *s, Py_ssize_t size,
                                               const char *errors);

extern PyObject *PyUnicode_DecodeASCII(const char *s, Py_ssize_t size,
                                                const char *errors);

extern PyObject *PyUnicode_DecodeLatin1(const char *s, Py_ssize_t size,
                                                 const char *errors);

extern PyObject *PyUnicode_AsEncodedString(PyObject *unicode,
                                                    const char *encoding,
                                                    const char *errors);

extern Py_UCS4 PyUnicode_ReadChar(PyObject *unicode, Py_ssize_t index);

extern int PyUnicode_WriteChar(PyObject *unicode, Py_ssize_t index,
                                       Py_UCS4 character);

extern PyObject *PyUnicode_Format(PyObject *format, PyObject *args);

/* ========================================================================
 * Bytes / ByteArray (remaining)
 * ======================================================================== */

static inline Py_ssize_t PyBytes_Size(PyObject *o) {
    uint64_t len = 0;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    if (molt_bytes_as_ptr(_molt_py_handle(o), &len) == NULL) {
        if (molt_err_pending() != 0) return -1;
    }
    return (Py_ssize_t)len;
}

static inline PyObject *PyBytes_FromFormat(const char *format, ...) {
    va_list va;
    char stack_buf[512];
    int needed;
    va_start(va, format);
    needed = vsnprintf(stack_buf, sizeof(stack_buf), format, va);
    va_end(va);
    if (needed < 0) {
        PyErr_SetString(PyExc_SystemError, "PyBytes_FromFormat: vsnprintf failed");
        return NULL;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        return _molt_pyobject_from_result(
            molt_bytes_from((const uint8_t *)stack_buf, (uint64_t)needed));
    }
    {
        char *heap_buf = (char *)malloc((size_t)needed + 1);
        PyObject *out;
        if (heap_buf == NULL) {
            PyErr_SetString(PyExc_MemoryError, "PyBytes_FromFormat: allocation failed");
            return NULL;
        }
        va_start(va, format);
        vsnprintf(heap_buf, (size_t)needed + 1, format, va);
        va_end(va);
        out = _molt_pyobject_from_result(
            molt_bytes_from((const uint8_t *)heap_buf, (uint64_t)needed));
        free(heap_buf);
        return out;
    }
}

static inline void PyBytes_Concat(PyObject **bytes, PyObject *newpart) {
    PyObject *result;
    if (bytes == NULL || *bytes == NULL) return;
    if (newpart == NULL) { Py_CLEAR(*bytes); return; }
    result = _molt_pyobject_from_result(molt_add(_molt_py_handle(*bytes), _molt_py_handle(newpart)));
    Py_DECREF(*bytes);
    *bytes = result;
}

static inline void PyBytes_ConcatAndDel(PyObject **bytes, PyObject *newpart) {
    PyBytes_Concat(bytes, newpart);
    Py_XDECREF(newpart);
}

static inline PyObject *PyBytes_DecodeEscape(const char *s, Py_ssize_t len,
                                               const char *errors,
                                               Py_ssize_t unicode,
                                               const char *recode_encoding) {
    (void)errors; (void)unicode; (void)recode_encoding;
    if (s == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_bytes_from((const uint8_t *)s, (uint64_t)len));
}

extern int PyByteArray_Check(PyObject *o);
extern PyObject *PyByteArray_FromStringAndSize(const char *string, Py_ssize_t len);

extern PyObject *PyByteArray_FromObject(PyObject *o);

extern char *PyByteArray_AsString(PyObject *o);
extern Py_ssize_t PyByteArray_Size(PyObject *o);
extern int PyByteArray_Resize(PyObject *o, Py_ssize_t len);
#define PyByteArray_AS_STRING(o) PyByteArray_AsString((PyObject *)(o))
#define PyByteArray_GET_SIZE(o) PyByteArray_Size((PyObject *)(o))

extern PyObject *PyByteArray_Concat(PyObject *a, PyObject *b);

/* ========================================================================
 * Dict (remaining)
 * ======================================================================== */

/* ========================================================================
 * List (remaining)
 * ======================================================================== */

/* ========================================================================
 * Mapping Protocol (remaining)
 * ======================================================================== */

/* ========================================================================
 * Additional Number Protocol: one external ABI authority
 * ======================================================================== */

extern int PyNumber_Check(PyObject *);
extern PyObject *PyNumber_MatrixMultiply(PyObject *, PyObject *);
#define PyNumber_Matmul PyNumber_MatrixMultiply
extern PyObject *PyNumber_InPlacePower(PyObject *, PyObject *, PyObject *);
extern PyObject *PyNumber_InPlaceMatrixMultiply(PyObject *, PyObject *);
#define PyNumber_InPlaceMatmul PyNumber_InPlaceMatrixMultiply
extern PyObject *PyNumber_Divmod(PyObject *, PyObject *);
extern Py_ssize_t PyNumber_AsSsize_t(PyObject *, PyObject *);

/* ========================================================================
 * Additional Dict
 * ======================================================================== */

/* ========================================================================
 * Additional List
 * ======================================================================== */

/* ========================================================================
 * Unicode interning
 * ======================================================================== */

extern PyObject *PyUnicode_InternFromString(const char *v);

extern void PyUnicode_InternInPlace(PyObject **p);

/* ========================================================================
 * Additional memory helpers (aliases)
 * ======================================================================== */

/* ========================================================================
 * Call convenience helpers
 * ======================================================================== */

/* ========================================================================
 * Unicode macro aliases
 * ======================================================================== */

#define PyUnicode_GET_LENGTH(op) PyUnicode_GetLength((PyObject *)(op))

/* ========================================================================
 * Additional memory helpers (aliases)
 * ======================================================================== */

static inline void *PyObject_Calloc(size_t nelem, size_t elsize) {
    return PyMem_Calloc(nelem, elsize);
}

static inline void *PyObject_Realloc(void *ptr, size_t new_size) {
    return PyMem_Realloc(ptr, new_size);
}

/* ========================================================================
 * Frame Object API — lightweight AOT frame for C extension compatibility
 * ======================================================================== */

typedef struct _molt_pycodeobject {
    PyObject ob_base;
    PyObject *co_filename;    /* str: source filename */
    PyObject *co_name;        /* str: function/module name */
    PyObject *co_varnames;    /* tuple of local variable names */
    int co_nlocals;
    int co_nfreevars;
    int co_firstlineno;
} PyCodeObject;

typedef struct _molt_pyframeobject {
    PyObject ob_base;
    struct _molt_pyframeobject *f_back;   /* previous frame (caller) */
    PyCodeObject *f_code;                  /* code object for this frame */
    PyObject *f_builtins;                  /* builtins dict */
    PyObject *f_globals;                   /* module globals dict */
    PyObject *f_locals;                    /* locals dict */
    int f_lineno;                          /* current line number */
    int f_lasti;                           /* last bytecode index (always -1 for AOT) */
} PyFrameObject;

typedef struct _molt_pytracebackobject {
    PyObject ob_base;
    struct _molt_pytracebackobject *tb_next;
    PyFrameObject *tb_frame;
    int tb_lineno;
    int tb_lasti;
} PyTracebackObject;

typedef int (*Py_tracefunc)(PyObject *, PyFrameObject *, int, PyObject *);

/* ---------- frame stack (thread-local, single-threaded in molt) ---------- */

#define _MOLT_FRAME_STACK_MAX 256

static inline PyFrameObject **_molt_frame_stack(void) {
    static PyFrameObject *stack[_MOLT_FRAME_STACK_MAX];
    return stack;
}

static inline int *_molt_frame_stack_top(void) {
    static int top = 0;
    return &top;
}

static inline PyFrameObject *_molt_get_current_frame(void) {
    int top = *_molt_frame_stack_top();
    if (top <= 0) return NULL;
    return _molt_frame_stack()[top - 1];
}

static inline void _molt_push_frame(PyFrameObject *frame) {
    int *top = _molt_frame_stack_top();
    if (*top < _MOLT_FRAME_STACK_MAX) {
        _molt_frame_stack()[*top] = frame;
        (*top)++;
    }
}

static inline void _molt_pop_frame(void) {
    int *top = _molt_frame_stack_top();
    if (*top > 0) {
        (*top)--;
    }
}

/* ---------- code object constructor ---------- */

static inline PyCodeObject *_molt_code_new(const char *filename,
                                            const char *funcname,
                                            int firstlineno) {
    PyCodeObject *co = (PyCodeObject *)PyObject_Malloc(sizeof(PyCodeObject));
    if (co == NULL) return NULL;
    memset(co, 0, sizeof(*co));
    /* ob_base: set refcount to 1 via a raw handle, or just memset is fine
       since C extensions check the fields, not the type pointer */
    co->co_filename = PyUnicode_FromString(filename ? filename : "<molt-compiled>");
    co->co_name = PyUnicode_FromString(funcname ? funcname : "<module>");
    co->co_varnames = PyTuple_New(0);
    co->co_nlocals = 0;
    co->co_nfreevars = 0;
    co->co_firstlineno = firstlineno;
    return co;
}

/* ---------- frame object constructor ---------- */

static inline PyFrameObject *_molt_frame_new(PyCodeObject *code,
                                              PyObject *globals,
                                              PyObject *locals,
                                              int lineno) {
    PyFrameObject *f = (PyFrameObject *)PyObject_Malloc(sizeof(PyFrameObject));
    if (f == NULL) return NULL;
    memset(f, 0, sizeof(*f));
    f->f_back = _molt_get_current_frame();
    f->f_code = code;
    f->f_builtins = PyEval_GetBuiltins();
    f->f_globals = globals ? globals : PyEval_GetBuiltins();
    f->f_locals = locals ? locals : PyDict_New();
    f->f_lineno = lineno;
    f->f_lasti = -1;
    Py_XINCREF(f->f_builtins);
    Py_XINCREF(f->f_globals);
    Py_XINCREF(f->f_locals);
    _molt_push_frame(f);
    return f;
}

static inline void _molt_frame_destroy(PyFrameObject *f) {
    if (f == NULL) return;
    _molt_pop_frame();
    Py_XDECREF(f->f_builtins);
    Py_XDECREF(f->f_globals);
    Py_XDECREF(f->f_locals);
    /* code objects are owned separately — don't free here */
    PyObject_Free(f);
}

/* ---------- PyFrame_* accessors ---------- */

static inline PyObject *PyFrame_GetBack(PyFrameObject *frame) {
    if (frame == NULL || frame->f_back == NULL) {
        Py_RETURN_NONE;
    }
    return (PyObject *)frame->f_back;
}

static inline PyObject *PyFrame_GetBuiltins(PyFrameObject *frame) {
    if (frame == NULL || frame->f_builtins == NULL) {
        Py_RETURN_NONE;
    }
    Py_INCREF(frame->f_builtins);
    return frame->f_builtins;
}

static inline PyObject *PyFrame_GetGlobals(PyFrameObject *frame) {
    if (frame == NULL || frame->f_globals == NULL) {
        Py_RETURN_NONE;
    }
    Py_INCREF(frame->f_globals);
    return frame->f_globals;
}

static inline PyObject *PyFrame_GetLocals(PyFrameObject *frame) {
    if (frame == NULL || frame->f_locals == NULL) {
        Py_RETURN_NONE;
    }
    Py_INCREF(frame->f_locals);
    return frame->f_locals;
}

static inline int PyFrame_GetLineNumber(PyFrameObject *frame) {
    if (frame == NULL) return -1;
    return frame->f_lineno;
}

static inline PyCodeObject *PyFrame_GetCode(PyFrameObject *frame) {
    if (frame == NULL || frame->f_code == NULL) {
        /* Return a default code object so callers never get NULL */
        return _molt_code_new("<molt-compiled>", "<module>", 0);
    }
    /* Return borrowed-ish: caller typically doesn't decref code objects */
    return frame->f_code;
}

static inline PyObject *PyFrame_GetGenerator(PyFrameObject *frame) {
    (void)frame;
    /* Molt AOT does not expose generator objects from frames */
    Py_RETURN_NONE;
}

static inline int PyFrame_GetLasti(PyFrameObject *frame) {
    if (frame == NULL) return -1;
    return frame->f_lasti;
}

static inline PyObject *PyFrame_GetVar(PyFrameObject *frame, PyObject *name) {
    PyObject *val;
    if (frame == NULL || frame->f_locals == NULL || name == NULL) {
        PyErr_SetString(PyExc_NameError, "frame variable not found");
        return NULL;
    }
    val = PyDict_GetItem(frame->f_locals, name);
    if (val == NULL) {
        PyErr_Format(PyExc_NameError, "name not found in frame locals");
        return NULL;
    }
    Py_INCREF(val);
    return val;
}

static inline PyObject *PyFrame_GetVarString(PyFrameObject *frame, const char *name) {
    PyObject *key = PyUnicode_FromString(name);
    PyObject *val;
    if (key == NULL) return NULL;
    val = PyFrame_GetVar(frame, key);
    Py_DECREF(key);
    return val;
}

/* ========================================================================
 * Code Object API
 * ======================================================================== */

/* _molt_code_type_tag: a unique address used to identify code objects */
static inline void *_molt_code_type_tag(void) {
    static int tag = 0;
    return &tag;
}

static inline int PyCode_Check(PyObject *co) {
    /* Heuristic: check if the object looks like our MoltCodeObject.
       Since we allocate code objects ourselves, check co_filename is set. */
    PyCodeObject *c;
    if (co == NULL) return 0;
    c = (PyCodeObject *)co;
    /* If co_filename is a valid string, this is likely a code object */
    return (c->co_filename != NULL && PyUnicode_Check(c->co_filename)) ? 1 : 0;
}

static inline PyObject *PyCode_GetFileName(PyCodeObject *co) {
    if (co == NULL || co->co_filename == NULL) {
        return PyUnicode_FromString("<molt-compiled>");
    }
    Py_INCREF(co->co_filename);
    return co->co_filename;
}

static inline int PyCode_GetNumFree(PyCodeObject *co) {
    if (co == NULL) return 0;
    return co->co_nfreevars;
}

static inline int PyCode_GetFirstFreeVar(PyCodeObject *co) {
    if (co == NULL) return 0;
    return co->co_nlocals;
}

static inline PyObject *PyCode_GetCode(PyCodeObject *co) {
    (void)co;
    /* AOT compiled — no bytecode to return */
    return PyBytes_FromStringAndSize("", 0);
}

static inline PyObject *PyCode_GetVarnames(PyCodeObject *co) {
    if (co != NULL && co->co_varnames != NULL) {
        Py_INCREF(co->co_varnames);
        return co->co_varnames;
    }
    return PyTuple_New(0);
}

static inline PyObject *PyCode_GetFreevars(PyCodeObject *co) {
    (void)co;
    return PyTuple_New(0);
}

static inline PyObject *PyCode_GetCellvars(PyCodeObject *co) {
    (void)co;
    return PyTuple_New(0);
}

/* ========================================================================
 * Traceback API — lightweight linked list for exception chains
 * ======================================================================== */

/* Traceback head for the current exception chain */
static inline PyTracebackObject **_molt_traceback_head(void) {
    static PyTracebackObject *head = NULL;
    return &head;
}

static inline int PyTraceBack_Here(PyFrameObject *frame) {
    PyTracebackObject *tb;
    if (frame == NULL) return 0;
    tb = (PyTracebackObject *)PyObject_Malloc(sizeof(PyTracebackObject));
    if (tb == NULL) return -1;
    memset(tb, 0, sizeof(*tb));
    tb->tb_next = *_molt_traceback_head();
    tb->tb_frame = frame;
    tb->tb_lineno = frame->f_lineno;
    tb->tb_lasti = frame->f_lasti;
    *_molt_traceback_head() = tb;
    return 0;
}

static inline int PyTraceBack_Print(PyObject *tb, PyObject *f) {
    PyTracebackObject *cur = (PyTracebackObject *)tb;
    const char *header = "Traceback (most recent call last):\n";
    (void)f; /* TODO: write to f if it's a real file object */
    if (cur == NULL) return 0;
    fprintf(stderr, "%s", header);
    while (cur != NULL) {
        const char *filename = "<unknown>";
        const char *funcname = "<unknown>";
        if (cur->tb_frame != NULL && cur->tb_frame->f_code != NULL) {
            PyObject *fn = cur->tb_frame->f_code->co_filename;
            PyObject *nm = cur->tb_frame->f_code->co_name;
            if (fn != NULL && PyUnicode_Check(fn)) {
                filename = PyUnicode_AsUTF8(fn);
                if (filename == NULL) filename = "<unknown>";
            }
            if (nm != NULL && PyUnicode_Check(nm)) {
                funcname = PyUnicode_AsUTF8(nm);
                if (funcname == NULL) funcname = "<unknown>";
            }
        }
        fprintf(stderr, "  File \"%s\", line %d, in %s\n",
                filename, cur->tb_lineno, funcname);
        cur = cur->tb_next;
    }
    return 0;
}

PyAPI_DATA(PyTypeObject) PyTraceBack_Type;

static inline int PyTraceBack_Check(PyObject *ob) {
    return ob != NULL && Py_TYPE(ob) == &PyTraceBack_Type;
}

static inline PyObject *PyTraceBack_GetObject(PyObject *tb) {
    if (tb == NULL) {
        Py_RETURN_NONE;
    }
    Py_INCREF(tb);
    return tb;
}

/* ========================================================================
 * PyStructSequence API
 * ======================================================================== */

typedef struct {
    const char *name;
    const char *doc;
} PyStructSequence_Field;

typedef struct {
    const char *name;
    const char *doc;
    int n_in_sequence;
    PyStructSequence_Field *fields;
} PyStructSequence_Desc;

static inline int PyStructSequence_InitType2(PyTypeObject *type, PyStructSequence_Desc *desc) {
    (void)type; (void)desc;
    return 0;
}

static inline void PyStructSequence_InitType(PyTypeObject *type, PyStructSequence_Desc *desc) {
    (void)PyStructSequence_InitType2(type, desc);
}

static inline PyObject *PyStructSequence_New(PyTypeObject *type) {
    (void)type;
    return PyTuple_New(0);
}

static inline PyObject *PyStructSequence_GetItem(PyObject *p, Py_ssize_t pos) {
    return PyTuple_GetItem(p, pos);
}

static inline void PyStructSequence_SetItem(PyObject *p, Py_ssize_t pos, PyObject *o) {
    PyTuple_SetItem(p, pos, o);
}

#define PyStructSequence_SET_ITEM(p, pos, o) PyStructSequence_SetItem((p), (pos), (o))
#define PyStructSequence_GET_ITEM(p, pos) PyStructSequence_GetItem((p), (pos))

static inline PyTypeObject *PyStructSequence_NewType(PyStructSequence_Desc *desc) {
    (void)desc;
    return &PyTuple_Type;
}

/* PyCFunction/PyCMethod constructors and inspection are linkable ABI calls. */

static inline PyObject *PyInstanceMethod_New(PyObject *func) {
    Py_INCREF(func);
    return func;
}

static inline int PyInstanceMethod_Check(PyObject *op) {
    (void)op;
    return 0;
}

static inline PyObject *PyInstanceMethod_Function(PyObject *im) {
    Py_INCREF(im);
    return im;
}

#define PyInstanceMethod_GET_FUNCTION(im) (im)

/* ========================================================================
 * Property
 * ======================================================================== */

static inline PyObject *PyProperty_New(PyObject *fget, PyObject *fset,
                                        PyObject *fdel, PyObject *doc) {
    (void)fget; (void)fset; (void)fdel; (void)doc;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyProperty_New: not yet implemented in molt");
    return NULL;
}

/* ========================================================================
 * Cell / Generator / Coroutine
 * ======================================================================== */

static inline PyObject *PyCell_New(PyObject *ob) {
    PyObject *cell = PyTuple_New(1);
    if (cell == NULL) return NULL;
    if (ob != NULL) {
        Py_INCREF(ob);
        if (PyTuple_SetItem(cell, 0, ob) != 0) {
            Py_DECREF(ob);  /* SetItem failed — undo our incref */
            Py_DECREF(cell);
            return NULL;
        }
    }
    return cell;
}

static inline PyObject *PyCell_Get(PyObject *cell) {
    if (cell == NULL) {
        PyErr_SetString(PyExc_SystemError, "PyCell_Get: NULL cell");
        return NULL;
    }
    PyObject *contents = PyTuple_GetItem(cell, 0);
    Py_XINCREF(contents);
    return contents;
}

static inline int PyCell_Set(PyObject *cell, PyObject *value) {
    if (cell == NULL) {
        PyErr_SetString(PyExc_SystemError, "PyCell_Set: NULL cell");
        return -1;
    }
    Py_XINCREF(value);
    PyTuple_SetItem(cell, 0, value);
    return 0;
}

static inline int PyCell_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyGen_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyGen_CheckExact(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyCoro_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyCoro_CheckExact(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyAsyncGen_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline int PyAsyncGen_CheckExact(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline PyObject *PyGen_New(PyFrameObject *frame) {
    (void)frame;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyGen_New: generators are compiled natively in molt");
    return NULL;
}

static inline PyObject *PyGen_NewWithQualName(PyFrameObject *frame,
                                               PyObject *name, PyObject *qualname) {
    (void)frame; (void)name; (void)qualname;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyGen_NewWithQualName: generators are compiled natively in molt");
    return NULL;
}

static inline PyObject *PyCoro_New(PyFrameObject *frame, PyObject *name, PyObject *qualname) {
    (void)frame; (void)name; (void)qualname;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyCoro_New: coroutines are compiled natively in molt");
    return NULL;
}

/* ========================================================================
 * Exception creation helpers
 * ======================================================================== */

extern PyObject *PyException_GetTraceback(PyObject *ex);
extern int PyException_SetTraceback(PyObject *ex, PyObject *tb);
extern PyObject *PyException_GetCause(PyObject *ex);
extern void PyException_SetCause(PyObject *ex, PyObject *cause);
extern PyObject *PyException_GetContext(PyObject *ex);
extern void PyException_SetContext(PyObject *ex, PyObject *context);
extern PyObject *PyException_GetArgs(PyObject *ex);
extern void PyException_SetArgs(PyObject *ex, PyObject *args);
extern PyObject *PyErr_NewException(const char *name, PyObject *base, PyObject *dict);
extern PyObject *PyErr_NewExceptionWithDoc(const char *name, const char *doc,
                                           PyObject *base, PyObject *dict);

static inline const char *_molt_strerror(int errnum, char *buffer, size_t buffer_len) {
#ifdef _WIN32
    if (buffer == NULL || buffer_len == 0) {
        return "unknown error";
    }
    if (strerror_s(buffer, buffer_len, errnum) == 0) {
        return buffer;
    }
    return "unknown error";
#else
    const char *msg = strerror(errnum);
    (void)buffer;
    (void)buffer_len;
    return msg != NULL ? msg : "unknown error";
#endif
}

static inline PyObject *PyErr_SetFromErrno(PyObject *type) {
    char msg_buf[256];
    const char *msg;
    if (type == NULL) type = PyExc_OSError;
    msg = _molt_strerror(errno, msg_buf, sizeof(msg_buf));
    PyErr_SetString(type, msg);
    return NULL;
}

static inline PyObject *PyErr_SetFromErrnoWithFilenameObject(PyObject *type, PyObject *filenameObject) {
    (void)filenameObject;
    return PyErr_SetFromErrno(type);
}

static inline PyObject *PyErr_SetFromErrnoWithFilenameObjects(PyObject *type,
                                                               PyObject *filenameObject,
                                                               PyObject *filenameObject2) {
    (void)filenameObject; (void)filenameObject2;
    return PyErr_SetFromErrno(type);
}

static inline PyObject *PyErr_SetImportError(PyObject *msg, PyObject *name, PyObject *path) {
    (void)name; (void)path;
    if (msg != NULL) {
        PyErr_SetObject(PyExc_ImportError, msg);
    } else {
        PyErr_SetString(PyExc_ImportError, "import error");
    }
    return NULL;
}

static inline PyObject *PyErr_SetImportErrorSubclass(PyObject *exception, PyObject *msg,
                                                      PyObject *name, PyObject *path) {
    (void)exception; (void)name; (void)path;
    if (msg != NULL) {
        PyErr_SetObject(PyExc_ImportError, msg);
    } else {
        PyErr_SetString(PyExc_ImportError, "import error");
    }
    return NULL;
}

/* Signal C-API: projections of the runtime signal authority. Pending Python
   handlers run only on the registered main thread. */
int PyErr_CheckSignals(void);
void PyErr_SetInterrupt(void);
int PyErr_SetInterruptEx(int signum);

static inline void PyErr_Display(PyObject *exception, PyObject *value, PyObject *tb) {
    (void)exception; (void)tb;
    if (value != NULL) {
        PyObject *str = PyObject_Str(value);
        if (str != NULL) {
            const char *s = PyUnicode_AsUTF8(str);
            if (s != NULL) fprintf(stderr, "%s\n", s);
            Py_DECREF(str);
        }
    }
}

static inline int PyErr_WarnExplicitObject(PyObject *category, PyObject *message,
                                            PyObject *filename, int lineno,
                                            PyObject *module, PyObject *registry) {
    (void)category; (void)filename; (void)lineno; (void)module; (void)registry;
    const char *msg = PyUnicode_AsUTF8(message);
    if (msg == NULL) return -1;
    fprintf(stderr, "Warning: %s\n", msg);
    return 0;
}

static inline int PyErr_WarnExplicit(PyObject *category, const char *message,
                                      const char *filename, int lineno,
                                      const char *module, PyObject *registry) {
    (void)category; (void)filename; (void)lineno; (void)module; (void)registry;
    fprintf(stderr, "Warning: %s\n", message);
    return 0;
}

/* ========================================================================
 * Py_AtExit / Py_FinalizeEx
 * ======================================================================== */

static inline int Py_AtExit(void (*func)(void)) {
    (void)func;
    return 0;
}

static inline int Py_FinalizeEx(void) {
    return 0;
}

static inline void Py_InitializeEx(int initsigs) {
    (void)initsigs;
}

/* ========================================================================
 * PyRun API — molt does not support runtime eval/exec
 * ======================================================================== */

typedef struct {
    int cf_flags;
    int cf_feature_version;
} PyCompilerFlags;

#define _MOLT_NO_EVAL_MSG \
    "molt does not support runtime eval/exec — use compile-time imports"

static inline PyObject *PyRun_StringFlags(const char *str, int start,
                                           PyObject *globals, PyObject *locals,
                                           PyCompilerFlags *flags) {
    (void)str; (void)start; (void)globals; (void)locals; (void)flags;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return NULL;
}

static inline PyObject *PyRun_String(const char *str, int start,
                                      PyObject *globals, PyObject *locals) {
    return PyRun_StringFlags(str, start, globals, locals, NULL);
}

static inline int PyRun_SimpleStringFlags(const char *command, PyCompilerFlags *flags) {
    (void)command; (void)flags;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return -1;
}

static inline int PyRun_SimpleString(const char *command) {
    return PyRun_SimpleStringFlags(command, NULL);
}

static inline int PyRun_AnyFileFlags(FILE *fp, const char *filename, PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)flags;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return -1;
}

static inline int PyRun_AnyFile(FILE *fp, const char *filename) {
    return PyRun_AnyFileFlags(fp, filename, NULL);
}

static inline int PyRun_AnyFileExFlags(FILE *fp, const char *filename, int closeit,
                                        PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)closeit; (void)flags;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return -1;
}

static inline PyObject *PyRun_FileFlags(FILE *fp, const char *filename, int start,
                                         PyObject *globals, PyObject *locals,
                                         PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)start; (void)globals; (void)locals; (void)flags;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return NULL;
}

static inline PyObject *PyRun_File(FILE *fp, const char *filename, int start,
                                    PyObject *globals, PyObject *locals) {
    return PyRun_FileFlags(fp, filename, start, globals, locals, NULL);
}

static inline PyObject *Py_CompileString(const char *str, const char *filename, int start) {
    (void)str; (void)filename; (void)start;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return NULL;
}

static inline PyObject *Py_CompileStringFlags(const char *str, const char *filename,
                                               int start, PyCompilerFlags *flags) {
    (void)flags;
    return Py_CompileString(str, filename, start);
}

static inline PyObject *Py_CompileStringExFlags(const char *str, const char *filename,
                                                  int start, PyCompilerFlags *flags,
                                                  int optimize) {
    (void)optimize;
    return Py_CompileStringFlags(str, filename, start, flags);
}

/* ========================================================================
 * PyEval API — GIL delegates to molt runtime, eval raises RuntimeError
 * ======================================================================== */

static inline void PyEval_AcquireLock(void) {
    (void)molt_gil_acquire();
}

static inline void PyEval_ReleaseLock(void) {
    (void)molt_gil_release();
}

static inline void PyEval_AcquireThread(PyThreadState *tstate) {
    (void)tstate;
    (void)molt_gil_acquire();
}

static inline void PyEval_ReleaseThread(PyThreadState *tstate) {
    (void)tstate;
    (void)molt_gil_release();
}

static inline PyThreadState *PyEval_SaveThread(void) {
    PyThreadState *ts = PyThreadState_Get();
    (void)molt_gil_release();
    return ts;
}

static inline void PyEval_RestoreThread(PyThreadState *tstate) {
    (void)tstate;
    (void)molt_gil_acquire();
}

static inline PyFrameObject *PyEval_GetFrame(void) {
    return _molt_get_current_frame();
}

static inline int PyEval_MergeCompilerFlags(PyCompilerFlags *cf) {
    if (cf != NULL) {
        cf->cf_flags = 0;
        cf->cf_feature_version = 12; /* Python 3.12 compat */
    }
    return 0;
}

static inline PyObject *PyEval_EvalCode(PyObject *co, PyObject *globals, PyObject *locals) {
    (void)co; (void)globals; (void)locals;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return NULL;
}

static inline PyObject *PyEval_EvalCodeEx(PyObject *co, PyObject *globals, PyObject *locals,
                                           PyObject *const *args, int argcount,
                                           PyObject *const *kws, int kwcount,
                                           PyObject *const *defs, int defcount,
                                           PyObject *kwdefs, PyObject *closure) {
    (void)co; (void)globals; (void)locals;
    (void)args; (void)argcount; (void)kws; (void)kwcount;
    (void)defs; (void)defcount; (void)kwdefs; (void)closure;
    PyErr_SetString(PyExc_RuntimeError, _MOLT_NO_EVAL_MSG);
    return NULL;
}

/* ========================================================================
 * PyGILState additional
 * ======================================================================== */

static inline PyThreadState *PyGILState_GetThisThreadState(void) {
    return PyThreadState_Get();
}

/* ========================================================================
 * PyThreadState / PyInterpreterState additional
 * ======================================================================== */

static inline PyThreadState *PyThreadState_New(PyInterpreterState *interp) {
    (void)interp;
    return PyThreadState_Get();
}

static inline void PyThreadState_Delete(PyThreadState *tstate) {
    (void)tstate;
}

static inline PyInterpreterState *PyThreadState_GetInterpreter(PyThreadState *tstate) {
    (void)tstate;
    static PyInterpreterState _molt_main_interp = {0};
    return &_molt_main_interp;
}

/* Forward declaration — implemented after MoltFrameObject is defined */
static inline PyFrameObject *_molt_get_current_frame(void);
static inline PyFrameObject *PyThreadState_GetFrame(PyThreadState *tstate) {
    (void)tstate;
    return _molt_get_current_frame();
}

static inline uint64_t PyThreadState_GetID(PyThreadState *tstate) {
    (void)tstate;
    return 1;
}

static inline int64_t PyInterpreterState_GetID(PyInterpreterState *interp) {
    (void)interp;
    return 0;
}

static inline PyObject *PyInterpreterState_GetDict(PyInterpreterState *interp) {
    static PyObject *cached_dict = NULL;
    (void)interp;
    if (cached_dict == NULL) {
        cached_dict = PyDict_New();
    }
    Py_XINCREF(cached_dict);
    return cached_dict;
}

/* ========================================================================
 * Version / Platform info
 * ======================================================================== */

static inline const char *Py_GetVersion(void) {
    return "3.12.0 (molt AOT)";
}

static inline const char *Py_GetPlatform(void) {
#if defined(__APPLE__)
    return "darwin";
#elif defined(__linux__)
    return "linux";
#elif defined(_WIN32)
    return "win32";
#elif defined(__wasm__)
    return "wasi";
#else
    return "unknown";
#endif
}

static inline const char *Py_GetCopyright(void) {
    return "Copyright (c) molt contributors";
}

static inline const char *Py_GetCompiler(void) {
#if defined(__clang__)
    return "[Clang " __clang_version__ "]";
#elif defined(__GNUC__)
    return "[GCC]";
#elif defined(_MSC_VER)
    return "[MSVC]";
#else
    return "[Unknown]";
#endif
}

static inline const char *Py_GetBuildInfo(void) {
    return "molt AOT compiled";
}

static inline const char *Py_GetProgramName(void) {
    return "molt";
}

static inline const char *Py_GetProgramFullPath(void) {
    return "molt";
}

static inline const char *Py_GetPrefix(void) {
    return "";
}

static inline const char *Py_GetExecPrefix(void) {
    return "";
}

static inline const char *Py_GetPath(void) {
    return "";
}

static inline const char *Py_GetPythonHome(void) {
    return "";
}

/* ========================================================================
 * Py_Is / identity checks
 * ======================================================================== */

static inline int Py_Is(PyObject *x, PyObject *y) {
    return x == y;
}

static inline int Py_IsNone(PyObject *x) {
    return Py_Is(x, Py_None);
}

static inline int Py_IsTrue(PyObject *x) {
    return Py_Is(x, Py_True);
}

static inline int Py_IsFalse(PyObject *x) {
    return Py_Is(x, Py_False);
}

/* ========================================================================
 * Recursive call guards
 * ======================================================================== */

static inline int Py_EnterRecursiveCall(const char *where) {
    (void)where;
    return 0;
}

static inline void Py_LeaveRecursiveCall(void) {
    /* no-op */
}

/* ========================================================================
 * PyFloat_FromString
 * ======================================================================== */

/* ========================================================================
 * Py_SIZE
 * ======================================================================== */

static inline Py_ssize_t _molt_py_size(PyObject *obj) {
    if (_molt_c_heap_object_is(obj)) {
        _molt_c_heap_fatal("Py_SIZE requires a canonical PyVarObject header");
    }
    return ((PyVarObject *)obj)->ob_size;
}

static inline void _molt_py_set_size(PyObject *obj, Py_ssize_t size) {
    if (_molt_c_heap_object_is(obj)) {
        _molt_c_heap_fatal("Py_SET_SIZE requires a canonical PyVarObject header");
    }
    ((PyVarObject *)obj)->ob_size = size;
}

#define Py_SIZE(ob) _molt_py_size((PyObject *)(ob))
#define Py_SET_SIZE(ob, size) _molt_py_set_size((PyObject *)(ob), (Py_ssize_t)(size))

/* ========================================================================
 * Utility macros
 * ======================================================================== */

#ifndef Py_STRINGIFY
#define Py_STRINGIFY(x) #x
#define Py_XSTRINGIFY(x) Py_STRINGIFY(x)
#endif

#ifndef Py_UNREACHABLE
#ifdef __GNUC__
#define Py_UNREACHABLE() __builtin_unreachable()
#elif defined(_MSC_VER)
#define Py_UNREACHABLE() __assume(0)
#else
#define Py_UNREACHABLE() abort()
#endif
#endif

#ifndef Py_ABS
#define Py_ABS(x) ((x) >= 0 ? (x) : -(x))
#endif

#ifndef Py_MIN
#define Py_MIN(x, y) (((x) < (y)) ? (x) : (y))
#endif

#ifndef Py_MAX
#define Py_MAX(x, y) (((x) > (y)) ? (x) : (y))
#endif

#ifndef Py_ARRAY_LENGTH
#define Py_ARRAY_LENGTH(a) (sizeof(a) / sizeof((a)[0]))
#endif

#ifndef Py_MEMBER_SIZE
#define Py_MEMBER_SIZE(type, member) sizeof(((type *)0)->member)
#endif

#ifndef Py_SAFE_DOWNCAST
#define Py_SAFE_DOWNCAST(VALUE, WIDE, NARROW) ((NARROW)(VALUE))
#endif

#ifndef Py_CHARMASK
#define Py_CHARMASK(c) ((unsigned char)((c) & 0xff))
#endif

#ifndef Py_DEPRECATED
#if defined(__GNUC__) || defined(__clang__)
#define Py_DEPRECATED(VERSION_UNUSED) __attribute__((deprecated))
#else
#define Py_DEPRECATED(VERSION_UNUSED)
#endif
#endif

#ifndef PyDoc_STR
#define PyDoc_STR(str) str
#endif

#ifndef PyDoc_STRVAR
#define PyDoc_STRVAR(name, str) static const char name[] = str
#endif

#ifndef PyDoc_VAR
#define PyDoc_VAR(name) static const char name[]
#endif

/* ========================================================================
 * Eval / start token constants
 * ======================================================================== */

#ifndef Py_eval_input
#define Py_eval_input 258
#endif

#ifndef Py_file_input
#define Py_file_input 257
#endif

#ifndef Py_single_input
#define Py_single_input 256
#endif

/* ========================================================================
 * PySys additional
 * ======================================================================== */

static inline void PySys_AddWarnOption(const char *s) {
    (void)s;
}

static inline void PySys_AddWarnOptionUnicode(PyObject *option) {
    (void)option;
}

static inline void PySys_SetPath(const char *path) {
    (void)path;
}

static inline void PySys_SetArgv(int argc, char **argv) {
    (void)argc; (void)argv;
}

static inline void PySys_SetArgvEx(int argc, char **argv, int updatepath) {
    (void)argc; (void)argv; (void)updatepath;
}

static inline void PySys_AddXOption(const char *s) {
    (void)s;
}

static inline PyObject *PySys_GetXOptions(void) {
    return PyDict_New();
}

/* ========================================================================
 * PyImport additional
 * ======================================================================== */

static inline PyObject *PyImport_AddModuleObject(PyObject *name) {
    (void)name;
    Py_RETURN_NONE;
}

static inline PyObject *PyImport_ExecCodeModule(const char *name, PyObject *co) {
    (void)name; (void)co;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyImport_ExecCodeModule: not supported in molt");
    return NULL;
}

static inline PyObject *PyImport_ExecCodeModuleEx(const char *name, PyObject *co,
                                                    const char *pathname) {
    (void)pathname;
    return PyImport_ExecCodeModule(name, co);
}

static inline PyObject *PyImport_ExecCodeModuleWithPathnames(const char *name, PyObject *co,
                                                              const char *pathname,
                                                              const char *cpathname) {
    (void)pathname; (void)cpathname;
    return PyImport_ExecCodeModule(name, co);
}

static inline long PyImport_GetMagicNumber(void) {
    return 3531;
}

static inline const char *PyImport_GetMagicTag(void) {
    return "cpython-312";
}

static inline int PyImport_ImportFrozenModuleObject(PyObject *name) {
    (void)name;
    return 0;
}

/* ========================================================================
 * Py_BEGIN/END_ALLOW_THREADS
 * ======================================================================== */

#ifndef Py_PRINT_RAW
#define Py_PRINT_RAW 1
#endif

#ifndef Py_BEGIN_ALLOW_THREADS
#define Py_BEGIN_ALLOW_THREADS {
#endif

#ifndef Py_END_ALLOW_THREADS
#define Py_END_ALLOW_THREADS }
#endif

#ifndef Py_BLOCK_THREADS
#define Py_BLOCK_THREADS
#endif

#ifndef Py_UNBLOCK_THREADS
#define Py_UNBLOCK_THREADS
#endif

/* All C-method conventions and type slot IDs are defined above. */

/* ========================================================================
 * PyLong additional conversions
 * ======================================================================== */

/* ========================================================================
 * PyNumber_ToBase
 * ======================================================================== */

static inline PyObject *PyNumber_ToBase(PyObject *n, int base) {
    PyObject *idx, *builtins_mod, *func, *result;
    const char *func_name;
    if (n == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    /* First ensure we have an integer via __index__ */
    idx = PyNumber_Index(n);
    if (idx == NULL) return NULL;
    switch (base) {
    case 2:  func_name = "bin"; break;
    case 8:  func_name = "oct"; break;
    case 10:
        /* base 10: just return str(idx) */
        result = PyObject_Str(idx);
        Py_DECREF(idx);
        return result;
    case 16: func_name = "hex"; break;
    default:
        Py_DECREF(idx);
        PyErr_SetString(PyExc_ValueError,
            "PyNumber_ToBase: base must be 2, 8, 10, or 16");
        return NULL;
    }
    builtins_mod = PyImport_ImportModule("builtins");
    if (builtins_mod == NULL) { Py_DECREF(idx); return NULL; }
    func = PyObject_GetAttrString(builtins_mod, func_name);
    Py_DECREF(builtins_mod);
    if (func == NULL) { Py_DECREF(idx); return NULL; }
    result = PyObject_CallOneArg(func, idx);
    Py_DECREF(func);
    Py_DECREF(idx);
    return result;
}

/* ========================================================================
 * _Py_Identifier
 * ======================================================================== */

typedef struct _Py_Identifier {
    const char *string;
    PyObject *object;
} _Py_Identifier;

#define _Py_IDENTIFIER(varname) \
    static _Py_Identifier PyId_##varname = { .string = #varname, .object = NULL }

#define _Py_static_string(varname, value) \
    static _Py_Identifier varname = { .string = value, .object = NULL }

/* ========================================================================
 * Py_SetProgramName / Py_SetPythonHome
 * ======================================================================== */

static inline void Py_SetProgramName(const wchar_t *name) {
    (void)name;
}

static inline void Py_SetPythonHome(const wchar_t *home) {
    (void)home;
}

/* ========================================================================
 * PyObject_GetAIter / PyAIter_Check
 * ======================================================================== */

static inline PyObject *PyObject_GetAIter(PyObject *o) {
    PyObject *meth = PyObject_GetAttrString(o, "__aiter__");
    if (meth == NULL) return NULL;
    PyObject *result = PyObject_CallNoArgs(meth);
    Py_DECREF(meth);
    return result;
}

static inline int PyAIter_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

/* ========================================================================
 * PyUnicode additional
 * ======================================================================== */

static inline int PyUnicode_FSConverter(PyObject *arg, void *addr) {
    PyObject **result = (PyObject **)addr;
    if (PyUnicode_Check(arg)) {
        *result = PyUnicode_AsEncodedString(arg, "utf-8", "surrogateescape");
        return *result != NULL ? 1 : 0;
    }
    return 0;
}

static inline int PyUnicode_FSDecoder(PyObject *arg, void *addr) {
    PyObject **result = (PyObject **)addr;
    if (PyUnicode_Check(arg)) {
        Py_INCREF(arg);
        *result = arg;
        return 1;
    }
    return 0;
}

static inline PyObject *PyUnicode_DecodeFSDefault(const char *s) {
    return PyUnicode_FromString(s);
}

static inline PyObject *PyUnicode_DecodeFSDefaultAndSize(const char *s, Py_ssize_t size) {
    return PyUnicode_FromStringAndSize(s, size);
}

static inline PyObject *PyUnicode_EncodeFSDefault(PyObject *unicode) {
    return PyUnicode_AsEncodedString(unicode, "utf-8", "surrogateescape");
}

static inline PyObject *PyUnicode_RichCompare(PyObject *left, PyObject *right, int op) {
    return PyObject_RichCompare(left, right, op);
}

static inline PyObject *PyUnicode_Splitlines(PyObject *s, int keepends) {
    return PyObject_CallMethod(s, "splitlines", "(i)", keepends);
}

static inline PyObject *PyUnicode_Partition(PyObject *s, PyObject *sep) {
    return PyObject_CallMethod(s, "partition", "(O)", sep);
}

static inline PyObject *PyUnicode_RPartition(PyObject *s, PyObject *sep) {
    return PyObject_CallMethod(s, "rpartition", "(O)", sep);
}

extern PyObject *PyUnicode_FromOrdinal(int ordinal);

/* ========================================================================
 * Item deletion macro
 * ======================================================================== */

#ifndef PyObject_DelItem
#define PyObject_DelItem(o, key) PyObject_SetItem((o), (key), NULL)
#endif

#ifndef PyMapping_DelItem
#define PyMapping_DelItem(o, key) PyObject_DelItem((o), (key))
#endif

#ifndef PyMapping_DelItemString
#endif

/* ========================================================================
 * Py_VISIT / Py_TRASHCAN macros (GC support)
 * ======================================================================== */

#ifndef Py_VISIT
#define Py_VISIT(op) \
    do { \
        if (op) { \
            int vret = visit((PyObject *)(op), arg); \
            if (vret) return vret; \
        } \
    } while (0)
#endif

#ifndef Py_TRASHCAN_BEGIN
#define Py_TRASHCAN_BEGIN(op, dealloc) {
#endif

#ifndef Py_TRASHCAN_END
#define Py_TRASHCAN_END }
#endif

/* ========================================================================
 * GC controls use the linked runtime owner. Private C-heap headers have no
 * CPython layout or trace authority and cannot enter this object family.
 * ======================================================================== */

#include "shared/_gc_exports.h"

static inline PyObject *_molt_gc_new(PyTypeObject *type) {
    if (_molt_c_heap_object_is((PyObject *)type)) {
        _molt_c_heap_fatal("_PyObject_GC_New cannot allocate a private C-heap type");
    }
    return _PyObject_GC_New(type);
}

static inline PyVarObject *_molt_gc_new_var(PyTypeObject *type, Py_ssize_t size) {
    if (_molt_c_heap_object_is((PyObject *)type)) {
        _molt_c_heap_fatal("_PyObject_GC_NewVar cannot allocate a private C-heap type");
    }
    return _PyObject_GC_NewVar(type, size);
}

static inline void _molt_gc_track(void *obj) {
    if (_molt_c_heap_object_is((PyObject *)obj)) {
        _molt_c_heap_fatal("PyObject_GC_Track has no trace authority for private C-heap objects");
    }
    PyObject_GC_Track(obj);
}

static inline void _molt_gc_untrack(void *obj) {
    if (!_molt_c_heap_object_is((PyObject *)obj)) {
        PyObject_GC_UnTrack(obj);
    }
}

static inline void _molt_gc_del(void *obj) {
    if (_molt_c_heap_object_is((PyObject *)obj)) {
        _molt_c_heap_fatal("PyObject_GC_Del cannot free a registered private C-heap object");
    }
    PyObject_GC_Del(obj);
}

static inline int _molt_gc_is_tracked(PyObject *obj) {
    return _molt_c_heap_object_is(obj) ? 0 : PyObject_GC_IsTracked(obj);
}

static inline int _molt_gc_is_finalized(PyObject *obj) {
    return _molt_c_heap_object_is(obj) ? 0 : PyObject_GC_IsFinalized(obj);
}

#undef _PyObject_GC_New
#undef _PyObject_GC_NewVar
#undef PyObject_GC_Track
#undef PyObject_GC_UnTrack
#undef PyObject_GC_Del
#undef PyObject_GC_IsTracked
#undef PyObject_GC_IsFinalized
#define _PyObject_GC_New _molt_gc_new
#define _PyObject_GC_NewVar _molt_gc_new_var
#define PyObject_GC_Track _molt_gc_track
#define PyObject_GC_UnTrack _molt_gc_untrack
#define PyObject_GC_Del _molt_gc_del
#define PyObject_GC_IsTracked _molt_gc_is_tracked
#define PyObject_GC_IsFinalized _molt_gc_is_finalized

/* ========================================================================
 * PyObject arena / memory allocator structs
 * ======================================================================== */

typedef struct {
    void *ctx;
    void *(*alloc)(void *ctx, size_t size);
    void (*free)(void *ctx, void *ptr, size_t size);
} PyObjectArenaAllocator;

static inline void PyObject_GetArenaAllocator(PyObjectArenaAllocator *allocator) {
    if (allocator) memset(allocator, 0, sizeof(*allocator));
}

static inline void PyObject_SetArenaAllocator(PyObjectArenaAllocator *allocator) {
    (void)allocator;
}

typedef enum {
    PYMEM_DOMAIN_RAW = 0,
    PYMEM_DOMAIN_MEM = 1,
    PYMEM_DOMAIN_OBJ = 2
} PyMemAllocatorDomain;

typedef struct {
    void *ctx;
    void *(*malloc)(void *ctx, size_t size);
    void *(*calloc)(void *ctx, size_t nelem, size_t elsize);
    void *(*realloc)(void *ctx, void *ptr, size_t new_size);
    void (*free)(void *ctx, void *ptr);
} PyMemAllocatorEx;

static inline void PyMem_GetAllocator(PyMemAllocatorDomain domain, PyMemAllocatorEx *allocator) {
    (void)domain;
    if (allocator) memset(allocator, 0, sizeof(*allocator));
}

static inline void PyMem_SetAllocator(PyMemAllocatorDomain domain, PyMemAllocatorEx *allocator) {
    (void)domain; (void)allocator;
}

static inline void PyMem_SetupDebugHooks(void) {
    /* no-op */
}

/* ========================================================================
 * Member types for PyMemberDef
 * ======================================================================== */

#ifndef T_SHORT
#define Py_T_SHORT 0
#define Py_T_INT 1
#define Py_T_LONG 2
#define Py_T_FLOAT 3
#define Py_T_DOUBLE 4
#define Py_T_STRING 5
#define _Py_T_OBJECT 6
#define Py_T_OBJECT _Py_T_OBJECT
#define Py_T_CHAR 7
#define Py_T_BYTE 8
#define Py_T_UBYTE 9
#define Py_T_USHORT 10
#define Py_T_UINT 11
#define Py_T_ULONG 12
#define Py_T_STRING_INPLACE 13
#define Py_T_BOOL 14
#define Py_T_OBJECT_EX 16
#define Py_T_LONGLONG 17
#define Py_T_ULONGLONG 18
#define Py_T_PYSSIZET 19
#define _Py_T_NONE 20
#define Py_T_NONE _Py_T_NONE
#define T_SHORT Py_T_SHORT
#define T_INT Py_T_INT
#define T_LONG Py_T_LONG
#define T_FLOAT Py_T_FLOAT
#define T_DOUBLE Py_T_DOUBLE
#define T_STRING Py_T_STRING
#define T_OBJECT _Py_T_OBJECT
#define T_CHAR Py_T_CHAR
#define T_BYTE Py_T_BYTE
#define T_UBYTE Py_T_UBYTE
#define T_USHORT Py_T_USHORT
#define T_UINT Py_T_UINT
#define T_ULONG Py_T_ULONG
#define T_STRING_INPLACE Py_T_STRING_INPLACE
#define T_BOOL Py_T_BOOL
#define T_OBJECT_EX Py_T_OBJECT_EX
#define T_LONGLONG Py_T_LONGLONG
#define T_ULONGLONG Py_T_ULONGLONG
#define T_PYSSIZET Py_T_PYSSIZET
#define T_NONE _Py_T_NONE
#endif

#ifndef Py_READONLY
#define Py_READONLY 1
#endif

#ifndef Py_AUDIT_READ
#define Py_AUDIT_READ 2
#endif

#ifndef Py_RELATIVE_OFFSET
#define Py_RELATIVE_OFFSET 8
#endif

#ifndef _Py_WRITE_RESTRICTED
#define _Py_WRITE_RESTRICTED 4
#endif

#ifndef READONLY
#define READONLY Py_READONLY
#endif

#ifndef READ_RESTRICTED
#define READ_RESTRICTED Py_AUDIT_READ
#endif

#ifndef PY_WRITE_RESTRICTED
#define PY_WRITE_RESTRICTED _Py_WRITE_RESTRICTED
#endif

#ifndef RESTRICTED
#define RESTRICTED (READ_RESTRICTED | PY_WRITE_RESTRICTED)
#endif

/* Float constants */
#ifndef Py_NAN
#define Py_NAN ((double)(0.0 / 0.0))
#endif
#ifndef Py_HUGE_VAL
#define Py_HUGE_VAL HUGE_VAL
#endif

/* Buffer protocol flags */
#ifndef PyBUF_SIMPLE
#define PyBUF_SIMPLE 0
#endif
#ifndef PyBUF_WRITABLE
#define PyBUF_WRITABLE 0x0001
#endif
#ifndef PyBUF_WRITEABLE
#define PyBUF_WRITEABLE PyBUF_WRITABLE
#endif
#ifndef PyBUF_READ
#define PyBUF_READ 0x0100
#endif
#ifndef PyBUF_WRITE
#define PyBUF_WRITE 0x0200
#endif
#ifndef PyBUF_FORMAT
#define PyBUF_FORMAT 0x0004
#endif
#ifndef PyBUF_ND
#define PyBUF_ND 0x0008
#endif
#ifndef PyBUF_STRIDES
#define PyBUF_STRIDES (0x0010 | PyBUF_ND)
#endif
#ifndef PyBUF_C_CONTIGUOUS
#define PyBUF_C_CONTIGUOUS (0x0020 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_F_CONTIGUOUS
#define PyBUF_F_CONTIGUOUS (0x0040 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_ANY_CONTIGUOUS
#define PyBUF_ANY_CONTIGUOUS (0x0080 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_INDIRECT
#define PyBUF_INDIRECT (0x0100 | PyBUF_STRIDES)
#endif
#ifndef PyBUF_CONTIG_RO
#define PyBUF_CONTIG_RO PyBUF_ND
#endif
#ifndef PyBUF_CONTIG
#define PyBUF_CONTIG (PyBUF_ND | PyBUF_WRITABLE)
#endif
#ifndef PyBUF_RECORDS_RO
#define PyBUF_RECORDS_RO (PyBUF_STRIDES | PyBUF_FORMAT)
#endif
#ifndef PyBUF_RECORDS
#define PyBUF_RECORDS (PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE)
#endif
#ifndef PyBUF_FULL_RO
#define PyBUF_FULL_RO (PyBUF_INDIRECT | PyBUF_FORMAT)
#endif
#ifndef PyBUF_FULL
#define PyBUF_FULL (PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE)
#endif
#ifndef _MOLT_PYBUF_C_CONTIGUOUS_BIT
#define _MOLT_PYBUF_C_CONTIGUOUS_BIT 0x0020
#endif
#ifndef _MOLT_PYBUF_F_CONTIGUOUS_BIT
#define _MOLT_PYBUF_F_CONTIGUOUS_BIT 0x0040
#endif
#ifndef _MOLT_PYBUF_ANY_CONTIGUOUS_BIT
#define _MOLT_PYBUF_ANY_CONTIGUOUS_BIT 0x0080
#endif

/* GC traversal types */
typedef int (*visitproc)(PyObject *, void *);
typedef int (*traverseproc)(PyObject *, visitproc, void *);
typedef int (*inquiry)(PyObject *);

/* ========================================================================
 * Priority 1: Functions that block real C extensions (ujson, markupsafe, orjson)
 * ======================================================================== */

/*
 * tp_name access — Extensions use Py_TYPE(obj)->tp_name for error messages.
 * Since Molt's PyTypeObject is opaque (aliased to PyObject), we provide a
 * helper macro that fetches the name string from the runtime.
 *
 * Usage:  const char *name = Py_TYPE_NAME(obj);
 *         // name is valid until the returned object is decref'd
 *
 * For code that does Py_TYPE(obj)->tp_name, use the _Py_TYPE_NAME_CSTR
 * thread-local cache to avoid lifetime issues.
 */
static inline const char *_molt_type_name_cstr(PyObject *obj) {
    PyObject *type_obj;
    PyObject *name_obj;
    const char *name;
    if (obj == NULL) {
        return "<NULL>";
    }
    type_obj = (PyObject *)Py_TYPE(obj);
    if (type_obj == NULL) {
        return "<unknown>";
    }
    name_obj = PyObject_GetAttrString(type_obj, "__name__");
    if (name_obj == NULL) {
        /* Clear the error — callers use this for diagnostics, not control flow */
        PyErr_Clear();
        return "<unknown>";
    }
    name = PyUnicode_AsUTF8(name_obj);
    Py_DECREF(name_obj);
    if (name == NULL) {
        PyErr_Clear();
        return "<unknown>";
    }
    return name;
}

/* Macro that mirrors the common Py_TYPE(obj)->tp_name pattern */
#define Py_TYPE_NAME(obj) _molt_type_name_cstr((PyObject *)(obj))

/*
 * PyObject_CallMethodObjArgs — variadic method call with PyObject* arguments.
 * CPython signature: PyObject *PyObject_CallMethodObjArgs(PyObject *obj,
 *                        PyObject *name, ..., NULL)
 */
/* ========================================================================
 * Priority 2: Common patterns used by many extensions
 * ======================================================================== */

/* PyNumber_Index — defined earlier in this file. */

/*
 * PyObject_RichCompare — rich comparison with op argument.
 * op is one of Py_LT, Py_LE, Py_EQ, Py_NE, Py_GT, Py_GE.
 */
#ifndef Py_LT
#define Py_LT 0
#define Py_LE 1
#define Py_EQ 2
#define Py_NE 3
#define Py_GT 4
#define Py_GE 5
#endif

/* Heap-type factories and observers use the shared linked ABI declarations. */

/* PyObject_CallFunctionObjArgs — defined earlier in this file. */

/* PyErr_SetFromErrno, PyErr_SetFromErrnoWithFilenameObject,
 * PyErr_SetFromErrnoWithFilenameObjects — defined earlier in this file. */

/* PyErr_WriteUnraisable, PyIter_Next, PyObject_GetIter,
 * PyObject_RichCompareBool — declared by the shared linked ABI. */

/* =========================================================================
 * CPython 3.12 Stable ABI — Gap Fill
 * ~90 additional definitions to reach 100% coverage.
 * ========================================================================= */

/* ---- Memory macros (CPython-compatible) ---- */

#define PyMem_New(type, n) ((type *)PyMem_Malloc((n) * sizeof(type)))
#define PyMem_NEW(type, n) PyMem_New(type, n)
#define PyMem_Resize(p, type, n) \
    ((type *)PyMem_Realloc((p), (n) * sizeof(type)))
#define PyMem_RESIZE(p, type, n) PyMem_Resize(p, type, n)
#define PyMem_Del PyMem_Free
#define PyMem_DEL PyMem_Free

/* ---- PyLong: FromDouble, FromUnsignedLong ---- */

/* ---- Error helpers ---- */

static inline PyObject *PyErr_SetFromErrnoWithFilename(PyObject *exc, const char *filename) {
    char msg_buf[256];
    const char *msg = _molt_strerror(errno, msg_buf, sizeof(msg_buf));
    if (filename != NULL) {
        PyErr_Format(exc ? exc : PyExc_OSError, "[Errno %d] %s: '%s'", errno, msg, filename);
    } else {
        PyErr_SetString(exc ? exc : PyExc_OSError, msg);
    }
    return NULL;
}

/* ---- Capsule: Set/Get helpers ---- */

static inline int PyCapsule_SetPointer(PyObject *capsule, void *pointer) {
    PyObject *ptr_value;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_SetPointer called with NULL capsule");
        return -1;
    }
    if (pointer == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_SetPointer called with NULL pointer");
        return -1;
    }
    ptr_value = PyLong_FromLongLong((long long)(uintptr_t)pointer);
    if (ptr_value == NULL) return -1;
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_PTR_KEY, ptr_value) < 0) {
        Py_DECREF(ptr_value);
        return -1;
    }
    Py_DECREF(ptr_value);
    return 0;
}

static inline int PyCapsule_SetName(PyObject *capsule, const char *name) {
    PyObject *name_value;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_SetName called with NULL capsule");
        return -1;
    }
    if (name != NULL) {
        name_value = PyUnicode_FromString(name);
    } else {
        name_value = Py_None;
        Py_INCREF(name_value);
    }
    if (name_value == NULL) return -1;
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_NAME_KEY, name_value) < 0) {
        Py_DECREF(name_value);
        return -1;
    }
    Py_DECREF(name_value);
    return 0;
}

static inline PyCapsule_Destructor PyCapsule_GetDestructor(PyObject *capsule) {
    PyObject *dtor_obj;
    long long raw;
    if (capsule == NULL) return NULL;
    dtor_obj = PyDict_GetItemString(capsule, _MOLT_CAPSULE_DESTRUCTOR_KEY);
    if (dtor_obj == NULL) return NULL;
    raw = PyLong_AsLongLong(dtor_obj);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return NULL;
    }
    return (PyCapsule_Destructor)(uintptr_t)raw;
}

static inline int PyCapsule_SetDestructor(PyObject *capsule, PyCapsule_Destructor destructor) {
    PyObject *dtor_value;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_SetDestructor called with NULL capsule");
        return -1;
    }
    if (destructor != NULL) {
        dtor_value = PyLong_FromLongLong((long long)(uintptr_t)destructor);
        if (dtor_value == NULL) return -1;
    } else {
        dtor_value = Py_None;
        Py_INCREF(dtor_value);
    }
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_DESTRUCTOR_KEY, dtor_value) < 0) {
        Py_DECREF(dtor_value);
        return -1;
    }
    Py_DECREF(dtor_value);
    return 0;
}

#define _MOLT_CAPSULE_CONTEXT_KEY "__molt_capsule_context__"

static inline void *PyCapsule_GetContext(PyObject *capsule) {
    PyObject *ctx_obj;
    long long raw;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_GetContext called with NULL capsule");
        return NULL;
    }
    ctx_obj = PyDict_GetItemString(capsule, _MOLT_CAPSULE_CONTEXT_KEY);
    if (ctx_obj == NULL) return NULL;
    raw = PyLong_AsLongLong(ctx_obj);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return NULL;
    }
    return (void *)(uintptr_t)raw;
}

static inline int PyCapsule_SetContext(PyObject *capsule, void *context) {
    PyObject *ctx_value;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_SetContext called with NULL capsule");
        return -1;
    }
    ctx_value = PyLong_FromLongLong((long long)(uintptr_t)context);
    if (ctx_value == NULL) return -1;
    if (PyDict_SetItemString(capsule, _MOLT_CAPSULE_CONTEXT_KEY, ctx_value) < 0) {
        Py_DECREF(ctx_value);
        return -1;
    }
    Py_DECREF(ctx_value);
    return 0;
}

/* ---- Unicode: Decode, Append, Translate, RSplit, IsIdentifier, Tailmatch ---- */

extern PyObject *PyUnicode_Decode(const char *s, Py_ssize_t size,
                                          const char *encoding, const char *errors);

static inline void PyUnicode_Append(PyObject **p_left, PyObject *right) {
    PyObject *result;
    if (p_left == NULL) return;
    if (*p_left == NULL || right == NULL) {
        Py_XDECREF(*p_left);
        *p_left = NULL;
        return;
    }
    result = PyUnicode_Concat(*p_left, right);
    Py_DECREF(*p_left);
    *p_left = result;
}

static inline void PyUnicode_AppendAndDel(PyObject **p_left, PyObject *right) {
    PyUnicode_Append(p_left, right);
    Py_XDECREF(right);
}

static inline PyObject *PyUnicode_Translate(PyObject *str, PyObject *table,
                                             const char *errors) {
    PyObject *translate_fn;
    PyObject *result;
    (void)errors;
    if (str == NULL || table == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_Translate");
        return NULL;
    }
    translate_fn = PyObject_GetAttrString(str, "translate");
    if (translate_fn == NULL) return NULL;
    result = PyObject_CallOneArg(translate_fn, table);
    Py_DECREF(translate_fn);
    return result;
}

static inline PyObject *PyUnicode_RSplit(PyObject *s, PyObject *sep,
                                          Py_ssize_t maxsplit) {
    PyObject *rsplit_fn;
    PyObject *args;
    PyObject *result;
    if (s == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL string in PyUnicode_RSplit");
        return NULL;
    }
    rsplit_fn = PyObject_GetAttrString(s, "rsplit");
    if (rsplit_fn == NULL) return NULL;
    args = PyTuple_New(2);
    if (args == NULL) { Py_DECREF(rsplit_fn); return NULL; }
    if (sep != NULL) {
        Py_INCREF(sep);
        PyTuple_SetItem(args, 0, sep);
    } else {
        Py_INCREF(Py_None);
        PyTuple_SetItem(args, 0, Py_None);
    }
    PyTuple_SetItem(args, 1, PyLong_FromSsize_t(maxsplit));
    result = PyObject_CallObject(rsplit_fn, args);
    Py_DECREF(args);
    Py_DECREF(rsplit_fn);
    return result;
}

static inline int PyUnicode_IsIdentifier(PyObject *s) {
    PyObject *method;
    PyObject *result;
    int ret;
    if (s == NULL) return 0;
    method = PyObject_GetAttrString(s, "isidentifier");
    if (method == NULL) { PyErr_Clear(); return 0; }
    result = PyObject_CallNoArgs(method);
    Py_DECREF(method);
    if (result == NULL) { PyErr_Clear(); return 0; }
    ret = PyObject_IsTrue(result);
    Py_DECREF(result);
    return ret;
}

extern Py_ssize_t PyUnicode_Tailmatch(PyObject *str, PyObject *substr,
                                               Py_ssize_t start, Py_ssize_t end,
                                               int direction);

static inline Py_ssize_t PyUnicode_AsWideChar(PyObject *unicode, wchar_t *w, Py_ssize_t size) {
    const char *utf8;
    Py_ssize_t utf8_len;
    Py_ssize_t i, count;
    if (unicode == NULL) return -1;
    utf8 = PyUnicode_AsUTF8AndSize(unicode, &utf8_len);
    if (utf8 == NULL) return -1;
    /* Simple: for BMP-only content, each byte sequence maps 1:1 for ASCII. */
    count = 0;
    for (i = 0; i < utf8_len && count < size; ) {
        unsigned char c = (unsigned char)utf8[i];
        if (c < 0x80) {
            if (w) w[count] = (wchar_t)c;
            count++; i++;
        } else if ((c & 0xE0) == 0xC0 && i + 1 < utf8_len) {
            if (w) w[count] = (wchar_t)(((c & 0x1F) << 6) | (utf8[i+1] & 0x3F));
            count++; i += 2;
        } else if ((c & 0xF0) == 0xE0 && i + 2 < utf8_len) {
            if (w) w[count] = (wchar_t)(((c & 0x0F) << 12) | ((utf8[i+1] & 0x3F) << 6) | (utf8[i+2] & 0x3F));
            count++; i += 3;
        } else if ((c & 0xF8) == 0xF0 && i + 3 < utf8_len) {
            /* Supplementary character — encode as single wchar_t if sizeof(wchar_t) >= 4 */
            uint32_t cp = ((c & 0x07) << 18) | ((utf8[i+1] & 0x3F) << 12) |
                          ((utf8[i+2] & 0x3F) << 6) | (utf8[i+3] & 0x3F);
            if (sizeof(wchar_t) >= 4) {
                if (w) w[count] = (wchar_t)cp;
                count++; i += 4;
            } else {
                /* UTF-16 surrogate pair */
                if (count + 1 < size) {
                    if (w) {
                        cp -= 0x10000;
                        w[count]     = (wchar_t)(0xD800 | (cp >> 10));
                        w[count + 1] = (wchar_t)(0xDC00 | (cp & 0x3FF));
                    }
                    count += 2; i += 4;
                } else {
                    break;
                }
            }
        } else {
            /* Invalid byte — skip */
            i++;
        }
    }
    return count;
}

static inline wchar_t *PyUnicode_AsWideCharString(PyObject *unicode, Py_ssize_t *size) {
    Py_ssize_t len;
    wchar_t *buf;
    if (unicode == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_AsWideCharString");
        return NULL;
    }
    len = PyUnicode_GetLength(unicode);
    if (len < 0) return NULL;
    /* Allocate generous buffer: worst case each char becomes a surrogate pair */
    buf = (wchar_t *)PyMem_Malloc((size_t)(len + 1) * sizeof(wchar_t));
    if (buf == NULL) return NULL;
    len = PyUnicode_AsWideChar(unicode, buf, len + 1);
    if (len < 0) {
        PyMem_Free(buf);
        return NULL;
    }
    buf[len] = L'\0';
    if (size != NULL) *size = len;
    return buf;
}

static inline PyObject *PyUnicode_FromWideChar(const wchar_t *w, Py_ssize_t size) {
    /* Convert wchar_t string to UTF-8, then to PyObject */
    char *buf;
    Py_ssize_t i, pos;
    PyObject *result;
    if (w == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_FromWideChar");
        return NULL;
    }
    if (size < 0) {
        size = 0;
        while (w[size] != L'\0') size++;
    }
    /* Worst case: 4 bytes per wchar_t */
    buf = (char *)PyMem_Malloc((size_t)(size * 4 + 1));
    if (buf == NULL) return NULL;
    pos = 0;
    for (i = 0; i < size; i++) {
        uint32_t cp = (uint32_t)w[i];
        /* Handle UTF-16 surrogates on narrow wchar_t platforms */
        if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < size) {
            uint32_t lo = (uint32_t)w[i + 1];
            if (lo >= 0xDC00 && lo <= 0xDFFF) {
                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                i++;
            }
        }
        if (cp < 0x80) {
            buf[pos++] = (char)cp;
        } else if (cp < 0x800) {
            buf[pos++] = (char)(0xC0 | (cp >> 6));
            buf[pos++] = (char)(0x80 | (cp & 0x3F));
        } else if (cp < 0x10000) {
            buf[pos++] = (char)(0xE0 | (cp >> 12));
            buf[pos++] = (char)(0x80 | ((cp >> 6) & 0x3F));
            buf[pos++] = (char)(0x80 | (cp & 0x3F));
        } else {
            buf[pos++] = (char)(0xF0 | (cp >> 18));
            buf[pos++] = (char)(0x80 | ((cp >> 12) & 0x3F));
            buf[pos++] = (char)(0x80 | ((cp >> 6) & 0x3F));
            buf[pos++] = (char)(0x80 | (cp & 0x3F));
        }
    }
    buf[pos] = '\0';
    result = PyUnicode_FromStringAndSize(buf, pos);
    PyMem_Free(buf);
    return result;
}

static inline PyObject *PyUnicode_DecodeLocale(const char *str, const char *errors) {
    if (str == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL string in PyUnicode_DecodeLocale");
        return NULL;
    }
    return PyUnicode_DecodeUTF8(str, (Py_ssize_t)strlen(str), errors);
}

static inline PyObject *PyUnicode_DecodeLocaleAndSize(const char *str, Py_ssize_t len,
                                                       const char *errors) {
    if (str == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL string in PyUnicode_DecodeLocaleAndSize");
        return NULL;
    }
    return PyUnicode_DecodeUTF8(str, len, errors);
}

static inline PyObject *PyUnicode_EncodeLocale(PyObject *unicode, const char *errors) {
    (void)errors;
    return PyUnicode_AsEncodedString(unicode, "utf-8", "surrogateescape");
}

extern PyObject *PyUnicode_FromKindAndData(int kind, const void *buffer, Py_ssize_t size);

static inline PyObject *PyUnicode_AsUnicodeEscapeString(PyObject *unicode) {
    PyObject *method;
    PyObject *args;
    PyObject *result;
    if (unicode == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_AsUnicodeEscapeString");
        return NULL;
    }
    method = PyObject_GetAttrString(unicode, "encode");
    if (method == NULL) return NULL;
    args = PyTuple_Pack(1, PyUnicode_FromString("unicode_escape"));
    if (args == NULL) { Py_DECREF(method); return NULL; }
    result = PyObject_CallObject(method, args);
    Py_DECREF(args);
    Py_DECREF(method);
    return result;
}

static inline PyObject *PyUnicode_AsRawUnicodeEscapeString(PyObject *unicode) {
    PyObject *method;
    PyObject *args;
    PyObject *result;
    if (unicode == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_AsRawUnicodeEscapeString");
        return NULL;
    }
    method = PyObject_GetAttrString(unicode, "encode");
    if (method == NULL) return NULL;
    args = PyTuple_Pack(1, PyUnicode_FromString("raw_unicode_escape"));
    if (args == NULL) { Py_DECREF(method); return NULL; }
    result = PyObject_CallObject(method, args);
    Py_DECREF(args);
    Py_DECREF(method);
    return result;
}

static inline PyObject *PyUnicode_DecodeUnicodeEscape(const char *s, Py_ssize_t size,
                                                       const char *errors) {
    return PyUnicode_Decode(s, size, "unicode_escape", errors);
}

static inline PyObject *PyUnicode_DecodeRawUnicodeEscape(const char *s, Py_ssize_t size,
                                                          const char *errors) {
    return PyUnicode_Decode(s, size, "raw_unicode_escape", errors);
}

/* ---- Tuple / List slice helpers ---- */

/* ---- Thread-specific Storage (TSS) API — CPython 3.7+ stable ABI ---- */

typedef struct {
    int _is_initialized;
    void *_key;
} Py_tss_t;

#define Py_tss_NEEDS_INIT {0, NULL}

static inline Py_tss_t *PyThread_tss_alloc(void) {
    Py_tss_t *key = (Py_tss_t *)PyMem_Malloc(sizeof(Py_tss_t));
    if (key != NULL) {
        key->_is_initialized = 0;
        key->_key = NULL;
    }
    return key;
}

static inline void PyThread_tss_free(Py_tss_t *key) {
    if (key != NULL) {
        PyMem_Free(key);
    }
}

static inline int PyThread_tss_is_created(Py_tss_t *key) {
    if (key == NULL) return 0;
    return key->_is_initialized;
}

static inline int PyThread_tss_create(Py_tss_t *key) {
    if (key == NULL) return -1;
    key->_is_initialized = 1;
    key->_key = NULL;
    return 0;
}

static inline void PyThread_tss_delete(Py_tss_t *key) {
    if (key != NULL) {
        key->_is_initialized = 0;
        key->_key = NULL;
    }
}

static inline int PyThread_tss_set(Py_tss_t *key, void *value) {
    if (key == NULL || !key->_is_initialized) return -1;
    key->_key = value;
    return 0;
}

static inline void *PyThread_tss_get(Py_tss_t *key) {
    if (key == NULL || !key->_is_initialized) return NULL;
    return key->_key;
}

/* ---- PyIndex_Check ---- */

static inline int PyIndex_Check(PyObject *obj) {
    if (obj == NULL) return 0;
    /* An object supports the index protocol if it has __index__ */
    {
        PyObject *method = PyObject_GetAttrString(obj, "__index__");
        if (method != NULL) {
            Py_DECREF(method);
            return 1;
        }
        PyErr_Clear();
        return 0;
    }
}

/* ---- Py_FatalError ---- */

static inline void Py_FatalError(const char *message) {
    fprintf(stderr, "Fatal Python error: %s\n", message ? message : "(null)");
    abort();
}

/* ---- PyType_GenericAlloc / PyType_GenericNew ---- */

static inline PyObject *_molt_type_generic_alloc(PyTypeObject *type, Py_ssize_t nitems) {
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyObject *(*)(PyTypeObject *, Py_ssize_t))_molt_host_abi_symbol("PyType_GenericAlloc"))(type, nitems);
#else
    return PyType_GenericAlloc(type, nitems);
#endif
}

static inline PyObject *_molt_type_generic_new(PyTypeObject *type, PyObject *args, PyObject *kwds) {
    _molt_require_canonical_allocation_type(type);
#ifdef MOLT_EXTENSION_HOST_ABI
    return ((PyObject *(*)(PyTypeObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyType_GenericNew"))(type, args, kwds);
#else
    return PyType_GenericNew(type, args, kwds);
#endif
}

#define PyType_GenericAlloc _molt_type_generic_alloc
#define PyType_GenericNew _molt_type_generic_new

/* ---- Py_Exit ---- */

static inline void Py_Exit(int status) {
    exit(status);
}

/* ---- PyOS_vsnprintf ---- */

static inline int PyOS_vsnprintf(char *str, size_t size, const char *format, va_list va) {
    return vsnprintf(str, size, format, va);
}

/* PyDict_GetItemWithError — already defined earlier in this file. */

/* PyDict_GetItemRef, PyDict_GetItemStringRef — defined earlier in this file. */

/* ---- PyDict_Pop (3.12+) ---- */

/* Py_NewRef, Py_XNewRef — already defined earlier in this file. */

/* ---- PyDictProxy_New ---- */

/* ---- PyObject_ClearWeakRefs ---- */

static inline void PyObject_ClearWeakRefs(PyObject *obj) {
    /* In CPython this clears all weak references pointing to *obj* and is
       called from tp_dealloc.  Molt's GC handles weak-reference invalidation
       internally, so this is a no-op at the C-API boundary. */
    (void)obj;
}

/* ---- PyFloat_GetInfo ---- */

/* ---- PyLong_AsUnsignedLongLongMask already exists but ensure PyLong_AsUnsignedLongMask ---- */

/* PyOS_stricmp, PyOS_strnicmp — already defined earlier in this file. */

/* ---- Py_AddPendingCall / Py_MakePendingCalls ---- */

int Py_AddPendingCall(int (*func)(void *), void *arg);
int Py_MakePendingCalls(void);

/* ---- PyObject_GC_IsTracked / PyObject_GC_IsFinalized already exist;
        ensure _PyObject_GC_TRACK / _PyObject_GC_UNTRACK macros ---- */

#ifndef _PyObject_GC_TRACK
#define _PyObject_GC_TRACK(op) PyObject_GC_Track(op)
#endif
#ifndef _PyObject_GC_UNTRACK
#define _PyObject_GC_UNTRACK(op) PyObject_GC_UnTrack(op)
#endif

/* ---- PyUnicode_New (used by some extensions to allocate mutable buffers) ---- */

extern PyObject *PyUnicode_New(Py_ssize_t size, Py_UCS4 maxchar);

/* ---- PyUnicode_AsUCS4 / PyUnicode_AsUCS4Copy ---- */

extern Py_UCS4 *PyUnicode_AsUCS4(PyObject *unicode, Py_UCS4 *target,
                                          Py_ssize_t targetsize, int copy_null);

extern Py_UCS4 *PyUnicode_AsUCS4Copy(PyObject *unicode);

/* Py_SetProgramName, Py_GetProgramName, Py_GetProgramFullPath,
   Py_GetPrefix, Py_GetExecPrefix, Py_GetPath — already defined earlier. */

/* ---- Py_GetRecursionLimit / Py_SetRecursionLimit ---- */

static inline int Py_GetRecursionLimit(void) {
    return 1000;
}

static inline void Py_SetRecursionLimit(int limit) {
    (void)limit; /* no-op — Molt uses its own stack management */
}

/* ---- PyOS_InterruptOccurred ---- */

int PyOS_InterruptOccurred(void);

/* ---- PyUnicode_Splitlines already exists, ensure PyUnicode_DecodeCharmap ---- */

static inline PyObject *PyUnicode_DecodeCharmap(const char *data, Py_ssize_t size,
                                                  PyObject *mapping, const char *errors) {
    (void)mapping; (void)errors;
    /* Fallback: decode as latin-1 */
    return PyUnicode_DecodeLatin1(data, size, errors);
}

/* PyType_FromModuleAndSpec is declared by the shared type-object surface. */

/* ---- PyModule_AddIntMacro / PyModule_AddStringMacro ---- */

#ifndef PyModule_AddIntMacro
#define PyModule_AddIntMacro(module, macro) \
    PyModule_AddIntConstant(module, #macro, (long)(macro))
#endif

#ifndef PyModule_AddStringMacro
#define PyModule_AddStringMacro(module, macro) \
    PyModule_AddStringConstant(module, #macro, macro)
#endif

/* Py_EnterRecursiveCall, Py_LeaveRecursiveCall — already defined earlier. */

/* ---- Py_UNREACHABLE ---- */

#ifndef Py_UNREACHABLE
#define Py_UNREACHABLE() abort()
#endif

/* ---- Py_UNUSED ---- */

#ifndef Py_UNUSED
#define Py_UNUSED(name) _unused_ ## name __attribute__((unused))
#endif

/* ---- PyLong_AsInt (3.12+) ---- */

/* PyObject_CallFunction, PyObject_CallMethod, PyObject_HasAttr,
   PyObject_HasAttrString — already defined earlier in this file. */

/* ---- Py_GETENV ---- */

#ifndef Py_GETENV
#define Py_GETENV(s) getenv(s)
#endif

/* ---- Py_ssize_t max/min ---- */

#ifndef PY_SSIZE_T_MAX
#define PY_SSIZE_T_MAX ((Py_ssize_t)(((size_t)-1) >> 1))
#endif

#ifndef PY_SSIZE_T_MIN
#define PY_SSIZE_T_MIN (-PY_SSIZE_T_MAX - 1)
#endif

/* PyMemAllocatorDomain, PyMemAllocatorEx, PyMem_SetAllocator,
   PyMem_GetAllocator — already defined earlier in this file. */

/* ---- Py_SetPath ---- */

static inline void Py_SetPath(const wchar_t *path) {
    (void)path;
}

/* PyEval_SaveThread, PyEval_RestoreThread, PyEval_GetFrame,
   PyEval_GetBuiltins, PyEval_GetGlobals, PyEval_GetLocals,
   PyFrameObject — already defined earlier in this file. */

/* PyFrame_GetBack, PyFrame_GetCode, PyFrame_GetLineNumber,
   PyFrame_GetLocals, PyFrame_GetGlobals, PyFrame_GetBuiltins,
   PyFrame_GetLasti — already defined earlier in this file. */

/* Descriptor exports are declared in shared/_descriptor_exports.h. */

/* ---- PySlice_GetIndices ---- */

/* ---- Py_MATH_PI / Py_MATH_E / Py_MATH_TAU / Py_MATH_INF / Py_MATH_NAN ---- */

#ifndef Py_MATH_PI
#define Py_MATH_PI 3.14159265358979323846
#endif
#ifndef Py_MATH_E
#define Py_MATH_E 2.71828182845904523536
#endif
#ifndef Py_MATH_TAU
#define Py_MATH_TAU 6.28318530717958647692
#endif
#ifndef Py_MATH_INF
#define Py_MATH_INF HUGE_VAL
#endif
#ifndef Py_MATH_NAN
#define Py_MATH_NAN ((double)NAN)
#endif

/* ---- Py_STRINGIFY ---- */

#ifndef Py_STRINGIFY
#define _Py_STRINGIFY(x) #x
#define Py_STRINGIFY(x) _Py_STRINGIFY(x)
#endif

/* Py_ABS, Py_MIN, Py_MAX, Py_MEMBER_SIZE, Py_ARRAY_LENGTH
   — already defined earlier in this file. */

/* PyUnicode_1BYTE_KIND, PyUnicode_2BYTE_KIND, PyUnicode_4BYTE_KIND
   — already defined earlier in this file. */

#ifndef PyUnicode_KIND
static inline unsigned int PyUnicode_KIND(PyObject *op) {
    return molt_capi_unicode_kind(op);
}
#define PyUnicode_KIND(op) PyUnicode_KIND((PyObject *)(op))
#endif

#ifndef PyUnicode_DATA
static inline void *PyUnicode_DATA(PyObject *op) {
    return molt_capi_unicode_data(op);
}
#define PyUnicode_DATA(op) PyUnicode_DATA((PyObject *)(op))
#endif

/* Py_CLEAR, Py_SETREF, Py_XSETREF — already defined earlier in this file. */

/* ---- Py_IS_TYPE ---- */

#ifndef Py_IS_TYPE
#define Py_IS_TYPE(ob, type) (Py_TYPE(ob) == (type))
#endif

/* ---- Missing utility macros for C extension compatibility ---- */

#ifndef PyObject_INIT
#define PyObject_INIT(op, typeobj) PyObject_Init((PyObject *)(op), (PyTypeObject *)(typeobj))
#endif

#ifndef PyObject_INIT_VAR
#define PyObject_INIT_VAR(op, typeobj, size) PyObject_InitVar((PyVarObject *)(op), (PyTypeObject *)(typeobj), (Py_ssize_t)(size))
#endif

#ifndef _PyObject_EXTRA_INIT
#define _PyObject_EXTRA_INIT
#endif

#ifndef _Py_IsFinalizing
static inline int _Py_IsFinalizing(void) {
    return 0;
}
#endif

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MOLT_C_API_PYTHON_H */
