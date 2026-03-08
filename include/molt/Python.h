#ifndef MOLT_C_API_PYTHON_H
#define MOLT_C_API_PYTHON_H

/* Source-compat facade only; stable ABI lives in include/molt/molt.h. */

#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <math.h>
#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <wctype.h>

#include <molt/molt.h>

#if defined(_MULTIARRAYMODULE) || defined(_UMATHMODULE)
#ifndef PYTHONCAPI_COMPAT
#define PYTHONCAPI_COMPAT 1
#endif
#endif

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Symbols exported by the runtime exception subsystem.
 * These are used to synthesize PyExc_* shims in this compatibility header.
 */
MoltHandle molt_exception_kind(MoltHandle exc_bits);
MoltHandle molt_exception_class(MoltHandle kind_bits);

typedef intptr_t Py_ssize_t;
typedef Py_ssize_t Py_hash_t;
typedef size_t Py_uhash_t;
typedef struct _frame PyFrameObject;
typedef struct _molt_pyinterpreterstate PyInterpreterState;
struct PyNumberMethods;
struct PyMappingMethods;
struct PySequenceMethods;
struct PyAsyncMethods;
struct PyBufferProcs;
typedef struct _molt_pyobject *(*vectorcallfunc)(
    struct _molt_pyobject *,
    struct _molt_pyobject *const *,
    size_t,
    struct _molt_pyobject *);
typedef struct _molt_pyobject {
    Py_ssize_t ob_refcnt;
    struct _molt_pyobject *ob_type;
    const char *tp_name;
    Py_ssize_t tp_basicsize;
    Py_ssize_t tp_itemsize;
    void (*tp_dealloc)(struct _molt_pyobject *);
    Py_ssize_t tp_vectorcall_offset;
    struct _molt_pyobject *(*tp_getattr)(struct _molt_pyobject *, char *);
    int (*tp_setattr)(struct _molt_pyobject *, char *, struct _molt_pyobject *);
    struct PyAsyncMethods *tp_as_async;
    struct _molt_pyobject *(*tp_repr)(struct _molt_pyobject *);
    struct PyNumberMethods *tp_as_number;
    struct PySequenceMethods *tp_as_sequence;
    struct PyMappingMethods *tp_as_mapping;
    Py_hash_t (*tp_hash)(struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_call)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_str)(struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_getattro)(
        struct _molt_pyobject *, struct _molt_pyobject *);
    int (*tp_setattro)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    struct PyBufferProcs *tp_as_buffer;
    unsigned long tp_flags;
    const char *tp_doc;
    int (*tp_traverse)(
        struct _molt_pyobject *, int (*)(struct _molt_pyobject *, void *), void *);
    int (*tp_clear)(struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_richcompare)(
        struct _molt_pyobject *, struct _molt_pyobject *, int);
    Py_ssize_t tp_weaklistoffset;
    struct _molt_pyobject *(*tp_iter)(struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_iternext)(struct _molt_pyobject *);
    struct PyMethodDef *tp_methods;
    struct PyMemberDef *tp_members;
    struct PyGetSetDef *tp_getset;
    struct _molt_pyobject *tp_base;
    struct _molt_pyobject *tp_dict;
    struct _molt_pyobject *(*tp_descr_get)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    int (*tp_descr_set)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    Py_ssize_t tp_dictoffset;
    int (*tp_init)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    struct _molt_pyobject *(*tp_alloc)(struct _molt_pyobject *, Py_ssize_t);
    struct _molt_pyobject *(*tp_new)(
        struct _molt_pyobject *, struct _molt_pyobject *, struct _molt_pyobject *);
    void (*tp_free)(void *);
    int (*tp_is_gc)(struct _molt_pyobject *);
    struct _molt_pyobject *tp_bases;
    struct _molt_pyobject *tp_mro;
    struct _molt_pyobject *tp_cache;
    void *tp_subclasses;
    struct _molt_pyobject *tp_weaklist;
    void (*tp_del)(struct _molt_pyobject *);
    unsigned int tp_version_tag;
    void (*tp_finalize)(struct _molt_pyobject *);
    vectorcallfunc tp_vectorcall;
} PyObject;
typedef PyObject PyTypeObject;
typedef PyObject PyCodeObject;
typedef int PyGILState_STATE;
typedef uint8_t Py_UCS1;
typedef uint16_t Py_UCS2;
typedef uint32_t Py_UCS4;
typedef intptr_t Py_intptr_t;
typedef uintptr_t Py_uintptr_t;
typedef struct {
    void *buf;
    PyObject *obj;
    Py_ssize_t len;
    Py_ssize_t itemsize;
    int readonly;
    int ndim;
    char *format;
    Py_ssize_t *shape;
    Py_ssize_t *strides;
    Py_ssize_t *suboffsets;
    void *internal;
} Py_buffer;
typedef PyObject PyBytesObject;
typedef PyObject PyUnicodeObject;
typedef PyObject PyDictObject;
typedef PyObject PyLongObject;
typedef PyObject PyCFunctionObject;
typedef PyObject PyGetSetDescrObject;
typedef PyObject PyMemberDescrObject;
typedef PyObject PyMethodDescrObject;
typedef PyObject PyIntUScalarObject;

typedef struct {
    Py_ssize_t length;
    wchar_t **items;
} PyWideStringList;

typedef enum {
    PYCONFIG_MEMBER_UNKNOWN = 0
} PyConfigMemberType;

typedef struct {
    int _molt_reserved;
    PyWideStringList argv;
} PyConfig;

typedef struct {
    uintptr_t _molt_reserved;
} PyGC_Head;

typedef struct {
    int _molt_reserved;
} PyCriticalSection;

typedef struct _molt_pythreadstate {
    PyInterpreterState *interp;
    PyFrameObject *frame;
    uint64_t id;
    int tracing;
    int use_tracing;
    void *c_tracefunc;
    void *c_profilefunc;
    void *cframe;
    int _molt_reserved;
} PyThreadState;

struct _molt_pyinterpreterstate {
    int _molt_reserved;
};

typedef void (*PyCapsule_Destructor)(PyObject *);

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);
typedef int (*getbufferproc)(PyObject *, Py_buffer *, int);
typedef void (*releasebufferproc)(PyObject *, Py_buffer *);
typedef PyObject *(*getter)(PyObject *, void *);
typedef int (*setter)(PyObject *, PyObject *, void *);
typedef Py_ssize_t (*lenfunc)(PyObject *);
typedef int (*visitproc)(PyObject *, void *);
typedef int (*traverseproc)(PyObject *, visitproc, void *);
typedef PyObject *(*unaryfunc)(PyObject *);
typedef PyObject *(*binaryfunc)(PyObject *, PyObject *);
typedef PyObject *(*ternaryfunc)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*ssizeargfunc)(PyObject *, Py_ssize_t);
typedef PyObject *(*ssizessizeargfunc)(PyObject *, Py_ssize_t, Py_ssize_t);
typedef int (*ssizeobjargproc)(PyObject *, Py_ssize_t, PyObject *);
typedef int (*ssizessizeobjargproc)(PyObject *, Py_ssize_t, Py_ssize_t, PyObject *);
typedef int (*objobjargproc)(PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)(PyObject *, PyObject *);
typedef int (*inquiry)(PyObject *);
typedef void (*freefunc)(void *);
typedef void (*destructor)(PyObject *);
typedef PyObject *(*getattrfunc)(PyObject *, char *);
typedef PyObject *(*getattrofunc)(PyObject *, PyObject *);
typedef int (*setattrfunc)(PyObject *, char *, PyObject *);
typedef int (*setattrofunc)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*reprfunc)(PyObject *);
typedef Py_hash_t (*hashfunc)(PyObject *);
typedef PyObject *(*richcmpfunc)(PyObject *, PyObject *, int);
typedef PyObject *(*getiterfunc)(PyObject *);
typedef PyObject *(*iternextfunc)(PyObject *);
typedef PyObject *(*descrgetfunc)(PyObject *, PyObject *, PyObject *);
typedef int (*descrsetfunc)(PyObject *, PyObject *, PyObject *);
typedef int (*initproc)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*newfunc)(PyTypeObject *, PyObject *, PyObject *);

typedef void *PyThread_type_lock;
typedef long long PY_TIMEOUT_T;

typedef enum PyLockStatus {
    PY_LOCK_FAILURE = 0,
    PY_LOCK_ACQUIRED = 1,
    PY_LOCK_INTR
} PyLockStatus;

typedef struct PyMutex {
    PyThread_type_lock _molt_lock;
} PyMutex;

typedef struct PyNumberMethods {
    binaryfunc nb_add;
    binaryfunc nb_subtract;
    binaryfunc nb_multiply;
    binaryfunc nb_remainder;
    binaryfunc nb_divmod;
    ternaryfunc nb_power;
    unaryfunc nb_negative;
    unaryfunc nb_positive;
    unaryfunc nb_absolute;
    inquiry nb_bool;
    unaryfunc nb_invert;
    binaryfunc nb_lshift;
    binaryfunc nb_rshift;
    binaryfunc nb_and;
    binaryfunc nb_xor;
    binaryfunc nb_or;
    unaryfunc nb_int;
    void *nb_reserved;
    unaryfunc nb_float;
    binaryfunc nb_inplace_add;
    binaryfunc nb_inplace_subtract;
    binaryfunc nb_inplace_multiply;
    binaryfunc nb_inplace_remainder;
    ternaryfunc nb_inplace_power;
    binaryfunc nb_inplace_lshift;
    binaryfunc nb_inplace_rshift;
    binaryfunc nb_inplace_and;
    binaryfunc nb_inplace_xor;
    binaryfunc nb_inplace_or;
    binaryfunc nb_floor_divide;
    binaryfunc nb_true_divide;
    binaryfunc nb_inplace_floor_divide;
    binaryfunc nb_inplace_true_divide;
    unaryfunc nb_index;
    binaryfunc nb_matrix_multiply;
    binaryfunc nb_inplace_matrix_multiply;
} PyNumberMethods;

typedef struct PyMappingMethods {
    lenfunc mp_length;
    binaryfunc mp_subscript;
    objobjargproc mp_ass_subscript;
} PyMappingMethods;

typedef struct PySequenceMethods {
    lenfunc sq_length;
    binaryfunc sq_concat;
    ssizeargfunc sq_repeat;
    ssizeargfunc sq_item;
    void *was_sq_slice;
    ssizeobjargproc sq_ass_item;
    void *was_sq_ass_slice;
    objobjproc sq_contains;
    binaryfunc sq_inplace_concat;
    ssizeargfunc sq_inplace_repeat;
} PySequenceMethods;

typedef struct PyAsyncMethods {
    unaryfunc am_await;
    unaryfunc am_aiter;
    unaryfunc am_anext;
    binaryfunc am_send;
} PyAsyncMethods;

typedef struct PyBufferProcs {
    int (*bf_getbuffer)(PyObject *, Py_buffer *, int);
    void (*bf_releasebuffer)(PyObject *, Py_buffer *);
} PyBufferProcs;

typedef struct PyMethodDef {
    const char *ml_name;
    void *ml_meth;
    int ml_flags;
    const char *ml_doc;
} PyMethodDef;

typedef struct PyModuleDef {
    void *m_base;
    const char *m_name;
    const char *m_doc;
    Py_ssize_t m_size;
    PyMethodDef *m_methods;
    void *m_slots;
    void *m_traverse;
    void *m_clear;
    void *m_free;
} PyModuleDef;

typedef struct PyModuleDef_Slot {
    int slot;
    void *value;
} PyModuleDef_Slot;

typedef struct PyType_Slot {
    int slot;
    void *pfunc;
} PyType_Slot;

typedef struct PyType_Spec {
    const char *name;
    int basicsize;
    int itemsize;
    unsigned int flags;
    PyType_Slot *slots;
} PyType_Spec;

typedef struct PyGetSetDef {
    const char *name;
    getter get;
    setter set;
    const char *doc;
    void *closure;
} PyGetSetDef;

typedef struct PyMemberDef {
    const char *name;
    int type;
    Py_ssize_t offset;
    int flags;
    const char *doc;
} PyMemberDef;

#define Py_T_SHORT 0
#define Py_T_INT 1
#define Py_T_LONG 2
#define Py_T_FLOAT 3
#define Py_T_DOUBLE 4
#define Py_T_STRING 5
#define _Py_T_OBJECT 6
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

#define Py_READONLY 1
#define Py_AUDIT_READ 2
#define _Py_WRITE_RESTRICTED 4
#define Py_RELATIVE_OFFSET 8

typedef struct {
    double real;
    double imag;
} Py_complex;

typedef struct {
    PyObject *ob_base;
    Py_complex cval;
} PyComplexObject;

typedef struct {
    PyObject *ob_base;
    double ob_fval;
} PyFloatObject;

typedef struct {
    PyTypeObject ht_type;
} PyHeapTypeObject;

typedef struct {
    PyObject *ob_base;
    Py_ssize_t ob_size;
} PyVarObject;

typedef struct {
    PyObject *ob_base;
    Py_ssize_t ob_size;
    PyObject *ob_item[1];
} PyTupleObject;

static inline const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out);
static inline PyObject *PyType_FromSpec(PyType_Spec *spec);
static inline PyObject *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases);
static inline PyObject *PyType_FromModuleAndSpec(
    PyObject *module, PyType_Spec *spec, PyObject *bases);
static inline PyObject *PyType_GetModule(PyTypeObject *type);
static inline void *PyType_GetModuleState(PyTypeObject *type);
static inline PyObject *PyType_GetModuleByDef(PyTypeObject *type, PyModuleDef *def);
static inline PyModuleDef *PyModule_GetDef(PyObject *module);
static inline PyObject *PyModule_GetDict(PyObject *module);
static inline void *PyModule_GetState(PyObject *module);
static inline int PyModule_AddFunctions(PyObject *module, PyMethodDef *functions);
static inline int PyState_AddModule(PyObject *module, PyModuleDef *def);
static inline int PyModule_SetDocString(PyObject *module, const char *docstring);
static inline PyObject *_molt_builtin_class_lookup_utf8(const char *name);
static inline PyTypeObject *_molt_builtin_type_object_borrowed(const char *name);
static inline void PyErr_Clear(void);
static inline void PyErr_Fetch(PyObject **ptype, PyObject **pvalue, PyObject **ptraceback);
static inline int PyErr_ExceptionMatches(PyObject *exc);
static inline PyObject *PyErr_Occurred(void);
static inline void PyErr_SetString(PyObject *exc, const char *message);
static inline void PyErr_SetNone(PyObject *exc);
static inline PyObject *PyErr_NoMemory(void);
static inline void PyErr_BadInternalCall(void);
static inline int PyErr_CheckSignals(void);
static inline PyObject *PyErr_FormatV(PyObject *exc, const char *format, va_list vargs);
static inline PyObject *PyErr_NewException(const char *name, PyObject *base, PyObject *dict);
static inline PyObject *PyErr_SetFromErrno(PyObject *exc);
static inline PyObject *PyErr_SetFromErrnoWithFilenameObject(PyObject *exc, PyObject *filename);
static inline int PyArg_UnpackTuple(
    PyObject *args,
    const char *name,
    Py_ssize_t min,
    Py_ssize_t max,
    ...);
static inline int PyArg_VaParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    va_list vargs);
static inline void *PyMem_Malloc(size_t size);
static inline void PyMem_Free(void *ptr);
static inline PyObject *Py_NewRef(PyObject *obj);
static inline PyObject *PyObject_Str(PyObject *obj);
static inline const char *PyUnicode_AsUTF8(PyObject *value);
static inline PyObject *PyUnicode_AsEncodedString(
    PyObject *unicode,
    const char *encoding,
    const char *errors);
static inline int PyUnicode_Check(PyObject *obj);
static inline int PyContextVar_Get(PyObject *var, PyObject *default_value, PyObject **value);
static inline PyObject *PyContextVar_New(const char *name, PyObject *default_value);
static inline int PyContextVar_Set(PyObject *var, PyObject *value);
static inline PyObject *PyUnicode_FromString(const char *value);
static inline PyObject *PyUnicode_InternFromString(const char *value);
static inline PyObject *PyUnicode_FromKindAndData(
    int kind, const void *buffer, Py_ssize_t size);
static inline PyObject *PyUnicode_FromWideChar(const wchar_t *value, Py_ssize_t size);
static inline Py_UCS4 *PyUnicode_AsUCS4Copy(PyObject *value);
static inline int PyUnicode_CompareWithASCIIString(PyObject *unicode, const char *string);
static inline int PyUnicode_Contains(PyObject *container, PyObject *element);
static inline int PyUnicode_FSConverter(PyObject *obj, void *result);
static inline PyObject *PyUnicode_Replace(
    PyObject *string,
    PyObject *substr,
    PyObject *replacement,
    Py_ssize_t maxcount);
static inline int PyUnicode_Tailmatch(
    PyObject *string,
    PyObject *substring,
    Py_ssize_t start,
    Py_ssize_t end,
    int direction);
static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size);
static inline Py_ssize_t PyBytes_Size(PyObject *value);
static inline int PyBytes_Check(PyObject *obj);
static inline PyObject *PyFloat_FromString(PyObject *obj);
static inline int PyFloat_Check(PyObject *obj);
static inline double PyComplex_RealAsDouble(PyObject *obj);
static inline double PyComplex_ImagAsDouble(PyObject *obj);
static inline PyObject *PyComplex_FromCComplex(Py_complex value);
static inline Py_complex PyComplex_AsCComplex(PyObject *obj);
static inline long PyLong_AsLong(PyObject *obj);
static inline int PyLong_Check(PyObject *obj);
static inline long long PyLong_AsLongLong(PyObject *obj);
static inline long long PyLong_AsLongLongAndOverflow(PyObject *obj, int *overflow);
static inline unsigned long PyLong_AsUnsignedLong(PyObject *obj);
static inline unsigned long long PyLong_AsUnsignedLongLong(PyObject *obj);
static inline unsigned long long PyLong_AsUnsignedLongLongMask(PyObject *obj);
static inline PyObject *PyLong_FromUnsignedLong(unsigned long value);
static inline PyObject *PyLong_FromUnicodeObject(PyObject *obj, int base);
static inline PyObject *PyLong_FromVoidPtr(void *ptr);
static inline void *PyLong_AsVoidPtr(PyObject *obj);
static inline int PyIndex_Check(PyObject *obj);
static inline PyObject *PyNumber_Float(PyObject *obj);
static inline PyObject *PyNumber_Index(PyObject *obj);
static inline PyObject *PyNumber_Lshift(PyObject *a, PyObject *b);
static inline PyObject *PyNumber_Long(PyObject *obj);
static inline PyObject *PyNumber_Negative(PyObject *obj);
static inline PyObject *PyNumber_Or(PyObject *a, PyObject *b);
static inline int PyIter_Check(PyObject *obj);
static inline PyObject *PyIter_Next(PyObject *obj);
static inline double PyOS_string_to_double(
    const char *text, char **endptr, PyObject *overflow_exception);
static inline PyObject *PyImport_ImportModule(const char *name);
static inline PyObject *PyImport_Import(PyObject *name);
static inline PyObject *PySys_GetObject(const char *name);
static inline void *PyCapsule_Import(const char *name, int no_block);
static inline int PyCapsule_SetName(PyObject *capsule, const char *name);
static inline PyObject *PyObject_Dir(PyObject *obj);
static inline PyObject *PyMemoryView_FromObject(PyObject *obj);
static inline PyObject *PyMethod_New(PyObject *func, PyObject *self);
static inline int PyObject_CheckBuffer(PyObject *obj);
static inline void PyObject_ClearWeakRefs(PyObject *obj);
static inline int PyObject_DelAttrString(PyObject *obj, const char *name);
static inline PyObject *PyObject_GenericGetAttr(PyObject *obj, PyObject *name);
static inline PyObject *PyObject_GenericGetDict(PyObject *obj, void *context);
static inline int PyObject_GenericSetAttr(PyObject *obj, PyObject *name, PyObject *value);
static inline PyObject *PyObject_GetAttrString(PyObject *obj, const char *name);
static inline PyObject *PyObject_Format(PyObject *obj, PyObject *format_spec);
static inline PyObject *PyObject_GetIter(PyObject *obj);
static inline void PyObject_CallFinalizer(PyObject *op);
static inline int PyObject_CallFinalizerFromDealloc(PyObject *op);
static inline void PyObject_GC_Track(PyObject *op);
static inline void PyObject_GC_UnTrack(PyObject *op);
static inline int PyDict_Check(PyObject *obj);
static inline int PyObject_SetAttrString(PyObject *obj, const char *name, PyObject *value);
static inline PyObject *PyObject_CallMethod(
    PyObject *obj,
    const char *name,
    const char *format,
    ...);
static inline PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...);
static inline PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...);
static inline PyObject *PyObject_Call(PyObject *callable, PyObject *args, PyObject *kwargs);
static inline PyObject *PyObject_CallObject(PyObject *callable, PyObject *args);
static inline Py_ssize_t PyObject_LengthHint(PyObject *obj, Py_ssize_t defaultvalue);
static inline Py_ssize_t PyObject_Length(PyObject *obj);
static inline int PyObject_Not(PyObject *obj);
static inline PyObject *PyObject_SelfIter(PyObject *obj);
static inline Py_ssize_t PyObject_Size(PyObject *obj);
static inline PyObject *PyObject_Init(PyObject *obj, PyTypeObject *typeobj);
static inline PyObject *PyObject_InitVar(PyVarObject *obj, PyTypeObject *typeobj, Py_ssize_t size);
static inline int PyObject_IsSubclass(PyObject *derived, PyObject *cls);
static inline PyObject *PyObject_Type(PyObject *obj);
static inline PyObject *PyObject_Vectorcall(
    PyObject *callable, PyObject *const *args, size_t nargsf, PyObject *kwnames);
static inline PyObject *PyObject_VectorcallMethod(
    PyObject *name,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwnames);
static inline int PyModule_Check(PyObject *obj);
static inline PyThreadState *PyThreadState_Get(void);
static inline PyObject *PyThreadState_GetDict(void);
static inline PyInterpreterState *PyThreadState_GetInterpreter(PyThreadState *tstate);
static inline PyFrameObject *PyThreadState_GetFrame(PyThreadState *tstate);
static inline PyInterpreterState *PyInterpreterState_Get(void);
static inline PyInterpreterState *PyInterpreterState_Main(void);
static inline int PyTuple_Check(PyObject *obj);
static inline Py_ssize_t PyTuple_Size(PyObject *tuple);
static inline PyObject *PyTuple_GetItem(PyObject *tuple, Py_ssize_t index);
static inline PyObject *PyTuple_New(Py_ssize_t size);
static inline PyObject *PyTuple_Pack(Py_ssize_t n, ...);
static inline int PyTuple_SetItem(PyObject *tuple, Py_ssize_t index, PyObject *value);
static inline PyObject *PyList_AsTuple(PyObject *list);
static inline int PyList_SetSlice(
    PyObject *list,
    Py_ssize_t low,
    Py_ssize_t high,
    PyObject *itemlist);
static inline PyGILState_STATE PyGILState_Ensure(void);
static inline void PyGILState_Release(PyGILState_STATE state);
static inline int PyGILState_Check(void);
static inline int PyGC_Collect(void);
static inline int PyTraceMalloc_Track(unsigned int domain, uintptr_t ptr, size_t size);
static inline int PyTraceMalloc_Untrack(unsigned int domain, uintptr_t ptr);
static inline PyObject *PyDict_New(void);
static inline PyCodeObject *PyFrame_GetCode(PyFrameObject *frame);
static inline PyFrameObject *PyFrame_GetBack(PyFrameObject *frame);
static inline PyObject *PyFrame_GetLocals(PyFrameObject *frame);
static inline PyObject *PyFrame_GetGlobals(PyFrameObject *frame);
static inline PyObject *PyFrame_GetBuiltins(PyFrameObject *frame);
static inline int PyWeakref_Check(PyObject *obj);
static inline PyObject *PyWeakref_GetObject(PyObject *ref);
static inline PyObject *PyWeakref_NewRef(PyObject *obj, PyObject *callback);
static inline int PySlice_AdjustIndices(
    Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t step);
static inline int PySlice_GetIndicesEx(
    PyObject *slice,
    Py_ssize_t length,
    Py_ssize_t *start,
    Py_ssize_t *stop,
    Py_ssize_t *step,
    Py_ssize_t *slicelength);
static inline PyObject *PySlice_New(PyObject *start, PyObject *stop, PyObject *step);
static inline PyObject *PySeqIter_New(PyObject *seq);
static inline PyObject *PySequence_Concat(PyObject *left, PyObject *right);
static inline PyObject *PyType_GenericNew(PyTypeObject *subtype, PyObject *args, PyObject *kwargs);
static inline unsigned long PyType_GetFlags(PyTypeObject *type);
static inline PyObject *PyUnicode_AsLatin1String(PyObject *unicode);
static inline PyObject *PyUnicode_Format(PyObject *format, PyObject *args);
static inline PyObject *PyUnicode_FromFormatV(const char *format, va_list vargs);
static inline Py_ssize_t PyUnicode_GetSize(PyObject *unicode);
static inline PyObject *PyExceptionInstance_Class(PyObject *exc);
static inline PyObject *PyVectorcall_Call(PyObject *callable, PyObject *tuple, PyObject *dict);
static inline int PyUnstable_Object_IsUniqueReferencedTemporary(PyObject *obj);
static inline PyObject *Py_InitModule(const char *name, PyMethodDef *methods);
static inline PyObject *Py_InitModule4(
    const char *name,
    PyMethodDef *methods,
    const char *doc,
    PyObject *self,
    int apiver);
static inline void PyCriticalSection_Begin(PyCriticalSection *critical_section);
static inline void PyCriticalSection_End(PyCriticalSection *critical_section);
static inline int _PyObject_LookupAttr(PyObject *obj, PyObject *name, PyObject **result);
static inline int _Py_IsFinalizing(void);
static inline int _PyLong_AsInt(PyObject *obj);
static inline PyObject **_PyObject_GetDictPtr(PyObject *obj);
static inline PyThreadState *_PyThreadState_UncheckedGet(void);
static inline int _molt_pyunicode_is_ascii(PyObject *unicode);
static inline void *_molt_pyunicode_data(PyObject *unicode);

#ifndef PYTHON_API_VERSION
#define PYTHON_API_VERSION 1013
#endif

#include <frameobject.h>

#define METH_VARARGS 0x0001
#define METH_KEYWORDS 0x0002
#define METH_NOARGS 0x0004
#define METH_O 0x0008
#define METH_CLASS 0x0010
#define METH_STATIC 0x0020
#define METH_COEXIST 0x0040
#define _MOLT_METH_CALL_MASK (METH_VARARGS | METH_KEYWORDS | METH_NOARGS | METH_O)
#define _MOLT_METH_MODIFIER_MASK (METH_CLASS | METH_STATIC | METH_COEXIST)
#define _MOLT_TYPE_MODULE_ATTR "__molt_type_module__"

#define Py_nb_add 7
#define Py_nb_multiply 29
#define Py_nb_subtract 36
#define Py_sq_concat 40
#define Py_tp_base 48
#define Py_tp_bases 49
#define Py_tp_call 50
#define Py_tp_doc 56
#define Py_tp_iter 62
#define Py_tp_iternext 63
#define Py_tp_methods 64
#define Py_tp_new 65
#define Py_tp_repr 66
#define Py_tp_str 70
#define Py_tp_members 72
#define Py_tp_getset 73
#define Py_tp_dealloc 52
#define Py_tp_traverse 71
#define Py_tp_clear 51

#define Py_LT 0
#define Py_LE 1
#define Py_EQ 2
#define Py_NE 3
#define Py_GT 4
#define Py_GE 5

#define READONLY Py_READONLY
#define T_OBJECT _Py_T_OBJECT
#define T_OBJECT_EX Py_T_OBJECT_EX

#define Py_TPFLAGS_DEFAULT 0UL
#define Py_TPFLAGS_BASETYPE (1UL << 10)
#define Py_TPFLAGS_HEAPTYPE (1UL << 9)
#define Py_TPFLAGS_HAVE_GC (1UL << 14)
#define Py_TPFLAGS_METHOD_DESCRIPTOR (1UL << 17)
#define Py_TPFLAGS_HAVE_VECTORCALL (1UL << 11)
#define Py_TPFLAGS_MANAGED_DICT (1UL << 4)

#define PyModuleDef_HEAD_INIT NULL
#define PyModule_AddIntMacro(module, macro) \
    PyModule_AddIntConstant((module), #macro, (long)(macro))
#define PyModule_AddStringMacro(module, macro) \
    PyModule_AddStringConstant((module), #macro, (macro))

#define Py_SUCCESS 0
#define Py_FAILURE -1
#define PY_SSIZE_T_MAX ((Py_ssize_t)(SIZE_MAX >> 1))
#define PY_SSIZE_T_MIN (-PY_SSIZE_T_MAX - 1)

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0
#define PY_RELEASE_LEVEL_FINAL 0xF
#define PY_RELEASE_LEVEL PY_RELEASE_LEVEL_FINAL
#define PY_RELEASE_SERIAL 0
#define PY_VERSION_HEX ((PY_MAJOR_VERSION << 24) | (PY_MINOR_VERSION << 16) | (PY_MICRO_VERSION << 8) | (PY_RELEASE_LEVEL << 4) | PY_RELEASE_SERIAL)
#define PyLong_SHIFT 30

#define PyOS_snprintf snprintf
#define Py_HUGE_VAL HUGE_VAL

#define PyGILState_LOCKED 0
#define PyGILState_UNLOCKED 1

#define Py_GIL_DISABLED 0
#define Py_MOD_GIL_NOT_USED 1
#define Py_MOD_PER_INTERPRETER_GIL_SUPPORTED 1
#define Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED 0
#define Py_mod_exec 2
#define Py_mod_gil 3
#define Py_mod_multiple_interpreters 3

#define PyBUF_SIMPLE 0
#define PyBUF_WRITABLE 0x0001
#define PyBUF_FORMAT 0x0004
#define PyBUF_ND 0x0008
#define PyBUF_STRIDES (0x0010 | PyBUF_ND)
#define PyBUF_C_CONTIGUOUS (0x0020 | PyBUF_STRIDES)
#define PyBUF_F_CONTIGUOUS (0x0040 | PyBUF_STRIDES)
#define PyBUF_ANY_CONTIGUOUS (0x0080 | PyBUF_STRIDES)
#define PyBUF_WRITEABLE PyBUF_WRITABLE
#define PyBUF_INDIRECT (0x0100 | PyBUF_STRIDES)
#define PyBUF_STRIDED (PyBUF_STRIDES | PyBUF_WRITABLE)
#define PyBUF_STRIDED_RO (PyBUF_STRIDES)
#define PyBUF_RECORDS (PyBUF_STRIDES | PyBUF_WRITABLE | PyBUF_FORMAT)
#define PyBUF_RECORDS_RO (PyBUF_STRIDES | PyBUF_FORMAT)
#define PyBUF_FULL (PyBUF_INDIRECT | PyBUF_WRITABLE | PyBUF_FORMAT)
#define PyBUF_FULL_RO (PyBUF_INDIRECT | PyBUF_FORMAT)
#define WAIT_LOCK 1
#define NOWAIT_LOCK 0
#define PY_TIMEOUT_MAX LLONG_MAX

#ifndef Py_BEGIN_CRITICAL_SECTION
#define Py_BEGIN_CRITICAL_SECTION(obj) do { (void)(obj); } while (0)
#define Py_END_CRITICAL_SECTION() do { } while (0)
#define Py_BEGIN_CRITICAL_SECTION2(a, b)                                         \
    do {                                                                          \
        (void)(a);                                                                \
        (void)(b);                                                                \
    } while (0)
#define Py_END_CRITICAL_SECTION2() do { } while (0)
#endif

#if !defined(Py_LIMITED_API) && !defined(_MULTIARRAYMODULE) && !defined(_UMATHMODULE)
#define Py_LIMITED_API 0x030C0000
#endif

#ifndef PY_LONG_LONG
#define PY_LONG_LONG long long
#endif

#define PY_VECTORCALL_ARGUMENTS_OFFSET ((size_t)1 << (8 * sizeof(size_t) - 1))

#ifndef PyAPI_FUNC
#define PyAPI_FUNC(RTYPE) RTYPE
#endif

#ifndef PyAPI_DATA
#define PyAPI_DATA(RTYPE) extern RTYPE
#endif

#ifndef PyMODINIT_FUNC
#define PyMODINIT_FUNC PyObject *
#endif

#define PyInit_ PyInit_

#define Py_DEBUG 0
#define Py_PRINT_RAW 1
#define Py_CLEANUP_SUPPORTED 0x20000
#define Py_UNREACHABLE() abort()
#define Py_CHARMASK(c) ((unsigned char)((c) & 0xff))
#define Py_SAFE_DOWNCAST(VALUE, WIDE, NARROW) ((NARROW)(VALUE))
#define Py_MIN(A, B) ((A) < (B) ? (A) : (B))
#define Py_MAX(A, B) ((A) > (B) ? (A) : (B))

#define PyObject_HEAD_INIT(type) 0, (PyObject *)(type),
#define PyVarObject_HEAD_INIT(type, size) PyObject_HEAD_INIT(type)
#define PyObject_HEAD PyObject ob_base;
#define PyObject_VAR_HEAD PyObject ob_base;
#define Py_SIZE(ob) (((PyVarObject *)(ob))->ob_size)
#define Py_SET_SIZE(ob, size) (((PyVarObject *)(ob))->ob_size = (Py_ssize_t)(size))

static int Py_OptimizeFlag = 0;

#if defined(__GNUC__) || defined(__clang__)
#define Py_UNUSED(name) name __attribute__((unused))
#else
#define Py_UNUSED(name) name
#endif

static inline MoltHandle _molt_py_handle(const PyObject *obj) {
    return (MoltHandle)(uintptr_t)obj;
}

static inline PyObject *_molt_pyobject_from_handle(MoltHandle bits) {
    return (PyObject *)(uintptr_t)bits;
}

static inline PyObject *_molt_pyobject_from_result(MoltHandle bits) {
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(bits);
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

static inline MoltHandle _molt_exception_class_from_name(const char *name) {
    MoltHandle kind_bits = _molt_string_from_utf8(name);
    MoltHandle class_bits;
    if (kind_bits == 0 || molt_err_pending() != 0) {
        return 0;
    }
    class_bits = molt_exception_class(kind_bits);
    molt_handle_decref(kind_bits);
    if (molt_err_pending() != 0) {
        return 0;
    }
    return class_bits;
}

static inline PyObject *_molt_pyexc_type_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("TypeError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_value_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("ValueError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_runtime_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("RuntimeError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_overflow_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("OverflowError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_import_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("ImportError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_module_not_found_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("ModuleNotFoundError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_permission_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("PermissionError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_key_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("KeyError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_memory_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("MemoryError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_index_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("IndexError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_system_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("SystemError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_attribute_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("AttributeError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_name_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("NameError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_assertion_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("AssertionError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_not_implemented_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("NotImplementedError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_runtime_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("RuntimeWarning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_user_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("UserWarning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_os_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("OSError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_stop_iteration(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("StopIteration");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_deprecation_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("DeprecationWarning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_future_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("FutureWarning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_unicode_decode_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("UnicodeDecodeError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_buffer_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("BufferError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_exception(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("Exception");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("Warning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_import_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("ImportWarning");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_recursion_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("RecursionError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_unicode_encode_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("UnicodeEncodeError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_floating_point_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("FloatingPointError");
    }
    return _molt_pyobject_from_handle(cached);
}

static inline PyObject *_molt_pyexc_zero_division_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("ZeroDivisionError");
    }
    return _molt_pyobject_from_handle(cached);
}

#define PyExc_TypeError _molt_pyexc_type_error()
#define PyExc_ValueError _molt_pyexc_value_error()
#define PyExc_Exception _molt_pyexc_exception()
#define PyExc_RuntimeError _molt_pyexc_runtime_error()
#define PyExc_OverflowError _molt_pyexc_overflow_error()
#define PyExc_ImportError _molt_pyexc_import_error()
#define PyExc_ImportWarning _molt_pyexc_import_warning()
#define PyExc_ModuleNotFoundError _molt_pyexc_module_not_found_error()
#define PyExc_PermissionError _molt_pyexc_permission_error()
#define PyExc_KeyError _molt_pyexc_key_error()
#define PyExc_MemoryError _molt_pyexc_memory_error()
#define PyExc_IndexError _molt_pyexc_index_error()
#define PyExc_RecursionError _molt_pyexc_recursion_error()
#define PyExc_SystemError _molt_pyexc_system_error()
#define PyExc_AttributeError _molt_pyexc_attribute_error()
#define PyExc_NameError _molt_pyexc_name_error()
#define PyExc_AssertionError _molt_pyexc_assertion_error()
#define PyExc_NotImplementedError _molt_pyexc_not_implemented_error()
#define PyExc_FloatingPointError _molt_pyexc_floating_point_error()
#define PyExc_RuntimeWarning _molt_pyexc_runtime_warning()
#define PyExc_UserWarning _molt_pyexc_user_warning()
#define PyExc_Warning _molt_pyexc_warning()
#define PyExc_DeprecationWarning _molt_pyexc_deprecation_warning()
#define PyExc_FutureWarning _molt_pyexc_future_warning()
#define PyExc_UnicodeDecodeError _molt_pyexc_unicode_decode_error()
#define PyExc_UnicodeEncodeError _molt_pyexc_unicode_encode_error()
#define PyExc_ZeroDivisionError _molt_pyexc_zero_division_error()
#define PyExc_BufferError _molt_pyexc_buffer_error()
#define PyExc_OSError _molt_pyexc_os_error()
#define PyExc_IOError PyExc_OSError
#define PyExc_StopIteration _molt_pyexc_stop_iteration()

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
    if (errno == ERANGE && overflow_exception != NULL) {
        PyErr_SetString(overflow_exception, "floating-point conversion overflow");
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

static inline void Py_FatalError(const char *message) {
    fputs(message != NULL ? message : "fatal Python error", stderr);
    fputc('\n', stderr);
    abort();
}

static inline PyThreadState *PyThreadState_Get(void) {
    static PyInterpreterState interp = {0};
    static PyThreadState state = {0};
    if (molt_gil_is_held() == 0) {
        PyErr_SetString(PyExc_RuntimeError, "PyThreadState_Get requires the GIL");
        return NULL;
    }
    if (state.interp == NULL) {
        state.interp = &interp;
        state.id = 1;
    }
    return &state;
}

static inline PyInterpreterState *PyThreadState_GetInterpreter(PyThreadState *tstate) {
    return tstate != NULL ? tstate->interp : NULL;
}

static inline PyFrameObject *PyThreadState_GetFrame(PyThreadState *tstate) {
    return (tstate != NULL && tstate->frame != NULL)
        ? (PyFrameObject *)Py_NewRef((PyObject *)tstate->frame)
        : NULL;
}

static inline PyInterpreterState *PyInterpreterState_Get(void) {
    PyThreadState *tstate = PyThreadState_Get();
    return tstate != NULL ? tstate->interp : NULL;
}

static inline PyInterpreterState *PyInterpreterState_Main(void) {
    return PyInterpreterState_Get();
}

static inline PyObject *PyThreadState_GetDict(void) {
    static MoltHandle dict_bits = 0;
    if (dict_bits == 0) {
        dict_bits = molt_dict_from_pairs(NULL, NULL, 0);
        if (dict_bits == 0 || molt_err_pending() != 0) {
            dict_bits = 0;
            return NULL;
        }
    }
    return _molt_pyobject_from_handle(dict_bits);
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

static inline int PyGILState_Check(void) {
    return molt_gil_is_held() != 0;
}

static inline int PyGC_Collect(void) {
    return 0;
}

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

static inline PyThread_type_lock PyThread_allocate_lock(void) {
    return calloc(1, sizeof(int));
}

static inline void PyThread_free_lock(PyThread_type_lock lock) {
    free(lock);
}

static inline int PyThread_acquire_lock(PyThread_type_lock lock, int waitflag) {
    int *state = (int *)lock;
    if (state == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "PyThread lock must not be NULL");
        return 0;
    }
    if (*state == 0) {
        *state = 1;
        return 1;
    }
    if (waitflag == NOWAIT_LOCK) {
        return 0;
    }
    PyErr_SetString(
        PyExc_RuntimeError,
        "blocking PyThread locks are not yet implemented in Molt's C-API layer");
    return 0;
}

static inline PyLockStatus PyThread_acquire_lock_timed(
    PyThread_type_lock lock,
    PY_TIMEOUT_T microseconds,
    int intr_flag
) {
    (void)microseconds;
    (void)intr_flag;
    return PyThread_acquire_lock(lock, NOWAIT_LOCK) ? PY_LOCK_ACQUIRED : PY_LOCK_FAILURE;
}

static inline void PyThread_release_lock(PyThread_type_lock lock) {
    int *state = (int *)lock;
    if (state != NULL) {
        *state = 0;
    }
}

static inline void PyMutex_Lock(PyMutex *mutex) {
    if (mutex == NULL) {
        return;
    }
    if (mutex->_molt_lock == NULL) {
        mutex->_molt_lock = PyThread_allocate_lock();
        if (mutex->_molt_lock == NULL) {
            return;
        }
    }
    (void)PyThread_acquire_lock(mutex->_molt_lock, WAIT_LOCK);
}

static inline void PyMutex_Unlock(PyMutex *mutex) {
    if (mutex != NULL && mutex->_molt_lock != NULL) {
        PyThread_release_lock(mutex->_molt_lock);
    }
}

static inline void PyCriticalSection_Begin(PyCriticalSection *critical_section) {
    (void)critical_section;
}

static inline void PyCriticalSection_End(PyCriticalSection *critical_section) {
    (void)critical_section;
}

static inline PyThreadState *PyEval_SaveThread(void) {
    PyThreadState *state = NULL;
    if (molt_gil_is_held() != 0) {
        state = PyThreadState_Get();
        if (state != NULL) {
            (void)molt_gil_release();
        }
    }
    return state;
}

static inline void PyEval_RestoreThread(PyThreadState *state) {
    if (state != NULL && molt_gil_is_held() == 0) {
        (void)molt_gil_acquire();
    }
}

#define Py_BEGIN_ALLOW_THREADS                                                     \
    {                                                                              \
        PyThreadState *_molt_threadstate_save = PyEval_SaveThread();

#define Py_END_ALLOW_THREADS                                                       \
        PyEval_RestoreThread(_molt_threadstate_save);                              \
    }

static inline void Py_IncRef(PyObject *obj) {
    if (obj != NULL) {
        molt_handle_incref(_molt_py_handle(obj));
    }
}

static inline void Py_DecRef(PyObject *obj) {
    if (obj != NULL) {
        molt_handle_decref(_molt_py_handle(obj));
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
#define Py_VISIT(op)                                                                \
    do {                                                                           \
        if ((op) != NULL) {                                                        \
        }                                                                          \
    } while (0)

#define Py_None _molt_pyobject_from_handle(molt_none())
#define Py_True _molt_pyobject_from_handle(molt_bool_from_i32(1))
#define Py_False _molt_pyobject_from_handle(molt_bool_from_i32(0))

static inline PyObject *_molt_pynotimplemented_singleton(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        PyObject *notimpl_type = _molt_builtin_class_lookup_utf8("NotImplementedType");
        PyObject *value;
        if (notimpl_type == NULL) {
            PyErr_Clear();
            return Py_None;
        }
        value = PyObject_CallObject(notimpl_type, NULL);
        Py_DECREF(notimpl_type);
        if (value == NULL) {
            PyErr_Clear();
            return Py_None;
        }
        cached = _molt_py_handle(value);
    }
    return _molt_pyobject_from_handle(cached);
}

#define Py_NotImplemented _molt_pynotimplemented_singleton()

static inline PyObject *_molt_pyellipsis_singleton(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        PyObject *ellipsis_type = _molt_builtin_class_lookup_utf8("ellipsis");
        PyObject *value;
        if (ellipsis_type == NULL) {
            PyErr_Clear();
            return Py_None;
        }
        value = PyObject_CallObject(ellipsis_type, NULL);
        Py_DECREF(ellipsis_type);
        if (value == NULL) {
            PyErr_Clear();
            return Py_None;
        }
        cached = _molt_py_handle(value);
    }
    return _molt_pyobject_from_handle(cached);
}

#define Py_Ellipsis _molt_pyellipsis_singleton()

static inline PyTypeObject *_molt_py_typeof(PyObject *obj) {
    PyObject *type_obj = PyObject_GetAttrString(obj, "__class__");
    if (type_obj == NULL) {
        return NULL;
    }
    Py_DECREF(type_obj);
    return (PyTypeObject *)type_obj;
}

static inline void _molt_py_set_type(PyObject *obj, PyTypeObject *type_obj) {
    if (obj == NULL || type_obj == NULL) {
        return;
    }
    (void)PyObject_SetAttrString(obj, "__class__", (PyObject *)type_obj);
}

#define Py_TYPE(ob) _molt_py_typeof((PyObject *)(ob))
#define Py_IS_TYPE(ob, type) (Py_TYPE(ob) == (type))
#define Py_SET_TYPE(ob, type_obj) _molt_py_set_type((PyObject *)(ob), (PyTypeObject *)(type_obj))
#define Py_REFCNT(ob) ((Py_ssize_t)1)
#define Py_SET_REFCNT(ob, refcnt) ((void)(ob), (void)(refcnt))
#define _PyDict_GetItemWithError PyDict_GetItemWithError
#define PyThreadState_GET() PyThreadState_Get()
#define PyObject_New(type, typeobj) ((type *)PyObject_CallObject((PyObject *)(typeobj), NULL))
#define PyObject_NewVar(type, typeobj, nitems) ((type *)PyObject_CallObject((PyObject *)(typeobj), NULL))
#define PyObject_GC_New(type, typeobj) PyObject_New(type, typeobj)
#define PyObject_INIT(obj, typeobj) PyObject_Init((PyObject *)(obj), (PyTypeObject *)(typeobj))
#define PyObject_IS_GC(obj)                                                       \
    ((obj) != NULL && (Py_TYPE(obj) != NULL)                                     \
     && (((Py_TYPE(obj))->tp_flags & Py_TPFLAGS_HAVE_GC) != 0))

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

static inline PyObject *PyObject_Init(PyObject *obj, PyTypeObject *typeobj) {
    if (obj == NULL || typeobj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object and type must not be NULL");
        return NULL;
    }
    Py_SET_TYPE(obj, typeobj);
    return obj;
}

static inline PyObject *PyObject_InitVar(PyVarObject *obj, PyTypeObject *typeobj, Py_ssize_t size) {
    PyObject *base;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "variable object must not be NULL");
        return NULL;
    }
    base = PyObject_Init((PyObject *)obj, typeobj);
    if (base == NULL) {
        return NULL;
    }
    Py_SET_SIZE(obj, size);
    return base;
}

static inline int PyUnstable_Object_IsUniqueReferencedTemporary(PyObject *obj) {
    return obj != NULL ? 1 : 0;
}

static inline PyCodeObject *PyFrame_GetCode(PyFrameObject *frame) {
    if (frame == NULL || frame->f_code == NULL) {
        return NULL;
    }
    return (PyCodeObject *)Py_XNewRef((PyObject *)frame->f_code);
}

static inline PyFrameObject *PyFrame_GetBack(PyFrameObject *frame) {
    if (frame == NULL || frame->f_back == NULL) {
        return NULL;
    }
    return (PyFrameObject *)Py_XNewRef((PyObject *)frame->f_back);
}

static inline PyObject *PyFrame_GetLocals(PyFrameObject *frame) {
    if (frame == NULL) {
        return NULL;
    }
    if (PyFrame_FastToLocalsWithError(frame) < 0) {
        return NULL;
    }
    return Py_XNewRef(frame->f_locals);
}

static inline PyObject *PyFrame_GetGlobals(PyFrameObject *frame) {
    return frame != NULL ? Py_XNewRef(frame->f_globals) : NULL;
}

static inline PyObject *PyFrame_GetBuiltins(PyFrameObject *frame) {
    return frame != NULL ? Py_XNewRef(frame->f_builtins) : NULL;
}

#define PyMODINIT_FUNC PyObject *

static inline PyObject *PyErr_Occurred(void) {
    if (molt_err_pending() == 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(molt_err_peek());
}

static inline void PyErr_Clear(void) {
    (void)molt_err_clear();
}

static inline int PyErr_ExceptionMatches(PyObject *exc) {
    if (exc == NULL) {
        return 0;
    }
    return molt_err_matches(_molt_py_handle(exc));
}

static inline int PyErr_GivenExceptionMatches(PyObject *given, PyObject *expected) {
    if (given == NULL || expected == NULL) {
        return 0;
    }
    return molt_object_equal(_molt_py_handle(given), _molt_py_handle(expected));
}

static inline void PyErr_SetString(PyObject *exc, const char *message) {
    const char *msg = message != NULL ? message : "";
    MoltHandle exc_bits = exc != NULL ? _molt_py_handle(exc) : _molt_py_handle(PyExc_RuntimeError);
    (void)molt_err_set(exc_bits, (const uint8_t *)msg, (uint64_t)strlen(msg));
}

static inline void PyErr_SetNone(PyObject *exc) {
    MoltHandle exc_bits = exc != NULL ? _molt_py_handle(exc) : _molt_py_handle(PyExc_RuntimeError);
    (void)molt_err_set(exc_bits, (const uint8_t *)"", 0);
}

static inline void PyErr_SetObject(PyObject *exc, PyObject *value) {
    MoltHandle exc_bits = exc != NULL ? _molt_py_handle(exc) : _molt_py_handle(PyExc_RuntimeError);
    if (value == NULL) {
        (void)molt_err_set(exc_bits, (const uint8_t *)"", 0);
        return;
    }
    (void)molt_err_restore(_molt_py_handle(value));
}

static inline PyObject *PyErr_Format(PyObject *exc, const char *fmt, ...) {
    char buffer[1024];
    va_list ap;
    size_t len;
    va_start(ap, fmt);
    (void)vsnprintf(buffer, sizeof(buffer), fmt, ap);
    va_end(ap);
    len = _molt_strnlen(buffer, sizeof(buffer));
    (void)molt_err_set(
        exc != NULL ? _molt_py_handle(exc) : _molt_py_handle(PyExc_RuntimeError),
        (const uint8_t *)buffer,
        (uint64_t)len);
    return NULL;
}

static inline PyObject *PyErr_NoMemory(void) {
    PyErr_SetString(PyExc_MemoryError, "out of memory");
    return NULL;
}

static inline void PyErr_BadInternalCall(void) {
    PyErr_SetString(PyExc_SystemError, "bad internal call");
}

static inline int PyErr_CheckSignals(void) {
    return 0;
}

static inline PyObject *PyErr_FormatV(PyObject *exc, const char *format, va_list vargs) {
    char stack_buf[1024];
    va_list copy;
    int needed;
    if (format == NULL) {
        PyErr_SetString(PyExc_TypeError, "format must not be NULL");
        return NULL;
    }
    va_copy(copy, vargs);
    needed = vsnprintf(stack_buf, sizeof(stack_buf), format, copy);
    va_end(copy);
    if (needed < 0) {
        PyErr_SetString(PyExc_ValueError, "failed to format error message");
        return NULL;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        PyErr_SetString(exc != NULL ? exc : PyExc_RuntimeError, stack_buf);
        return NULL;
    }
    {
        size_t cap = (size_t)needed + 1;
        char *heap_buf = (char *)PyMem_Malloc(cap);
        if (heap_buf == NULL) {
            return PyErr_NoMemory();
        }
        va_copy(copy, vargs);
        (void)vsnprintf(heap_buf, cap, format, copy);
        va_end(copy);
        PyErr_SetString(exc != NULL ? exc : PyExc_RuntimeError, heap_buf);
        PyMem_Free(heap_buf);
    }
    return NULL;
}

static inline PyObject *PyErr_NewException(const char *name, PyObject *base, PyObject *dict) {
    const char *short_name = name;
    const char *dot;
    PyTypeObject *type_type;
    PyObject *name_obj;
    PyObject *bases_obj;
    PyObject *dict_obj = dict;
    PyObject *out;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "exception name must not be empty");
        return NULL;
    }
    dot = strrchr(name, '.');
    if (dot != NULL && dot[1] != '\0') {
        short_name = dot + 1;
    }
    if (base == NULL) {
        base = PyExc_Exception;
    }
    type_type = _molt_builtin_type_object_borrowed("type");
    if (type_type == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "type constructor is unavailable");
        return NULL;
    }
    name_obj = PyUnicode_FromString(short_name);
    if (name_obj == NULL) {
        return NULL;
    }
    bases_obj = PyTuple_Pack(1, base);
    if (bases_obj == NULL) {
        Py_DECREF(name_obj);
        return NULL;
    }
    if (dict_obj == NULL) {
        dict_obj = PyDict_New();
        if (dict_obj == NULL) {
            Py_DECREF(name_obj);
            Py_DECREF(bases_obj);
            return NULL;
        }
    } else {
        Py_INCREF(dict_obj);
    }
    out = PyObject_CallFunctionObjArgs((PyObject *)type_type, name_obj, bases_obj, dict_obj, NULL);
    Py_DECREF(dict_obj);
    Py_DECREF(bases_obj);
    Py_DECREF(name_obj);
    return out;
}

static inline PyObject *PyErr_SetFromErrno(PyObject *exc) {
    const char *message = strerror(errno);
    PyErr_SetString(exc != NULL ? exc : PyExc_OSError, message != NULL ? message : "OS error");
    return NULL;
}

static inline PyObject *PyErr_SetFromErrnoWithFilenameObject(PyObject *exc, PyObject *filename) {
    const char *message = strerror(errno);
    const char *filename_text = NULL;
    if (filename != NULL && filename != Py_None) {
        filename_text = PyUnicode_AsUTF8(filename);
    }
    if (filename_text != NULL && filename_text[0] != '\0') {
        PyErr_Format(
            exc != NULL ? exc : PyExc_OSError,
            "%s: %s",
            message != NULL ? message : "OS error",
            filename_text);
    } else {
        PyErr_SetString(exc != NULL ? exc : PyExc_OSError, message != NULL ? message : "OS error");
    }
    return NULL;
}

static inline PyObject *PyMember_GetOne(const char *obj_addr, PyMemberDef *m) {
    (void)obj_addr;
    (void)m;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyMember_GetOne is not yet implemented in Molt's C-API compatibility layer");
    return NULL;
}

static inline int PyMember_SetOne(char *obj_addr, PyMemberDef *m, PyObject *o) {
    (void)obj_addr;
    (void)m;
    (void)o;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyMember_SetOne is not yet implemented in Molt's C-API compatibility layer");
    return -1;
}

static inline int PyErr_WarnEx(PyObject *category, const char *message, Py_ssize_t stacklevel) {
    (void)category;
    (void)message;
    (void)stacklevel;
    return 0;
}

static inline int PyErr_WarnFormat(
    PyObject *category,
    Py_ssize_t stacklevel,
    const char *format,
    ...
) {
    char buffer[1024];
    va_list ap;
    va_start(ap, format);
    (void)vsnprintf(buffer, sizeof(buffer), format != NULL ? format : "", ap);
    va_end(ap);
    return PyErr_WarnEx(category, buffer, stacklevel);
}

static inline void PyErr_WriteUnraisable(PyObject *obj) {
    (void)obj;
    PyErr_Clear();
}

static inline void PyErr_NormalizeException(
    PyObject **ptype,
    PyObject **pvalue,
    PyObject **ptraceback
) {
    if (ptraceback != NULL && *ptraceback != NULL) {
        return;
    }
    if (ptype == NULL || *ptype == NULL) {
        return;
    }
    if (pvalue != NULL && *pvalue == NULL) {
        *pvalue = Py_NewRef(*ptype);
    }
}

static inline void PyErr_Print(void) {
    PyObject *ptype = NULL;
    PyObject *pvalue = NULL;
    PyObject *ptraceback = NULL;
    const char *text = NULL;
    PyErr_Fetch(&ptype, &pvalue, &ptraceback);
    PyErr_NormalizeException(&ptype, &pvalue, &ptraceback);
    if (pvalue != NULL) {
        PyObject *rendered = PyObject_Str(pvalue);
        if (rendered != NULL) {
            text = PyUnicode_AsUTF8(rendered);
            if (text != NULL) {
                (void)fprintf(stderr, "%s\n", text);
            }
            Py_DECREF(rendered);
        }
    }
    Py_XDECREF(ptype);
    Py_XDECREF(pvalue);
    Py_XDECREF(ptraceback);
    PyErr_Clear();
}

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
#define PyMem_CALLOC PyMem_Calloc
#define PyMem_REALLOC PyMem_Realloc
#define PyMem_FREE PyMem_Free
#define PyObject_MALLOC PyMem_Malloc
#define PyObject_CALLOC PyMem_Calloc
#define PyObject_REALLOC PyMem_Realloc
#define PyObject_FREE PyMem_Free
#define PyObject_Malloc PyMem_Malloc
#define PyObject_Realloc PyMem_Realloc
#define PyObject_Free PyMem_Free
#define PyObject_Del PyMem_Free
#define PyObject_GC_Del PyMem_Free

static inline void PyErr_Fetch(PyObject **ptype, PyObject **pvalue, PyObject **ptraceback) {
    MoltHandle exc_bits = molt_err_fetch();
    MoltHandle kind_bits;
    MoltHandle class_bits;
    if (ptype != NULL) {
        *ptype = NULL;
    }
    if (pvalue != NULL) {
        *pvalue = NULL;
    }
    if (ptraceback != NULL) {
        *ptraceback = NULL;
    }
    if (molt_err_pending() != 0 || exc_bits == 0 || exc_bits == molt_none()) {
        return;
    }
    kind_bits = molt_exception_kind(exc_bits);
    if (molt_err_pending() != 0 || kind_bits == 0 || kind_bits == molt_none()) {
        if (pvalue != NULL) {
            *pvalue = _molt_pyobject_from_handle(exc_bits);
        } else {
            molt_handle_decref(exc_bits);
        }
        return;
    }
    class_bits = molt_exception_class(kind_bits);
    molt_handle_decref(kind_bits);
    if (molt_err_pending() == 0 && class_bits != 0 && class_bits != molt_none()) {
        if (ptype != NULL) {
            *ptype = _molt_pyobject_from_handle(class_bits);
        } else {
            molt_handle_decref(class_bits);
        }
    }
    if (pvalue != NULL) {
        *pvalue = _molt_pyobject_from_handle(exc_bits);
    } else {
        molt_handle_decref(exc_bits);
    }
}

static inline void PyErr_Restore(PyObject *type, PyObject *value, PyObject *traceback) {
    MoltHandle restore_bits = 0;
    (void)traceback;
    if (value != NULL) {
        restore_bits = _molt_py_handle(value);
    } else if (type != NULL) {
        restore_bits = _molt_py_handle(type);
    }
    if (restore_bits != 0) {
        (void)molt_err_restore(restore_bits);
    } else {
        (void)molt_err_clear();
    }
}

static inline void PyException_SetCause(PyObject *self, PyObject *cause) {
    if (self != NULL && cause != NULL) {
        (void)PyObject_SetAttrString(self, "__cause__", cause);
    }
}

static inline void PyException_SetContext(PyObject *self, PyObject *context) {
    if (self != NULL && context != NULL) {
        (void)PyObject_SetAttrString(self, "__context__", context);
    }
}

static inline int PyException_SetTraceback(PyObject *self, PyObject *traceback) {
    if (self != NULL && traceback != NULL) {
        return PyObject_SetAttrString(self, "__traceback__", traceback);
    }
    return 0;
}

static inline PyObject *PySeqIter_New(PyObject *seq) {
    return PyObject_GetIter(seq);
}

static inline PyObject *PySequence_Concat(PyObject *left, PyObject *right) {
    return PyObject_CallMethod(left, "__add__", "O", right);
}

static inline PyObject *PyExceptionInstance_Class(PyObject *exc) {
    if (exc == NULL) {
        PyErr_SetString(PyExc_TypeError, "exception instance must not be NULL");
        return NULL;
    }
    return PyObject_Type(exc);
}

static inline PyObject *PyObject_GetAttr(PyObject *obj, PyObject *name) {
    return _molt_pyobject_from_result(
        molt_object_getattr(_molt_py_handle(obj), _molt_py_handle(name)));
}

static inline int PyObject_GetOptionalAttr(PyObject *obj, PyObject *name, PyObject **result) {
    PyObject *value;
    if (result == NULL) {
        PyErr_SetString(PyExc_TypeError, "result must not be NULL");
        return -1;
    }
    *result = NULL;
    value = PyObject_GetAttr(obj, name);
    if (value == NULL) {
        if (PyErr_ExceptionMatches(PyExc_AttributeError)) {
            PyErr_Clear();
            return 0;
        }
        return -1;
    }
    *result = value;
    return 1;
}

static inline PyObject *PyObject_GetAttrString(PyObject *obj, const char *name) {
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "attribute name must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_object_getattr_bytes(
        _molt_py_handle(obj), (const uint8_t *)name, (uint64_t)strlen(name)));
}

static inline int PyObject_SetAttr(PyObject *obj, PyObject *name, PyObject *value) {
    return molt_object_setattr(_molt_py_handle(obj), _molt_py_handle(name), _molt_py_handle(value));
}

static inline int PyObject_SetAttrString(PyObject *obj, const char *name, PyObject *value) {
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "attribute name must not be NULL");
        return -1;
    }
    return molt_object_setattr_bytes(
        _molt_py_handle(obj), (const uint8_t *)name, (uint64_t)strlen(name), _molt_py_handle(value));
}

static inline int PyObject_HasAttr(PyObject *obj, PyObject *name) {
    return molt_object_hasattr(_molt_py_handle(obj), _molt_py_handle(name));
}

static inline int PyObject_HasAttrString(PyObject *obj, const char *name) {
    PyObject *name_obj;
    int out;
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "attribute name must not be NULL");
        return 0;
    }
    name_obj = _molt_pyobject_from_result(_molt_string_from_utf8(name));
    if (name_obj == NULL) {
        return 0;
    }
    out = PyObject_HasAttr(obj, name_obj);
    Py_DECREF(name_obj);
    return out;
}

static inline int PyObject_DelAttrString(PyObject *obj, const char *name) {
    PyObject *name_obj;
    PyObject *result;
    if (obj == NULL || name == NULL) {
        PyErr_SetString(PyExc_TypeError, "object and attribute name must not be NULL");
        return -1;
    }
    name_obj = PyUnicode_FromString(name);
    if (name_obj == NULL) {
        return -1;
    }
    result = PyObject_CallMethod(obj, "__delattr__", "O", name_obj);
    Py_DECREF(name_obj);
    if (result == NULL) {
        return -1;
    }
    Py_DECREF(result);
    return 0;
}

static inline PyObject *PyObject_GenericGetAttr(PyObject *obj, PyObject *name) {
    return PyObject_GetAttr(obj, name);
}

static inline int PyObject_GenericSetAttr(PyObject *obj, PyObject *name, PyObject *value) {
    return PyObject_SetAttr(obj, name, value);
}

static inline PyObject *PyObject_GenericGetDict(PyObject *obj, void *context) {
    (void)context;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    return PyObject_GetAttrString(obj, "__dict__");
}

static inline int PyObject_CheckBuffer(PyObject *obj) {
    if (obj == NULL) {
        return 0;
    }
    if (PyObject_HasAttrString(obj, "__buffer__") > 0) {
        return 1;
    }
    if (PyObject_HasAttrString(obj, "tobytes") > 0 && PyObject_HasAttrString(obj, "nbytes") > 0) {
        return 1;
    }
    PyErr_Clear();
    return 0;
}

static inline void PyObject_ClearWeakRefs(PyObject *obj) {
    (void)obj;
}

static inline int _PyObject_LookupAttr(PyObject *obj, PyObject *name, PyObject **result) {
    PyObject *value;
    if (result == NULL) {
        PyErr_SetString(PyExc_TypeError, "result must not be NULL");
        return -1;
    }
    *result = NULL;
    value = PyObject_GetAttr(obj, name);
    if (value == NULL) {
        if (PyErr_ExceptionMatches(PyExc_AttributeError)) {
            PyErr_Clear();
            return 0;
        }
        return -1;
    }
    *result = value;
    return 1;
}

static inline PyObject *PyObject_CallObject(PyObject *callable, PyObject *args) {
    MoltHandle args_bits;
    int owns_args = 0;
    MoltHandle out;
    if (args == NULL) {
        args_bits = molt_tuple_from_array(NULL, 0);
        if (molt_err_pending() != 0) {
            return NULL;
        }
        owns_args = 1;
    } else {
        args_bits = _molt_py_handle(args);
    }
    out = molt_object_call(_molt_py_handle(callable), args_bits, molt_none());
    if (owns_args) {
        molt_handle_decref(args_bits);
    }
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyObject_CallOneArg(PyObject *callable, PyObject *arg) {
    PyObject *const args[] = {arg};
    return PyObject_Vectorcall(callable, args, 1, NULL);
}

static inline int PyWeakref_Check(PyObject *obj) {
    int has_callback;
    int has_call;
    if (obj == NULL) {
        return 0;
    }
    has_callback = PyObject_HasAttrString(obj, "__callback__");
    if (has_callback <= 0) {
        PyErr_Clear();
        return 0;
    }
    has_call = PyObject_HasAttrString(obj, "__call__");
    if (has_call <= 0) {
        PyErr_Clear();
        return 0;
    }
    return 1;
}

static inline PyObject *PyWeakref_GetObject(PyObject *ref) {
    static _Thread_local PyObject *cached_target = NULL;
    PyObject *target;
    if (cached_target != NULL) {
        Py_DECREF(cached_target);
        cached_target = NULL;
    }
    if (ref == NULL) {
        return Py_None;
    }
    target = PyObject_CallObject(ref, NULL);
    if (target == NULL) {
        return NULL;
    }
    cached_target = target;
    return cached_target;
}

static inline PyObject *PyWeakref_NewRef(PyObject *obj, PyObject *callback) {
    PyObject *weakref_module;
    PyObject *ref_type;
    PyObject *out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    weakref_module = PyImport_ImportModule("weakref");
    if (weakref_module == NULL) {
        return NULL;
    }
    ref_type = PyObject_GetAttrString(weakref_module, "ref");
    Py_DECREF(weakref_module);
    if (ref_type == NULL) {
        return NULL;
    }
    if (callback != NULL && callback != Py_None) {
        out = PyObject_CallFunctionObjArgs(ref_type, obj, callback, NULL);
    } else {
        out = PyObject_CallFunctionObjArgs(ref_type, obj, NULL);
    }
    Py_DECREF(ref_type);
    return out;
}

static inline PyObject **_PyObject_GetDictPtr(PyObject *obj) {
    static _Thread_local PyObject *cached_dict = NULL;
    if (cached_dict != NULL) {
        Py_DECREF(cached_dict);
        cached_dict = NULL;
    }
    if (obj == NULL) {
        return NULL;
    }
    cached_dict = PyObject_GetAttrString(obj, "__dict__");
    if (cached_dict == NULL) {
        PyErr_Clear();
        return NULL;
    }
    return &cached_dict;
}

static inline PyObject *PyObject_Call(PyObject *callable, PyObject *args, PyObject *kwargs) {
    MoltHandle args_bits;
    MoltHandle kwargs_bits;
    MoltHandle out;
    int owns_args = 0;
    if (args == NULL) {
        args_bits = molt_tuple_from_array(NULL, 0);
        if (args_bits == 0 || molt_err_pending() != 0) {
            return NULL;
        }
        owns_args = 1;
    } else {
        args_bits = _molt_py_handle(args);
    }
    kwargs_bits = kwargs != NULL ? _molt_py_handle(kwargs) : molt_none();
    out = molt_object_call(_molt_py_handle(callable), args_bits, kwargs_bits);
    if (owns_args) {
        molt_handle_decref(args_bits);
    }
    return _molt_pyobject_from_result(out);
}

/*
 * CPython can deduplicate finalizer calls for GC-aware types and detect
 * resurrection during dealloc. Molt currently provides a best-effort lane:
 * call __del__ when present and report unraisable exceptions.
 */
static inline void PyObject_CallFinalizer(PyObject *op) {
    int has_finalizer;
    PyObject *result;
    if (op == NULL) {
        return;
    }
    has_finalizer = PyObject_HasAttrString(op, "__del__");
    if (has_finalizer <= 0) {
        if (molt_err_pending() != 0) {
            PyErr_Clear();
        }
        return;
    }
    result = PyObject_CallMethod(op, "__del__", NULL);
    if (result == NULL) {
        PyErr_WriteUnraisable(op);
        return;
    }
    Py_DECREF(result);
}

static inline int PyObject_CallFinalizerFromDealloc(PyObject *op) {
    if (op == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return -1;
    }
    PyObject_CallFinalizer(op);
    return 0;
}

static inline void PyObject_GC_Track(PyObject *op) {
    (void)op;
}

static inline void PyObject_GC_UnTrack(PyObject *op) {
    (void)op;
}

static inline PyObject *PyObject_GetItem(PyObject *obj, PyObject *key) {
    return _molt_pyobject_from_result(
        molt_mapping_getitem(_molt_py_handle(obj), _molt_py_handle(key)));
}

static inline int PyObject_IsTrue(PyObject *obj) {
    return molt_object_truthy(_molt_py_handle(obj));
}

static inline Py_hash_t PyObject_Hash(PyObject *obj) {
    PyObject *hash_method;
    PyObject *hash_value;
    long long out;
    hash_method = PyObject_GetAttrString(obj, "__hash__");
    if (hash_method == NULL) {
        return (Py_hash_t)-1;
    }
    hash_value = PyObject_CallObject(hash_method, NULL);
    Py_DECREF(hash_method);
    if (hash_value == NULL) {
        return (Py_hash_t)-1;
    }
    out = PyLong_AsLongLong(hash_value);
    Py_DECREF(hash_value);
    if (molt_err_pending() != 0) {
        return (Py_hash_t)-1;
    }
    return (Py_hash_t)out;
}

static inline PyObject *PyObject_Str(PyObject *obj) {
    return _molt_pyobject_from_result(molt_object_str(_molt_py_handle(obj)));
}

static inline PyObject *PyObject_Repr(PyObject *obj) {
    return _molt_pyobject_from_result(molt_object_repr(_molt_py_handle(obj)));
}

static inline PyObject *PyObject_Format(PyObject *obj, PyObject *format_spec) {
    PyObject *format_fn;
    PyObject *args;
    PyObject *out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    if (format_spec == NULL) {
        format_spec = PyUnicode_FromString("");
        if (format_spec == NULL) {
            return NULL;
        }
        out = PyObject_Format(obj, format_spec);
        Py_DECREF(format_spec);
        return out;
    }
    format_fn = PyObject_GetAttrString(obj, "__format__");
    if (format_fn == NULL) {
        return NULL;
    }
    args = PyTuple_New(1);
    if (args == NULL) {
        Py_DECREF(format_fn);
        return NULL;
    }
    Py_INCREF(format_spec);
    if (PyTuple_SetItem(args, 0, format_spec) < 0) {
        Py_DECREF(format_fn);
        Py_DECREF(args);
        return NULL;
    }
    out = PyObject_CallObject(format_fn, args);
    Py_DECREF(format_fn);
    Py_DECREF(args);
    return out;
}

static inline PyObject *PyObject_Dir(PyObject *obj) {
    PyObject *dir_fn;
    PyObject *out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    dir_fn = PyObject_GetAttrString(obj, "__dir__");
    if (dir_fn == NULL) {
        return NULL;
    }
    out = PyObject_CallObject(dir_fn, NULL);
    Py_DECREF(dir_fn);
    return out;
}

static inline int PyObject_Print(PyObject *obj, FILE *fp, int flags) {
    PyObject *text_obj;
    const char *text;
    (void)flags;
    text_obj = PyObject_Str(obj);
    if (text_obj == NULL) {
        return -1;
    }
    text = PyUnicode_AsUTF8(text_obj);
    if (text == NULL) {
        Py_DECREF(text_obj);
        return -1;
    }
    if (fputs(text, fp != NULL ? fp : stdout) < 0) {
        Py_DECREF(text_obj);
        PyErr_SetString(PyExc_RuntimeError, "failed to write object text");
        return -1;
    }
    Py_DECREF(text_obj);
    return 0;
}

static inline int PyType_Ready(PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return -1;
    }
    return molt_type_ready(_molt_py_handle((PyObject *)type));
}

static inline int _molt_dict_set_utf8_key(
    MoltHandle dict_bits,
    const char *key,
    MoltHandle value_bits) {
    MoltHandle key_bits;
    int rc;
    if (key == NULL) {
        PyErr_SetString(PyExc_TypeError, "dict key must not be NULL");
        return -1;
    }
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0 || molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_mapping_setitem(dict_bits, key_bits, value_bits);
    molt_handle_decref(key_bits);
    return rc;
}

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

static inline PyObject *_molt_type_wrap_single_arg_builtin(
    const char *wrapper_name,
    PyObject *callable_obj) {
    PyObject *wrapper_type;
    MoltHandle arg_bits;
    MoltHandle args_tuple_bits;
    PyObject *args_tuple_obj;
    PyObject *wrapped;
    wrapper_type = _molt_builtin_class_lookup_utf8(wrapper_name);
    if (wrapper_type == NULL) {
        return NULL;
    }
    arg_bits = _molt_py_handle(callable_obj);
    args_tuple_bits = molt_tuple_from_array(&arg_bits, 1);
    if (args_tuple_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(wrapper_type);
        return NULL;
    }
    args_tuple_obj = _molt_pyobject_from_handle(args_tuple_bits);
    wrapped = PyObject_CallObject(wrapper_type, args_tuple_obj);
    molt_handle_decref(args_tuple_bits);
    Py_DECREF(wrapper_type);
    return wrapped;
}

static inline PyObject *_molt_type_make_slot_callable(
    MoltHandle self_bits,
    const char *name,
    uintptr_t method_ptr,
    uint32_t call_flags,
    const char *doc) {
    uint64_t name_len;
    uint64_t doc_len = 0;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_TypeError, "slot callable name must not be empty");
        return NULL;
    }
    if (method_ptr == 0) {
        PyErr_SetString(PyExc_TypeError, "slot function pointer must not be NULL");
        return NULL;
    }
    name_len = (uint64_t)strlen(name);
    if (doc != NULL) {
        doc_len = (uint64_t)strlen(doc);
    }
    return _molt_pyobject_from_result(molt_cfunction_create_bytes(
        self_bits,
        (const uint8_t *)name,
        name_len,
        method_ptr,
        call_flags,
        (const uint8_t *)doc,
        doc_len));
}

static inline int _molt_type_maybe_set_slot_callable(
    PyObject *type_obj,
    const char *slot_attr,
    uintptr_t method_ptr,
    uint32_t call_flags) {
    int has_attr;
    PyObject *callable_obj;
    if (method_ptr == 0) {
        return 0;
    }
    has_attr = PyObject_HasAttrString(type_obj, slot_attr);
    if (molt_err_pending() != 0) {
        return -1;
    }
    if (has_attr != 0) {
        return 0;
    }
    callable_obj = _molt_type_make_slot_callable(
        molt_none(), slot_attr, method_ptr, call_flags, NULL);
    if (callable_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, slot_attr, callable_obj) < 0) {
        Py_DECREF(callable_obj);
        return -1;
    }
    Py_DECREF(callable_obj);
    return 0;
}

static inline int _molt_type_add_getset(PyObject *type_obj, PyGetSetDef *getset) {
    PyGetSetDef *entry;
    if (getset == NULL) {
        return 0;
    }
    for (entry = getset; entry->name != NULL; entry++) {
        PyObject *getter_callable;
        PyObject *property_obj;
        if (entry->name[0] == '\0') {
            PyErr_SetString(PyExc_TypeError, "getset name must not be empty");
            return -1;
        }
        if (entry->get == NULL) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported getset '%s': getter callback is required",
                entry->name);
            return -1;
        }
        if (entry->set != NULL) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported getset '%s': setter callbacks are not yet implemented",
                entry->name);
            return -1;
        }
        if (entry->closure != NULL) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported getset '%s': non-NULL closure is not yet implemented",
                entry->name);
            return -1;
        }
        getter_callable = _molt_type_make_slot_callable(
            molt_none(),
            entry->name,
            (uintptr_t)entry->get,
            (uint32_t)METH_NOARGS,
            entry->doc);
        if (getter_callable == NULL) {
            return -1;
        }
        property_obj = _molt_type_wrap_single_arg_builtin("property", getter_callable);
        Py_DECREF(getter_callable);
        if (property_obj == NULL) {
            return -1;
        }
        if (PyObject_SetAttrString(type_obj, entry->name, property_obj) < 0) {
            Py_DECREF(property_obj);
            return -1;
        }
        Py_DECREF(property_obj);
    }
    return 0;
}

static inline int _molt_type_attach_module(PyObject *type_obj, PyObject *module) {
    return PyObject_SetAttrString(type_obj, _MOLT_TYPE_MODULE_ATTR, module);
}

static inline PyObject *_molt_type_get_attached_module_borrowed(
    PyTypeObject *type,
    int suppress_missing_error) {
    PyObject *module;
    if (type == NULL) {
        if (!suppress_missing_error) {
            PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        }
        return NULL;
    }
    module = PyObject_GetAttrString((PyObject *)type, _MOLT_TYPE_MODULE_ATTR);
    if (module == NULL) {
        if (molt_err_pending() != 0) {
            (void)molt_err_clear();
        }
        if (!suppress_missing_error) {
            PyErr_SetString(PyExc_TypeError, "type has no associated module");
        }
        return NULL;
    }
    if (_molt_py_handle(module) == molt_none()) {
        Py_DECREF(module);
        if (!suppress_missing_error) {
            PyErr_SetString(PyExc_TypeError, "type has no associated module");
        }
        return NULL;
    }
    /*
     * Expose borrowed-ref semantics to mirror CPython's type/module APIs.
     * The strong reference is held by the type attribute itself.
     */
    Py_DECREF(module);
    return module;
}

static inline int _molt_type_add_methods(PyObject *type_obj, PyMethodDef *methods) {
    PyMethodDef *entry;
    if (type_obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return -1;
    }
    if (methods == NULL) {
        return 0;
    }
    for (entry = methods; entry->ml_name != NULL; entry++) {
        unsigned int raw_flags;
        unsigned int call_flags;
        unsigned int modifier_flags;
        MoltHandle callback_self_bits;
        PyObject *callable_obj;
        if (entry->ml_meth == NULL) {
            PyErr_Format(
                PyExc_TypeError,
                "method '%s' has NULL function pointer",
                entry->ml_name);
            return -1;
        }
        raw_flags = (unsigned int)entry->ml_flags;
        call_flags = raw_flags & _MOLT_METH_CALL_MASK;
        modifier_flags = raw_flags & _MOLT_METH_MODIFIER_MASK;
        if ((raw_flags & ~(_MOLT_METH_CALL_MASK | _MOLT_METH_MODIFIER_MASK)) != 0) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported method flags for '%s' (unsupported modifier bits set)",
                entry->ml_name);
            return -1;
        }
        if (call_flags == 0) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported method flags for '%s' (missing call signature)",
                entry->ml_name);
            return -1;
        }
        if ((modifier_flags & METH_CLASS) != 0 && (modifier_flags & METH_STATIC) != 0) {
            PyErr_Format(
                PyExc_RuntimeError,
                "unsupported method flags for '%s' (METH_CLASS and METH_STATIC are mutually exclusive)",
                entry->ml_name);
            return -1;
        }
        callback_self_bits = (modifier_flags & METH_STATIC) != 0 ? 0 : molt_none();
        callable_obj = _molt_type_make_slot_callable(
            callback_self_bits,
            entry->ml_name,
            (uintptr_t)entry->ml_meth,
            (uint32_t)call_flags,
            entry->ml_doc);
        if (callable_obj == NULL) {
            return -1;
        }
        if ((modifier_flags & METH_CLASS) != 0) {
            PyObject *wrapped = _molt_type_wrap_single_arg_builtin("classmethod", callable_obj);
            Py_DECREF(callable_obj);
            if (wrapped == NULL) {
                return -1;
            }
            callable_obj = wrapped;
        } else if ((modifier_flags & METH_STATIC) != 0) {
            PyObject *wrapped = _molt_type_wrap_single_arg_builtin("staticmethod", callable_obj);
            Py_DECREF(callable_obj);
            if (wrapped == NULL) {
                return -1;
            }
            callable_obj = wrapped;
        }
        if (PyObject_SetAttrString(type_obj, entry->ml_name, callable_obj) < 0) {
            Py_DECREF(callable_obj);
            return -1;
        }
        Py_DECREF(callable_obj);
    }
    return 0;
}

static inline PyObject *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases) {
    PyType_Slot *slot;
    PyMethodDef *type_methods = NULL;
    PyGetSetDef *type_getset = NULL;
    PyMemberDef *type_members = NULL;
    PyObject *slot_base = NULL;
    PyObject *slot_bases = NULL;
    const char *type_doc = NULL;
    void *type_new = NULL;
    uintptr_t slot_tp_call = 0;
    uintptr_t slot_tp_iter = 0;
    uintptr_t slot_tp_iternext = 0;
    uintptr_t slot_tp_repr = 0;
    uintptr_t slot_tp_str = 0;
    uintptr_t slot_nb_add = 0;
    uintptr_t slot_nb_subtract = 0;
    uintptr_t slot_nb_multiply = 0;
    uintptr_t slot_sq_concat = 0;
    int saw_methods = 0;
    int saw_getset = 0;
    int saw_members = 0;
    int saw_base = 0;
    int saw_bases = 0;
    int saw_doc = 0;
    int saw_new = 0;
    int saw_tp_call = 0;
    int saw_tp_iter = 0;
    int saw_tp_iternext = 0;
    int saw_tp_repr = 0;
    int saw_tp_str = 0;
    int saw_nb_add = 0;
    int saw_nb_subtract = 0;
    int saw_nb_multiply = 0;
    int saw_sq_concat = 0;
    PyObject *name_obj = NULL;
    PyObject *namespace_obj = NULL;
    PyObject *type_obj = NULL;
    PyObject *type_callable = NULL;
    PyObject *owned_bases = NULL;
    PyObject *resolved_bases = NULL;
    const char *full_name;
    const char *last_dot;
    const char *type_name;

    if (spec == NULL || spec->name == NULL || spec->name[0] == '\0') {
        PyErr_SetString(PyExc_TypeError, "type spec name must not be empty");
        return NULL;
    }
    full_name = spec->name;
    last_dot = strrchr(full_name, '.');
    type_name = (last_dot != NULL && last_dot[1] != '\0') ? last_dot + 1 : full_name;

    if (spec->slots != NULL) {
        for (slot = spec->slots; slot->slot != 0; slot++) {
            switch (slot->slot) {
            case Py_tp_doc:
                if (saw_doc) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_doc slot");
                    return NULL;
                }
                saw_doc = 1;
                type_doc = (const char *)slot->pfunc;
                break;
            case Py_tp_methods:
                if (saw_methods) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_methods slot");
                    return NULL;
                }
                saw_methods = 1;
                type_methods = (PyMethodDef *)slot->pfunc;
                break;
            case Py_tp_base:
                if (saw_base) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_base slot");
                    return NULL;
                }
                saw_base = 1;
                slot_base = (PyObject *)slot->pfunc;
                break;
            case Py_tp_bases:
                if (saw_bases) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_bases slot");
                    return NULL;
                }
                saw_bases = 1;
                slot_bases = (PyObject *)slot->pfunc;
                break;
            case Py_tp_new:
                if (saw_new) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_new slot");
                    return NULL;
                }
                saw_new = 1;
                type_new = slot->pfunc;
                break;
            case Py_tp_getset:
                if (saw_getset) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_getset slot");
                    return NULL;
                }
                saw_getset = 1;
                type_getset = (PyGetSetDef *)slot->pfunc;
                break;
            case Py_tp_members:
                if (saw_members) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_members slot");
                    return NULL;
                }
                saw_members = 1;
                type_members = (PyMemberDef *)slot->pfunc;
                break;
            case Py_tp_call:
                if (saw_tp_call) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_call slot");
                    return NULL;
                }
                saw_tp_call = 1;
                slot_tp_call = (uintptr_t)slot->pfunc;
                break;
            case Py_tp_iter:
                if (saw_tp_iter) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_iter slot");
                    return NULL;
                }
                saw_tp_iter = 1;
                slot_tp_iter = (uintptr_t)slot->pfunc;
                break;
            case Py_tp_iternext:
                if (saw_tp_iternext) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_iternext slot");
                    return NULL;
                }
                saw_tp_iternext = 1;
                slot_tp_iternext = (uintptr_t)slot->pfunc;
                break;
            case Py_tp_repr:
                if (saw_tp_repr) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_repr slot");
                    return NULL;
                }
                saw_tp_repr = 1;
                slot_tp_repr = (uintptr_t)slot->pfunc;
                break;
            case Py_tp_str:
                if (saw_tp_str) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_tp_str slot");
                    return NULL;
                }
                saw_tp_str = 1;
                slot_tp_str = (uintptr_t)slot->pfunc;
                break;
            case Py_nb_add:
                if (saw_nb_add) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_nb_add slot");
                    return NULL;
                }
                saw_nb_add = 1;
                slot_nb_add = (uintptr_t)slot->pfunc;
                break;
            case Py_nb_subtract:
                if (saw_nb_subtract) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_nb_subtract slot");
                    return NULL;
                }
                saw_nb_subtract = 1;
                slot_nb_subtract = (uintptr_t)slot->pfunc;
                break;
            case Py_nb_multiply:
                if (saw_nb_multiply) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_nb_multiply slot");
                    return NULL;
                }
                saw_nb_multiply = 1;
                slot_nb_multiply = (uintptr_t)slot->pfunc;
                break;
            case Py_sq_concat:
                if (saw_sq_concat) {
                    PyErr_SetString(PyExc_TypeError, "duplicate Py_sq_concat slot");
                    return NULL;
                }
                saw_sq_concat = 1;
                slot_sq_concat = (uintptr_t)slot->pfunc;
                break;
            default:
                PyErr_Format(
                    PyExc_RuntimeError,
                    "unsupported PyType_Spec slot %d for %s",
                    slot->slot,
                    full_name);
                return NULL;
            }
        }
    }

    name_obj = _molt_pyobject_from_result(_molt_string_from_utf8(type_name));
    if (name_obj == NULL) {
        return NULL;
    }
    namespace_obj = _molt_pyobject_from_result(molt_dict_from_pairs(NULL, NULL, 0));
    if (namespace_obj == NULL) {
        Py_DECREF(name_obj);
        return NULL;
    }
    if (type_doc != NULL) {
        MoltHandle doc_bits = _molt_string_from_utf8(type_doc);
        if (doc_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(namespace_obj);
            Py_DECREF(name_obj);
            return NULL;
        }
        if (_molt_dict_set_utf8_key(_molt_py_handle(namespace_obj), "__doc__", doc_bits) < 0) {
            molt_handle_decref(doc_bits);
            Py_DECREF(namespace_obj);
            Py_DECREF(name_obj);
            return NULL;
        }
        molt_handle_decref(doc_bits);
    }
    if (last_dot != NULL && last_dot != full_name) {
        uint64_t module_len = (uint64_t)(last_dot - full_name);
        MoltHandle module_bits = molt_string_from((const uint8_t *)full_name, module_len);
        if (module_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(namespace_obj);
            Py_DECREF(name_obj);
            return NULL;
        }
        if (_molt_dict_set_utf8_key(_molt_py_handle(namespace_obj), "__module__", module_bits) < 0) {
            molt_handle_decref(module_bits);
            Py_DECREF(namespace_obj);
            Py_DECREF(name_obj);
            return NULL;
        }
        molt_handle_decref(module_bits);
    }
    resolved_bases = bases;
    if (resolved_bases == NULL) {
        if (slot_bases != NULL) {
            resolved_bases = slot_bases;
        } else if (slot_base != NULL) {
            MoltHandle base_bits = _molt_py_handle(slot_base);
            owned_bases = _molt_pyobject_from_result(molt_tuple_from_array(&base_bits, 1));
            if (owned_bases == NULL) {
                Py_DECREF(namespace_obj);
                Py_DECREF(name_obj);
                return NULL;
            }
            resolved_bases = owned_bases;
        } else {
            PyObject *object_type = _molt_builtin_class_lookup_utf8("object");
            MoltHandle base_bits;
            if (object_type == NULL) {
                Py_DECREF(namespace_obj);
                Py_DECREF(name_obj);
                return NULL;
            }
            base_bits = _molt_py_handle(object_type);
            owned_bases = _molt_pyobject_from_result(molt_tuple_from_array(&base_bits, 1));
            Py_DECREF(object_type);
            if (owned_bases == NULL) {
                Py_DECREF(namespace_obj);
                Py_DECREF(name_obj);
                return NULL;
            }
            resolved_bases = owned_bases;
        }
    }

    type_callable = _molt_builtin_class_lookup_utf8("type");
    if (type_callable == NULL) {
        Py_XDECREF(owned_bases);
        Py_DECREF(namespace_obj);
        Py_DECREF(name_obj);
        return NULL;
    }
    {
        MoltHandle call_args_values[3];
        MoltHandle call_args_bits;
        call_args_values[0] = _molt_py_handle(name_obj);
        call_args_values[1] = _molt_py_handle(resolved_bases);
        call_args_values[2] = _molt_py_handle(namespace_obj);
        call_args_bits = molt_tuple_from_array(call_args_values, 3);
        if (call_args_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(type_callable);
            Py_XDECREF(owned_bases);
            Py_DECREF(namespace_obj);
            Py_DECREF(name_obj);
            return NULL;
        }
        type_obj = _molt_pyobject_from_result(
            molt_object_call(_molt_py_handle(type_callable), call_args_bits, molt_none()));
        molt_handle_decref(call_args_bits);
    }
    Py_DECREF(type_callable);
    Py_XDECREF(owned_bases);
    Py_DECREF(namespace_obj);
    Py_DECREF(name_obj);
    if (type_obj == NULL) {
        return NULL;
    }
    if (type_methods != NULL && _molt_type_add_methods(type_obj, type_methods) < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (type_getset != NULL && _molt_type_add_getset(type_obj, type_getset) < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (type_members != NULL && type_members->name != NULL) {
        PyErr_SetString(
            PyExc_RuntimeError,
            "Py_tp_members is not yet implemented in the libmolt compatibility header");
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj,
            "__call__",
            slot_tp_call,
            (uint32_t)(METH_VARARGS | METH_KEYWORDS))
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__iter__", slot_tp_iter, (uint32_t)METH_NOARGS)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__next__", slot_tp_iternext, (uint32_t)METH_NOARGS)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__repr__", slot_tp_repr, (uint32_t)METH_NOARGS)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__str__", slot_tp_str, (uint32_t)METH_NOARGS)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__add__", slot_nb_add, (uint32_t)METH_O)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__sub__", slot_nb_subtract, (uint32_t)METH_O)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__mul__", slot_nb_multiply, (uint32_t)METH_O)
        < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (slot_nb_add == 0
        && _molt_type_maybe_set_slot_callable(
               type_obj, "__add__", slot_sq_concat, (uint32_t)METH_O)
            < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    if (type_new != NULL) {
        PyObject *new_obj = _molt_type_make_slot_callable(
            molt_none(),
            "__new__",
            (uintptr_t)type_new,
            (uint32_t)(METH_VARARGS | METH_KEYWORDS),
            NULL);
        if (new_obj == NULL) {
            Py_DECREF(type_obj);
            return NULL;
        }
        if (PyObject_SetAttrString(type_obj, "__new__", new_obj) < 0) {
            Py_DECREF(new_obj);
            Py_DECREF(type_obj);
            return NULL;
        }
        Py_DECREF(new_obj);
    }
    if (PyType_Ready((PyTypeObject *)type_obj) < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    return type_obj;
}

static inline PyObject *PyType_FromSpec(PyType_Spec *spec) {
    return PyType_FromSpecWithBases(spec, NULL);
}

static inline PyObject *PyType_FromModuleAndSpec(
    PyObject *module,
    PyType_Spec *spec,
    PyObject *bases) {
    PyObject *type_obj;
    if (module != NULL) {
        MoltHandle module_dict_bits = molt_module_get_dict(_molt_py_handle(module));
        if (module_dict_bits == 0 || molt_err_pending() != 0) {
            if (molt_err_pending() == 0) {
                PyErr_SetString(PyExc_TypeError, "module must be a module object or NULL");
            }
            return NULL;
        }
        molt_handle_decref(module_dict_bits);
    }
    type_obj = PyType_FromSpecWithBases(spec, bases);
    if (type_obj == NULL) {
        return NULL;
    }
    if (module != NULL && _molt_type_attach_module(type_obj, module) < 0) {
        Py_DECREF(type_obj);
        return NULL;
    }
    return type_obj;
}

static inline PyObject *PyType_GetModule(PyTypeObject *type) {
    return _molt_type_get_attached_module_borrowed(type, 0);
}

static inline void *PyType_GetModuleState(PyTypeObject *type) {
    PyObject *module = PyType_GetModule(type);
    if (module == NULL) {
        return NULL;
    }
    return PyModule_GetState(module);
}

static inline PyObject *PyType_GetModuleByDef(PyTypeObject *type, PyModuleDef *def) {
    MoltHandle mro_bits;
    int64_t mro_len;
    int64_t i;
    if (type == NULL || def == NULL) {
        PyErr_SetString(PyExc_TypeError, "type/module definition must not be NULL");
        return NULL;
    }
    mro_bits = molt_object_getattr_bytes(
        _molt_py_handle((PyObject *)type), (const uint8_t *)"__mro__", (uint64_t)7);
    if (mro_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    mro_len = molt_sequence_length(mro_bits);
    if (mro_len < 0) {
        molt_handle_decref(mro_bits);
        return NULL;
    }
    for (i = 0; i < mro_len; i++) {
        MoltHandle index_bits = molt_int_from_i64(i);
        MoltHandle base_bits;
        PyObject *module;
        PyModuleDef *candidate_def;
        if (index_bits == 0 || molt_err_pending() != 0) {
            molt_handle_decref(mro_bits);
            return NULL;
        }
        base_bits = molt_sequence_getitem(mro_bits, index_bits);
        molt_handle_decref(index_bits);
        if (base_bits == 0 || molt_err_pending() != 0) {
            molt_handle_decref(mro_bits);
            return NULL;
        }
        module = _molt_type_get_attached_module_borrowed((PyTypeObject *)_molt_pyobject_from_handle(base_bits), 1);
        if (module != NULL) {
            candidate_def = PyModule_GetDef(module);
            if (candidate_def == def) {
                molt_handle_decref(base_bits);
                molt_handle_decref(mro_bits);
                return module;
            }
            if (candidate_def == NULL && molt_err_pending() != 0) {
                molt_handle_decref(base_bits);
                molt_handle_decref(mro_bits);
                return NULL;
            }
        }
        molt_handle_decref(base_bits);
    }
    molt_handle_decref(mro_bits);
    PyErr_SetString(PyExc_TypeError, "type has no associated module for the given definition");
    return NULL;
}

static inline PyObject *PyType_GenericNew(PyTypeObject *subtype, PyObject *args, PyObject *kwargs) {
    if (subtype == NULL) {
        PyErr_SetString(PyExc_TypeError, "subtype must not be NULL");
        return NULL;
    }
    return PyObject_Call((PyObject *)subtype, args, kwargs);
}

static inline unsigned long PyType_GetFlags(PyTypeObject *type) {
    if (type == NULL) {
        return 0UL;
    }
    return type->tp_flags;
}

static inline int _molt_module_attach_definition(PyObject *module, PyModuleDef *def) {
    uint64_t state_size = 0;
    if (def == NULL) {
        return 0;
    }
    if (def->m_size > 0) {
        state_size = (uint64_t)def->m_size;
    }
    if (molt_module_capi_register(_molt_py_handle(module), (uintptr_t)def, state_size) < 0) {
        return -1;
    }
    if (def->m_doc != NULL) {
        MoltHandle doc_bits = _molt_string_from_utf8(def->m_doc);
        if (doc_bits == 0 || molt_err_pending() != 0) {
            return -1;
        }
        if (molt_object_setattr_bytes(
                _molt_py_handle(module),
                (const uint8_t *)"__doc__",
                (uint64_t)7,
                doc_bits)
            < 0) {
            molt_handle_decref(doc_bits);
            return -1;
        }
        molt_handle_decref(doc_bits);
    }
    return 0;
}

static inline PyObject *PyModule_NewObject(PyObject *name) {
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module name object must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_module_create(_molt_py_handle(name)));
}

static inline PyObject *PyModule_New(const char *name) {
    MoltHandle name_bits;
    MoltHandle module_bits;
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module name must not be NULL");
        return NULL;
    }
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    module_bits = molt_module_create(name_bits);
    molt_handle_decref(name_bits);
    return _molt_pyobject_from_result(module_bits);
}

static inline int PyModule_Check(PyObject *obj) {
    int has_dict;
    int has_name;
    if (obj == NULL) {
        return 0;
    }
    has_dict = PyObject_HasAttrString(obj, "__dict__");
    if (has_dict <= 0) {
        PyErr_Clear();
        return 0;
    }
    has_name = PyObject_HasAttrString(obj, "__name__");
    if (has_name <= 0) {
        PyErr_Clear();
        return 0;
    }
    return 1;
}

static inline PyObject *Py_InitModule(const char *name, PyMethodDef *methods) {
    return Py_InitModule4(name, methods, NULL, NULL, PYTHON_API_VERSION);
}

static inline PyObject *Py_InitModule4(
    const char *name,
    PyMethodDef *methods,
    const char *doc,
    PyObject *self,
    int apiver
) {
    PyObject *module;
    (void)self;
    (void)apiver;
    module = PyModule_New(name);
    if (module == NULL) {
        return NULL;
    }
    if (doc != NULL && PyModule_SetDocString(module, doc) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    if (methods != NULL && PyModule_AddFunctions(module, methods) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    return module;
}

static inline PyObject *PyModule_Create2(PyModuleDef *def, int api_version) {
    MoltHandle name_bits;
    MoltHandle module_bits;
    PyObject *module;
    (void)api_version;
    if (def == NULL || def->m_name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module definition name must not be NULL");
        return NULL;
    }
    name_bits = _molt_string_from_utf8(def->m_name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    module_bits = molt_module_create(name_bits);
    molt_handle_decref(name_bits);
    module = _molt_pyobject_from_result(module_bits);
    if (module == NULL) {
        return NULL;
    }
    if (_molt_module_attach_definition(module, def) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    if (PyModule_AddFunctions(module, def->m_methods) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    if (PyState_AddModule(module, def) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    return module;
}

#define PyModule_Create(def) PyModule_Create2((def), PYTHON_API_VERSION)

static inline PyObject *PyModuleDef_Init(PyModuleDef *def) {
    if (def == NULL) {
        PyErr_SetString(PyExc_TypeError, "module definition must not be NULL");
        return NULL;
    }
    return (PyObject *)def;
}

static inline PyObject *PyModule_GetDict(PyObject *module) {
    return _molt_pyobject_from_result(molt_module_get_dict(_molt_py_handle(module)));
}

static inline int PyModule_AddObjectRef(PyObject *module, const char *name, PyObject *value) {
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module attribute name must not be NULL");
        return -1;
    }
    return molt_module_add_object_bytes(
        _molt_py_handle(module), (const uint8_t *)name, (uint64_t)strlen(name), _molt_py_handle(value));
}

static inline int PyModule_AddObject(PyObject *module, const char *name, PyObject *value) {
    int rc = PyModule_AddObjectRef(module, name, value);
    if (rc == 0 && value != NULL) {
        Py_DECREF(value);
    }
    return rc;
}

static inline int PyModule_Add(PyObject *module, const char *name, PyObject *value) {
    return PyModule_AddObject(module, name, value);
}

static inline int PyModule_AddType(PyObject *module, PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "module type must not be NULL");
        return -1;
    }
    return molt_module_add_type(
        _molt_py_handle(module), _molt_py_handle((PyObject *)type));
}

static inline PyObject *PyModule_GetObject(PyObject *module, const char *name) {
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module attribute name must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_module_get_object_bytes(
        _molt_py_handle(module), (const uint8_t *)name, (uint64_t)strlen(name)));
}

static inline PyObject *PyModule_GetNameObject(PyObject *module) {
    return PyModule_GetObject(module, "__name__");
}

static inline PyModuleDef *PyModule_GetDef(PyObject *module) {
    if (module == NULL) {
        PyErr_SetString(PyExc_TypeError, "module must not be NULL");
        return NULL;
    }
    return (PyModuleDef *)molt_module_capi_get_def(_molt_py_handle(module));
}

static inline void *PyModule_GetState(PyObject *module) {
    if (module == NULL) {
        PyErr_SetString(PyExc_TypeError, "module must not be NULL");
        return NULL;
    }
    return (void *)molt_module_capi_get_state(_molt_py_handle(module));
}

static inline int PyModule_SetDocString(PyObject *module, const char *docstring) {
    MoltHandle doc_bits;
    if (module == NULL) {
        PyErr_SetString(PyExc_TypeError, "module must not be NULL");
        return -1;
    }
    if (docstring == NULL) {
        return molt_object_setattr_bytes(
            _molt_py_handle(module), (const uint8_t *)"__doc__", (uint64_t)7, molt_none());
    }
    doc_bits = _molt_string_from_utf8(docstring);
    if (doc_bits == 0 || molt_err_pending() != 0) {
        return -1;
    }
    if (molt_object_setattr_bytes(
            _molt_py_handle(module), (const uint8_t *)"__doc__", (uint64_t)7, doc_bits)
        < 0) {
        molt_handle_decref(doc_bits);
        return -1;
    }
    molt_handle_decref(doc_bits);
    return 0;
}

static inline PyObject *PyModule_GetFilenameObject(PyObject *module) {
    PyObject *filename_obj = PyModule_GetObject(module, "__file__");
    if (filename_obj == NULL) {
        if (molt_err_pending() != 0) {
            (void)molt_err_clear();
        }
        PyErr_SetString(PyExc_RuntimeError, "module has no valid __file__");
        return NULL;
    }
    if (PyUnicode_AsUTF8AndSize(filename_obj, NULL) == NULL) {
        if (molt_err_pending() != 0) {
            (void)molt_err_clear();
        }
        Py_DECREF(filename_obj);
        PyErr_SetString(PyExc_RuntimeError, "module __file__ must be str");
        return NULL;
    }
    return filename_obj;
}

static inline const char *PyModule_GetName(PyObject *module) {
    static _Thread_local char *name_buf = NULL;
    static _Thread_local size_t name_cap = 0;
    PyObject *name_obj = PyModule_GetNameObject(module);
    const char *raw;
    Py_ssize_t len = 0;
    if (name_obj == NULL) {
        return NULL;
    }
    raw = PyUnicode_AsUTF8AndSize(name_obj, &len);
    if (raw == NULL) {
        Py_DECREF(name_obj);
        return NULL;
    }
    if ((size_t)len + 1 > name_cap) {
        char *next = (char *)realloc(name_buf, (size_t)len + 1);
        if (next == NULL) {
            Py_DECREF(name_obj);
            PyErr_SetString(PyExc_RuntimeError, "out of memory");
            return NULL;
        }
        name_buf = next;
        name_cap = (size_t)len + 1;
    }
    memcpy(name_buf, raw, (size_t)len);
    name_buf[(size_t)len] = '\0';
    Py_DECREF(name_obj);
    return name_buf;
}

static inline const char *PyModule_GetFilename(PyObject *module) {
    static _Thread_local char *filename_buf = NULL;
    static _Thread_local size_t filename_cap = 0;
    PyObject *filename_obj = PyModule_GetFilenameObject(module);
    const char *raw;
    Py_ssize_t len = 0;
    if (filename_obj == NULL) {
        return NULL;
    }
    raw = PyUnicode_AsUTF8AndSize(filename_obj, &len);
    if (raw == NULL) {
        Py_DECREF(filename_obj);
        return NULL;
    }
    if ((size_t)len + 1 > filename_cap) {
        char *next = (char *)realloc(filename_buf, (size_t)len + 1);
        if (next == NULL) {
            Py_DECREF(filename_obj);
            PyErr_SetString(PyExc_RuntimeError, "out of memory");
            return NULL;
        }
        filename_buf = next;
        filename_cap = (size_t)len + 1;
    }
    memcpy(filename_buf, raw, (size_t)len);
    filename_buf[(size_t)len] = '\0';
    Py_DECREF(filename_obj);
    return filename_buf;
}

static inline int PyModule_AddFunctions(PyObject *module, PyMethodDef *functions) {
    PyMethodDef *entry;
    if (module == NULL) {
        PyErr_SetString(PyExc_TypeError, "module must not be NULL");
        return -1;
    }
    if (functions == NULL) {
        return 0;
    }
    for (entry = functions; entry->ml_name != NULL; entry++) {
        if (entry->ml_meth == NULL) {
            PyErr_Format(PyExc_TypeError, "method '%s' has NULL function pointer", entry->ml_name);
            return -1;
        }
        if (molt_module_add_cfunction_bytes(
                _molt_py_handle(module),
                (const uint8_t *)entry->ml_name,
                (uint64_t)strlen(entry->ml_name),
                (uintptr_t)entry->ml_meth,
                (uint32_t)entry->ml_flags,
                (const uint8_t *)entry->ml_doc,
                entry->ml_doc != NULL ? (uint64_t)strlen(entry->ml_doc) : 0)
            < 0) {
            return -1;
        }
    }
    return 0;
}

static inline int PyState_AddModule(PyObject *module, PyModuleDef *def) {
    if (module == NULL || def == NULL) {
        PyErr_SetString(PyExc_TypeError, "module/definition must not be NULL");
        return -1;
    }
    return molt_module_state_add(_molt_py_handle(module), (uintptr_t)def);
}

static inline PyObject *PyState_FindModule(PyModuleDef *def) {
    MoltHandle bits;
    if (def == NULL) {
        PyErr_SetString(PyExc_TypeError, "module definition must not be NULL");
        return NULL;
    }
    bits = molt_module_state_find((uintptr_t)def);
    if (bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(bits);
}

static inline int PyState_RemoveModule(PyModuleDef *def) {
    if (def == NULL) {
        PyErr_SetString(PyExc_TypeError, "module definition must not be NULL");
        return -1;
    }
    if (molt_module_state_remove((uintptr_t)def) < 0) {
        if (molt_err_pending() == 0) {
            PyErr_SetString(PyExc_RuntimeError, "module definition was not registered");
        }
        return -1;
    }
    return 0;
}

static inline PyObject *PyModule_FromDefAndSpec2(PyModuleDef *def, PyObject *spec, int module_api_version) {
    PyObject *module;
    PyObject *name_obj;
    (void)module_api_version;
    if (def == NULL || spec == NULL) {
        PyErr_SetString(PyExc_TypeError, "module definition/spec must not be NULL");
        return NULL;
    }
    name_obj = PyObject_GetAttrString(spec, "name");
    if (name_obj == NULL) {
        if (molt_err_pending() != 0) {
            (void)molt_err_clear();
        }
        if (def->m_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "module spec missing name and definition has no name");
            return NULL;
        }
        module = PyModule_New(def->m_name);
    } else {
        module = PyModule_NewObject(name_obj);
        Py_DECREF(name_obj);
    }
    if (module == NULL) {
        return NULL;
    }
    if (_molt_module_attach_definition(module, def) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    if (PyObject_SetAttrString(module, "__spec__", spec) < 0) {
        Py_DECREF(module);
        return NULL;
    }
    return module;
}

static inline PyObject *PyModule_FromDefAndSpec(PyModuleDef *def, PyObject *spec) {
    return PyModule_FromDefAndSpec2(def, spec, PYTHON_API_VERSION);
}

static inline int PyModule_ExecDef(PyObject *module, PyModuleDef *def) {
    if (module == NULL || def == NULL) {
        PyErr_SetString(PyExc_TypeError, "module/definition must not be NULL");
        return -1;
    }
    if (_molt_module_attach_definition(module, def) < 0) {
        return -1;
    }
    if (def->m_doc != NULL && PyModule_SetDocString(module, def->m_doc) < 0) {
        return -1;
    }
    if (PyModule_AddFunctions(module, def->m_methods) < 0) {
        return -1;
    }
    return PyState_AddModule(module, def);
}

static inline int PyModule_AddIntConstant(PyObject *module, const char *name, long value) {
    MoltHandle name_bits;
    int rc;
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "module attribute name must not be NULL");
        return -1;
    }
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_module_add_int_constant(_molt_py_handle(module), name_bits, (int64_t)value);
    molt_handle_decref(name_bits);
    return rc;
}

static inline int PyModule_AddStringConstant(PyObject *module, const char *name, const char *value) {
    MoltHandle name_bits;
    int rc;
    if (name == NULL || value == NULL) {
        PyErr_SetString(PyExc_TypeError, "module constant name/value must not be NULL");
        return -1;
    }
    name_bits = _molt_string_from_utf8(name);
    if (name_bits == 0 || molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_module_add_string_constant(
        _molt_py_handle(module),
        name_bits,
        (const uint8_t *)value,
        (uint64_t)strlen(value));
    molt_handle_decref(name_bits);
    return rc;
}

static inline int PyUnstable_Module_SetGIL(PyObject *module, int gil_mode) {
    (void)module;
    (void)gil_mode;
    return 0;
}

static inline PyObject *PyLong_FromLong(long value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}
#define PyInt_FromLong PyLong_FromLong
#define PyInt PyLong_Type
#define PyInt_AsLong PyLong_AsLong
#define PyInt_Check PyLong_Check

static inline PyObject *PyBool_FromLong(long value) {
    return _molt_pyobject_from_result(molt_bool_from_i32(value != 0 ? 1 : 0));
}

static inline PyObject *PyLong_FromLongLong(long long value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline PyObject *PyLong_FromSsize_t(Py_ssize_t value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline PyObject *PyLong_FromUnsignedLongLong(unsigned long long value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline PyObject *PyLong_FromUnsignedLong(unsigned long value) {
    return PyLong_FromUnsignedLongLong((unsigned long long)value);
}

static inline PyObject *PyLong_FromVoidPtr(void *ptr) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)(intptr_t)ptr));
}

static inline long PyLong_AsLong(PyObject *obj) {
    return (long)molt_int_as_i64(_molt_py_handle(obj));
}

static inline int _PyLong_AsInt(PyObject *obj) {
    long value = PyLong_AsLong(obj);
    if (value == -1 && PyErr_Occurred() != NULL) {
        return -1;
    }
    if (value < INT_MIN || value > INT_MAX) {
        PyErr_SetString(PyExc_OverflowError, "Python int too large to convert to C int");
        return -1;
    }
    return (int)value;
}

static inline long PyLong_AsLongAndOverflow(PyObject *obj, int *overflow) {
    long long value = PyLong_AsLongLongAndOverflow(obj, overflow);
    if (overflow != NULL && *overflow != 0) {
        return (long)value;
    }
    if (value < LONG_MIN) {
        if (overflow != NULL) {
            *overflow = -1;
        }
        PyErr_SetString(PyExc_OverflowError, "Python int too large to convert to C long");
        return -1;
    }
    if (value > LONG_MAX) {
        if (overflow != NULL) {
            *overflow = 1;
        }
        PyErr_SetString(PyExc_OverflowError, "Python int too large to convert to C long");
        return -1;
    }
    return (long)value;
}

static inline long long PyLong_AsLongLong(PyObject *obj) {
    return (long long)molt_int_as_i64(_molt_py_handle(obj));
}

static inline long long PyLong_AsLongLongAndOverflow(PyObject *obj, int *overflow) {
    if (overflow != NULL) {
        *overflow = 0;
    }
    return PyLong_AsLongLong(obj);
}

static inline unsigned long long PyLong_AsUnsignedLongLong(PyObject *obj) {
    long long value = PyLong_AsLongLong(obj);
    if (molt_err_pending() != 0) {
        return (unsigned long long)-1;
    }
    if (value < 0) {
        PyErr_SetString(PyExc_OverflowError, "can't convert negative value to unsigned int");
        return (unsigned long long)-1;
    }
    return (unsigned long long)value;
}

static inline unsigned long PyLong_AsUnsignedLong(PyObject *obj) {
    unsigned long long value = PyLong_AsUnsignedLongLong(obj);
    if (molt_err_pending() != 0) {
        return (unsigned long)-1;
    }
    if (value > ULONG_MAX) {
        PyErr_SetString(PyExc_OverflowError, "Python int too large to convert to C unsigned long");
        return (unsigned long)-1;
    }
    return (unsigned long)value;
}

static inline unsigned long long PyLong_AsUnsignedLongLongMask(PyObject *obj) {
    long long value = PyLong_AsLongLong(obj);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0ULL;
    }
    return (unsigned long long)value;
}

static inline PyObject *PyLong_FromUnicodeObject(PyObject *obj, int base) {
    PyTypeObject *int_type;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "unicode object must not be NULL");
        return NULL;
    }
    if (!PyUnicode_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, "expected unicode object");
        return NULL;
    }
    int_type = _molt_builtin_type_object_borrowed("int");
    if (int_type == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "int constructor is unavailable");
        return NULL;
    }
    if (base == 10) {
        return PyObject_CallFunctionObjArgs((PyObject *)int_type, obj, NULL);
    }
    return PyObject_CallFunction((PyObject *)int_type, "Oi", obj, base);
}

static inline void *PyLong_AsVoidPtr(PyObject *obj) {
    long long value = PyLong_AsLongLong(obj);
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)(intptr_t)value;
}

static inline Py_ssize_t PyLong_AsSsize_t(PyObject *obj) {
    return (Py_ssize_t)PyLong_AsLongLong(obj);
}

static inline PyObject *PyFloat_FromDouble(double value) {
    return _molt_pyobject_from_result(molt_float_from_f64(value));
}

static inline PyObject *PyFloat_FromString(PyObject *obj) {
    PyTypeObject *float_type;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    float_type = _molt_builtin_type_object_borrowed("float");
    if (float_type == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "float constructor is unavailable");
        return NULL;
    }
    return PyObject_CallFunctionObjArgs((PyObject *)float_type, obj, NULL);
}

static inline double PyFloat_AsDouble(PyObject *obj) {
    return molt_float_as_f64(_molt_py_handle(obj));
}

static inline PyObject *PyNumber_Add(PyObject *a, PyObject *b) {
    return _molt_pyobject_from_result(molt_number_add(_molt_py_handle(a), _molt_py_handle(b)));
}

static inline PyObject *PyNumber_Subtract(PyObject *a, PyObject *b) {
    return _molt_pyobject_from_result(molt_number_sub(_molt_py_handle(a), _molt_py_handle(b)));
}

static inline PyObject *PyNumber_Multiply(PyObject *a, PyObject *b) {
    return _molt_pyobject_from_result(molt_number_mul(_molt_py_handle(a), _molt_py_handle(b)));
}

static inline PyObject *PyNumber_Or(PyObject *a, PyObject *b) {
    if (a == NULL || b == NULL) {
        PyErr_SetString(PyExc_TypeError, "operands must not be NULL");
        return NULL;
    }
    return PyObject_CallMethod(a, "__or__", "O", b);
}

static inline int PyIndex_Check(PyObject *obj) {
    if (obj == NULL) {
        return 0;
    }
    if (PyLong_Check(obj)) {
        return 1;
    }
    return PyObject_HasAttrString(obj, "__index__");
}

static inline PyObject *PyNumber_Float(PyObject *obj) {
    PyTypeObject *float_type;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    float_type = _molt_builtin_type_object_borrowed("float");
    if (float_type == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "float constructor is unavailable");
        return NULL;
    }
    return PyObject_CallFunctionObjArgs((PyObject *)float_type, obj, NULL);
}

static inline PyObject *PyNumber_Index(PyObject *obj) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    if (PyLong_Check(obj)) {
        Py_INCREF(obj);
        return obj;
    }
    return PyObject_CallMethod(obj, "__index__", NULL);
}

static inline PyObject *PyNumber_Lshift(PyObject *a, PyObject *b) {
    if (a == NULL || b == NULL) {
        PyErr_SetString(PyExc_TypeError, "operands must not be NULL");
        return NULL;
    }
    return PyObject_CallMethod(a, "__lshift__", "O", b);
}

static inline PyObject *PyNumber_Negative(PyObject *obj) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "operand must not be NULL");
        return NULL;
    }
    return PyObject_CallMethod(obj, "__neg__", NULL);
}

static inline PyObject *PyNumber_TrueDivide(PyObject *a, PyObject *b) {
    return _molt_pyobject_from_result(
        molt_number_truediv(_molt_py_handle(a), _molt_py_handle(b)));
}

static inline PyObject *PyNumber_FloorDivide(PyObject *a, PyObject *b) {
    return _molt_pyobject_from_result(
        molt_number_floordiv(_molt_py_handle(a), _molt_py_handle(b)));
}

static inline PyObject *PyNumber_Long(PyObject *obj) {
    return _molt_pyobject_from_result(molt_number_long(_molt_py_handle(obj)));
}

static inline Py_ssize_t PySequence_Size(PyObject *seq) {
    return (Py_ssize_t)molt_sequence_length(_molt_py_handle(seq));
}

#define PySequence_Length PySequence_Size

static inline PyObject *PySequence_List(PyObject *seq) {
    return _molt_pyobject_from_result(molt_sequence_to_list(_molt_py_handle(seq)));
}

static inline PyObject *PySequence_Tuple(PyObject *seq) {
    return _molt_pyobject_from_result(molt_sequence_to_tuple(_molt_py_handle(seq)));
}

static inline Py_ssize_t PyNumber_AsSsize_t(PyObject *obj, PyObject *exc) {
    long long value = PyLong_AsLongLong(obj);
    if (molt_err_pending() != 0) {
        return (Py_ssize_t)-1;
    }
    if (value < (long long)PY_SSIZE_T_MIN || value > (long long)PY_SSIZE_T_MAX) {
        PyErr_SetString(
            exc != NULL ? exc : PyExc_OverflowError,
            "Python int too large to convert to Py_ssize_t");
        return (Py_ssize_t)-1;
    }
    return (Py_ssize_t)value;
}

static inline PyObject *PySequence_GetItem(PyObject *seq, Py_ssize_t index) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    MoltHandle out;
    if (molt_err_pending() != 0) {
        return NULL;
    }
    out = molt_sequence_getitem(_molt_py_handle(seq), key);
    molt_handle_decref(key);
    return _molt_pyobject_from_result(out);
}

static inline int PySequence_SetItem(PyObject *seq, Py_ssize_t index, PyObject *value) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    int rc;
    if (molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_sequence_setitem(_molt_py_handle(seq), key, _molt_py_handle(value));
    molt_handle_decref(key);
    return rc;
}

static inline int PySequence_DelItem(PyObject *seq, Py_ssize_t index) {
    PyObject *index_obj;
    PyObject *result;
    if (seq == NULL) {
        PyErr_SetString(PyExc_TypeError, "sequence must not be NULL");
        return -1;
    }
    index_obj = PyLong_FromSsize_t(index);
    if (index_obj == NULL) {
        return -1;
    }
    result = PyObject_CallMethod(seq, "__delitem__", "O", index_obj);
    Py_DECREF(index_obj);
    if (result == NULL) {
        return -1;
    }
    Py_DECREF(result);
    return 0;
}

static inline Py_ssize_t PyMapping_Size(PyObject *mapping) {
    return (Py_ssize_t)molt_mapping_length(_molt_py_handle(mapping));
}

static inline int PyMapping_Check(PyObject *mapping) {
    int has_getitem;
    int has_keys;
    if (mapping == NULL) {
        return 0;
    }
    if (PyDict_Check(mapping)) {
        return 1;
    }
    has_getitem = PyObject_HasAttrString(mapping, "__getitem__");
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    if (has_getitem == 0) {
        return 0;
    }
    has_keys = PyObject_HasAttrString(mapping, "keys");
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return has_keys;
}

static inline PyObject *PyMapping_GetItemString(PyObject *mapping, const char *key) {
    MoltHandle key_bits;
    MoltHandle out;
    if (key == NULL) {
        PyErr_SetString(PyExc_TypeError, "mapping key must not be NULL");
        return NULL;
    }
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    out = molt_mapping_getitem(_molt_py_handle(mapping), key_bits);
    molt_handle_decref(key_bits);
    return _molt_pyobject_from_result(out);
}

static inline int PyMapping_SetItemString(PyObject *mapping, const char *key, PyObject *value) {
    MoltHandle key_bits;
    int rc;
    if (key == NULL) {
        PyErr_SetString(PyExc_TypeError, "mapping key must not be NULL");
        return -1;
    }
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0 || molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_mapping_setitem(_molt_py_handle(mapping), key_bits, _molt_py_handle(value));
    molt_handle_decref(key_bits);
    return rc;
}

static inline PyObject *_molt_mapping_method_to_list(PyObject *mapping, const char *method_name) {
    PyObject *view;
    PyObject *list;
    if (mapping == NULL || method_name == NULL) {
        PyErr_SetString(PyExc_TypeError, "mapping/method name must not be NULL");
        return NULL;
    }
    view = PyObject_CallMethod(mapping, method_name, NULL);
    if (view == NULL) {
        return NULL;
    }
    list = PySequence_List(view);
    Py_DECREF(view);
    return list;
}

static inline PyObject *PyMapping_Keys(PyObject *mapping) {
    return _molt_mapping_method_to_list(mapping, "keys");
}

static inline PyObject *PyMapping_Values(PyObject *mapping) {
    return _molt_mapping_method_to_list(mapping, "values");
}

static inline PyObject *PyMapping_Items(PyObject *mapping) {
    return _molt_mapping_method_to_list(mapping, "items");
}

static inline int PyMapping_HasKey(PyObject *mapping, PyObject *key) {
    int rc;
    if (mapping == NULL || key == NULL) {
        if (molt_err_pending() != 0) {
            PyErr_Clear();
        }
        return 0;
    }
    rc = molt_object_contains(_molt_py_handle(mapping), _molt_py_handle(key));
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return rc != 0;
}

static inline int PyMapping_HasKeyString(PyObject *mapping, const char *key) {
    MoltHandle key_bits;
    int rc;
    if (mapping == NULL || key == NULL) {
        if (molt_err_pending() != 0) {
            PyErr_Clear();
        }
        return 0;
    }
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0 || molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    rc = molt_object_contains(_molt_py_handle(mapping), key_bits);
    molt_handle_decref(key_bits);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return rc != 0;
}

static inline PyObject *PyDict_New(void) {
    return _molt_pyobject_from_result(molt_dict_from_pairs(NULL, NULL, 0));
}

static inline Py_ssize_t PyDict_Size(PyObject *dict) {
    return PyMapping_Size(dict);
}

#define PyDict_GET_SIZE(op) PyDict_Size((PyObject *)(op))

static inline int PyDict_SetItem(PyObject *dict, PyObject *key, PyObject *value) {
    return molt_mapping_setitem(_molt_py_handle(dict), _molt_py_handle(key), _molt_py_handle(value));
}

static inline int PyDict_SetItemString(PyObject *dict, const char *key, PyObject *value) {
    return PyMapping_SetItemString(dict, key, value);
}

static inline int PyDict_DelItem(PyObject *dict, PyObject *key) {
    PyObject *result;
    if (dict == NULL || key == NULL) {
        PyErr_SetString(PyExc_TypeError, "dict/key must not be NULL");
        return -1;
    }
    result = PyObject_CallMethod(dict, "__delitem__", "O", key);
    if (result == NULL) {
        return -1;
    }
    Py_DECREF(result);
    return 0;
}

static inline PyObject *PyDict_Keys(PyObject *dict) {
    return PyMapping_Keys(dict);
}

static inline PyObject *PyDict_Values(PyObject *dict) {
    return PyMapping_Values(dict);
}

static inline PyObject *PyDict_Items(PyObject *dict) {
    return PyMapping_Items(dict);
}

static inline PyObject *PyDict_GetItem(PyObject *dict, PyObject *key) {
    MoltHandle out = molt_mapping_getitem(_molt_py_handle(dict), _molt_py_handle(key));
    PyObject *result = _molt_pyobject_from_result(out);
    if (result == NULL) {
        return NULL;
    }
    Py_DECREF(result);
    return result;
}

static inline PyObject *PyDict_GetItemString(PyObject *dict, const char *key) {
    PyObject *result = PyMapping_GetItemString(dict, key);
    if (result == NULL) {
        return NULL;
    }
    Py_DECREF(result);
    return result;
}

static inline int PyDict_Contains(PyObject *dict, PyObject *key) {
    return molt_object_contains(_molt_py_handle(dict), _molt_py_handle(key));
}

static inline PyObject *PyDict_GetItemWithError(PyObject *dict, PyObject *key) {
    return PyDict_GetItem(dict, key);
}

static inline int PyDict_Merge(PyObject *mp, PyObject *other, int override) {
    PyObject *items;
    Py_ssize_t item_count;
    Py_ssize_t i;
    if (mp == NULL || other == NULL) {
        PyErr_SetString(PyExc_TypeError, "dict and other must not be NULL");
        return -1;
    }
    items = PyMapping_Items(other);
    if (items == NULL) {
        return -1;
    }
    item_count = PySequence_Size(items);
    if (item_count < 0) {
        Py_DECREF(items);
        return -1;
    }
    for (i = 0; i < item_count; i++) {
        PyObject *item = PySequence_GetItem(items, i);
        PyObject *key;
        PyObject *value;
        int should_write;
        if (item == NULL) {
            Py_DECREF(items);
            return -1;
        }
        if (PySequence_Size(item) < 2) {
            Py_DECREF(item);
            Py_DECREF(items);
            PyErr_SetString(PyExc_ValueError, "mapping items must contain key/value pairs");
            return -1;
        }
        key = PySequence_GetItem(item, 0);
        value = PySequence_GetItem(item, 1);
        Py_DECREF(item);
        if (key == NULL || value == NULL) {
            Py_XDECREF(key);
            Py_XDECREF(value);
            Py_DECREF(items);
            return -1;
        }
        should_write = override || !PyMapping_HasKey(mp, key);
        if (should_write && PyDict_SetItem(mp, key, value) < 0) {
            Py_DECREF(key);
            Py_DECREF(value);
            Py_DECREF(items);
            return -1;
        }
        Py_DECREF(key);
        Py_DECREF(value);
    }
    Py_DECREF(items);
    return 0;
}

static inline PyObject *PyDict_Copy(PyObject *mp) {
    if (mp == NULL) {
        PyErr_SetString(PyExc_TypeError, "dict must not be NULL");
        return NULL;
    }
    return PyObject_CallMethod(mp, "copy", NULL);
}

static inline PyObject *PyDictProxy_New(PyObject *mapping) {
    PyObject *types_module;
    PyObject *mappingproxy_type;
    PyObject *proxy;
    if (mapping == NULL) {
        PyErr_SetString(PyExc_TypeError, "mapping must not be NULL");
        return NULL;
    }
    types_module = PyImport_ImportModule("types");
    if (types_module == NULL) {
        return NULL;
    }
    mappingproxy_type = PyObject_GetAttrString(types_module, "MappingProxyType");
    Py_DECREF(types_module);
    if (mappingproxy_type == NULL) {
        return NULL;
    }
    proxy = PyObject_CallOneArg(mappingproxy_type, mapping);
    Py_DECREF(mappingproxy_type);
    return proxy;
}

static inline int PyDict_GetItemStringRef(PyObject *dict, const char *key, PyObject **result) {
    PyObject *item = PyMapping_GetItemString(dict, key);
    if (result != NULL) {
        *result = NULL;
    }
    if (item == NULL) {
        PyErr_Clear();
        return 0;
    }
    if (result != NULL) {
        *result = item;
    } else {
        Py_DECREF(item);
    }
    return 1;
}

static inline int PyDict_GetItemRef(PyObject *dict, PyObject *key, PyObject **result) {
    PyObject *item = PyDict_GetItem(dict, key);
    if (result != NULL) {
        *result = NULL;
    }
    if (item == NULL) {
        PyErr_Clear();
        return 0;
    }
    if (result != NULL) {
        Py_INCREF(item);
        *result = item;
    }
    return 1;
}

static inline int PyDict_DelItemString(PyObject *dict, const char *key) {
    PyObject *key_obj;
    int rc;
    if (key == NULL) {
        PyErr_SetString(PyExc_TypeError, "dict key must not be NULL");
        return -1;
    }
    key_obj = PyUnicode_FromString(key);
    if (key_obj == NULL) {
        return -1;
    }
    rc = PyDict_DelItem(dict, key_obj);
    Py_DECREF(key_obj);
    return rc;
}

static inline int PyDict_Next(
    PyObject *dict,
    Py_ssize_t *ppos,
    PyObject **pkey,
    PyObject **pvalue
) {
    (void)dict;
    if (ppos != NULL) {
        *ppos = 0;
    }
    if (pkey != NULL) {
        *pkey = NULL;
    }
    if (pvalue != NULL) {
        *pvalue = NULL;
    }
    return 0;
}

static inline PyObject *PyUnicode_FromString(const char *value) {
    MoltHandle bits = _molt_string_from_utf8(value);
    if (bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(bits);
}

static inline PyObject *PyUnicode_InternFromString(const char *value) {
    PyObject *text;
    PyObject *sys_module;
    PyObject *interned;
    if (value == NULL) {
        PyErr_SetString(PyExc_TypeError, "unicode source string must not be NULL");
        return NULL;
    }
    text = PyUnicode_FromString(value);
    if (text == NULL) {
        return NULL;
    }
    sys_module = PyImport_ImportModule("sys");
    if (sys_module == NULL) {
        Py_DECREF(text);
        return NULL;
    }
    interned = PyObject_CallMethod(sys_module, "intern", "O", text);
    Py_DECREF(sys_module);
    Py_DECREF(text);
    return interned;
}

static inline PyObject *PyUnicode_FromWideChar(const wchar_t *value, Py_ssize_t size) {
    (void)value;
    (void)size;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUnicode_FromWideChar is not yet implemented in Molt's C-API layer");
    return NULL;
}

static inline PyObject *PyUnicode_FromKindAndData(
    int kind, const void *buffer, Py_ssize_t size) {
    (void)buffer;
    (void)size;
    if (kind != 4) {
        PyErr_SetString(
            PyExc_RuntimeError,
            "PyUnicode_FromKindAndData currently only supports 4-byte kind stubs");
        return NULL;
    }
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUnicode_FromKindAndData is not yet implemented in Molt's C-API layer");
    return NULL;
}

static inline Py_UCS4 *PyUnicode_AsUCS4Copy(PyObject *value) {
    (void)value;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyUnicode_AsUCS4Copy is not yet implemented in Molt's C-API layer");
    return NULL;
}

static inline PyObject *PyUnicode_Concat(PyObject *left, PyObject *right) {
    const char *left_text;
    const char *right_text;
    Py_ssize_t left_len = 0;
    Py_ssize_t right_len = 0;
    char *buf;
    MoltHandle bits;
    left_text = PyUnicode_AsUTF8AndSize(left, &left_len);
    if (left_text == NULL) {
        return NULL;
    }
    right_text = PyUnicode_AsUTF8AndSize(right, &right_len);
    if (right_text == NULL) {
        return NULL;
    }
    buf = (char *)malloc((size_t)(left_len + right_len + 1));
    if (buf == NULL) {
        return NULL;
    }
    memcpy(buf, left_text, (size_t)left_len);
    memcpy(buf + left_len, right_text, (size_t)right_len);
    buf[left_len + right_len] = '\0';
    bits = molt_string_from((const uint8_t *)buf, (uint64_t)(left_len + right_len));
    free(buf);
    return _molt_pyobject_from_result(bits);
}

static inline const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out) {
    uint64_t len = 0;
    const uint8_t *ptr = molt_string_as_ptr(_molt_py_handle(value), &len);
    if (ptr == NULL || molt_err_pending() != 0) {
        return NULL;
    }
    if (size_out != NULL) {
        *size_out = (Py_ssize_t)len;
    }
    return (const char *)ptr;
}

static inline const char *PyUnicode_AsUTF8(PyObject *value) {
    return PyUnicode_AsUTF8AndSize(value, NULL);
}

static inline PyObject *PyUnicode_AsEncodedString(
    PyObject *unicode,
    const char *encoding,
    const char *errors
) {
    const char *text;
    Py_ssize_t len = 0;
    (void)errors;
    if (encoding != NULL
        && strcmp(encoding, "utf-8") != 0
        && strcmp(encoding, "utf8") != 0
        && strcmp(encoding, "utf_8") != 0
        && strcmp(encoding, "latin-1") != 0
        && strcmp(encoding, "latin1") != 0) {
        PyErr_SetString(
            PyExc_RuntimeError,
            "PyUnicode_AsEncodedString currently only supports utf-8/latin-1");
        return NULL;
    }
    text = PyUnicode_AsUTF8AndSize(unicode, &len);
    if (text == NULL) {
        return NULL;
    }
    return PyBytes_FromStringAndSize(text, len);
}

static inline Py_ssize_t PyUnicode_GetLength(PyObject *value) {
    Py_ssize_t len = 0;
    if (PyUnicode_AsUTF8AndSize(value, &len) == NULL) {
        return -1;
    }
    return len;
}

static inline int _molt_pyunicode_is_ascii(PyObject *unicode) {
    const char *text;
    Py_ssize_t len = 0;
    Py_ssize_t i;
    text = PyUnicode_AsUTF8AndSize(unicode, &len);
    if (text == NULL) {
        return 0;
    }
    for (i = 0; i < len; i++) {
        if (((unsigned char)text[i]) > 0x7f) {
            return 0;
        }
    }
    return 1;
}

static inline void *_molt_pyunicode_data(PyObject *unicode) {
    return (void *)PyUnicode_AsUTF8AndSize(unicode, NULL);
}

static inline int PyUnicode_Compare(PyObject *left, PyObject *right) {
    const char *left_text;
    const char *right_text;
    Py_ssize_t left_len = 0;
    Py_ssize_t right_len = 0;
    int cmp;
    left_text = PyUnicode_AsUTF8AndSize(left, &left_len);
    if (left_text == NULL) {
        return -1;
    }
    right_text = PyUnicode_AsUTF8AndSize(right, &right_len);
    if (right_text == NULL) {
        return -1;
    }
    cmp = memcmp(left_text, right_text, (size_t)(left_len < right_len ? left_len : right_len));
    if (cmp != 0) {
        return cmp < 0 ? -1 : 1;
    }
    if (left_len == right_len) {
        return 0;
    }
    return left_len < right_len ? -1 : 1;
}

static inline PyObject *PyUnicode_AsUTF8String(PyObject *value) {
    const char *text;
    Py_ssize_t len = 0;
    text = PyUnicode_AsUTF8AndSize(value, &len);
    if (text == NULL) {
        return NULL;
    }
    return PyBytes_FromStringAndSize(text, len);
}

static inline PyObject *PyUnicode_AsASCIIString(PyObject *value) {
    return PyUnicode_AsUTF8String(value);
}

#define PyUnicode_1BYTE_KIND 1
#define PyUnicode_2BYTE_KIND 2
#define PyUnicode_4BYTE_KIND 4
#define PyUnicode_KIND(op) PyUnicode_4BYTE_KIND
#define PyUnicode_1BYTE_DATA(op) ((Py_UCS1 *)PyUnicode_DATA(op))
#define PyUnicode_2BYTE_DATA(op) ((Py_UCS2 *)PyUnicode_DATA(op))
#define PyUnicode_4BYTE_DATA(op) ((Py_UCS4 *)PyUnicode_DATA(op))
#define PyUnicode_READ_CHAR(op, index) \
    ((Py_UCS4)(unsigned char)PyUnicode_AsUTF8((PyObject *)(op))[index])
#define Py_UNICODE_ISSPACE(ch) \
    ((ch) == ' ' || (ch) == '\t' || (ch) == '\n' || (ch) == '\r' || (ch) == '\f' || (ch) == '\v')
#define Py_UNICODE_ISALNUM(ch) (iswalnum((wint_t)(ch)) != 0)
#define Py_UNICODE_ISALPHA(ch) (iswalpha((wint_t)(ch)) != 0)
#define Py_UNICODE_ISDECIMAL(ch) (iswdigit((wint_t)(ch)) != 0)
#define Py_UNICODE_ISDIGIT(ch) (iswdigit((wint_t)(ch)) != 0)
#define Py_UNICODE_ISLOWER(ch) (iswlower((wint_t)(ch)) != 0)
#define Py_UNICODE_ISNUMERIC(ch) (iswdigit((wint_t)(ch)) != 0)
#define Py_UNICODE_ISTITLE(ch) (iswupper((wint_t)(ch)) != 0)
#define Py_UNICODE_ISUPPER(ch) (iswupper((wint_t)(ch)) != 0)

static inline Py_ssize_t PyUnicode_GetSize(PyObject *unicode) {
    return PyUnicode_GetLength(unicode);
}

static inline int PyUnicode_CompareWithASCIIString(PyObject *unicode, const char *string) {
    PyObject *other;
    int rc;
    if (unicode == NULL || string == NULL) {
        PyErr_SetString(PyExc_TypeError, "unicode and string must not be NULL");
        return -1;
    }
    other = PyUnicode_FromString(string);
    if (other == NULL) {
        return -1;
    }
    rc = PyUnicode_Compare(unicode, other);
    Py_DECREF(other);
    return rc;
}

static inline int PyUnicode_Contains(PyObject *container, PyObject *element) {
    return molt_object_contains(_molt_py_handle(container), _molt_py_handle(element));
}

static inline int PyUnicode_FSConverter(PyObject *obj, void *result) {
    PyObject **output = (PyObject **)result;
    PyObject *bytes_obj;
    if (output == NULL) {
        PyErr_SetString(PyExc_TypeError, "result must not be NULL");
        return 0;
    }
    *output = NULL;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return 0;
    }
    if (PyBytes_Check(obj)) {
        *output = Py_NewRef(obj);
        return 1;
    }
    if (!PyUnicode_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, "expected str, bytes, or os.PathLike object");
        return 0;
    }
    bytes_obj = PyUnicode_AsEncodedString(obj, "utf-8", "strict");
    if (bytes_obj == NULL) {
        return 0;
    }
    *output = bytes_obj;
    return 1;
}

static inline PyObject *PyUnicode_Replace(
    PyObject *string,
    PyObject *substr,
    PyObject *replacement,
    Py_ssize_t maxcount
) {
    if (maxcount < 0) {
        return PyObject_CallMethod(string, "replace", "OO", substr, replacement);
    }
    return PyObject_CallMethod(string, "replace", "OOn", substr, replacement, maxcount);
}

static inline int PyUnicode_Tailmatch(
    PyObject *string,
    PyObject *substring,
    Py_ssize_t start,
    Py_ssize_t end,
    int direction
) {
    PyObject *result;
    int truthy;
    const char *method = direction < 0 ? "startswith" : "endswith";
    result = PyObject_CallMethod(string, method, "Onn", substring, start, end);
    if (result == NULL) {
        return -1;
    }
    truthy = PyObject_IsTrue(result);
    Py_DECREF(result);
    return truthy;
}

static inline PyObject *PyUnicode_AsLatin1String(PyObject *unicode) {
    return PyUnicode_AsEncodedString(unicode, "latin-1", NULL);
}

static inline PyObject *PyUnicode_Format(PyObject *format, PyObject *args) {
    if (format == NULL) {
        PyErr_SetString(PyExc_TypeError, "format string must not be NULL");
        return NULL;
    }
    if (args == NULL) {
        args = Py_None;
    }
    return PyObject_CallMethod(format, "__mod__", "O", args);
}

static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size) {
    if (value == NULL && size > 0) {
        PyErr_SetString(PyExc_TypeError, "bytes source must not be NULL when size > 0");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_bytes_from((const uint8_t *)value, size < 0 ? 0u : (uint64_t)size));
}

static inline PyObject *PyBytes_FromString(const char *value) {
    if (value == NULL) {
        PyErr_SetString(PyExc_TypeError, "bytes value must not be NULL");
        return NULL;
    }
    return PyBytes_FromStringAndSize(value, (Py_ssize_t)strlen(value));
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

typedef struct {
    MoltBufferView view;
    Py_ssize_t shape[1];
    Py_ssize_t strides[1];
    char format[2];
} _MoltPyBufferBridge;

static inline int PyObject_GetBuffer(PyObject *obj, Py_buffer *view, int flags) {
    _MoltPyBufferBridge *bridge;
    Py_ssize_t itemsize;
    Py_ssize_t len;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "buffer object must not be NULL");
        return -1;
    }
    if (view == NULL) {
        PyErr_SetString(PyExc_TypeError, "buffer view must not be NULL");
        return -1;
    }
    memset(view, 0, sizeof(*view));
    bridge = (_MoltPyBufferBridge *)PyMem_Calloc(1, sizeof(*bridge));
    if (bridge == NULL) {
        PyErr_NoMemory();
        return -1;
    }
    if (molt_buffer_acquire(_molt_py_handle(obj), &bridge->view) != 0) {
        PyMem_Free(bridge);
        return -1;
    }
    len = (Py_ssize_t)bridge->view.len;
    itemsize = bridge->view.itemsize > 0 ? (Py_ssize_t)bridge->view.itemsize : 1;
    bridge->shape[0] = itemsize > 0 ? len / itemsize : len;
    bridge->strides[0] = bridge->view.stride != 0 ? (Py_ssize_t)bridge->view.stride : itemsize;
    bridge->format[0] = 'B';
    bridge->format[1] = '\0';

    view->buf = bridge->view.data;
    view->obj = obj;
    Py_INCREF(obj);
    view->len = len;
    view->itemsize = itemsize;
    view->readonly = bridge->view.readonly != 0 ? 1 : 0;
    view->ndim = 1;
    view->format = (flags & PyBUF_FORMAT) != 0 ? bridge->format : NULL;
    view->shape = (flags & PyBUF_ND) != 0 ? bridge->shape : NULL;
    view->strides = (flags & PyBUF_STRIDES) != 0 ? bridge->strides : NULL;
    view->suboffsets = NULL;
    view->internal = bridge;

    if ((flags & PyBUF_WRITABLE) != 0 && view->readonly) {
        (void)molt_buffer_release(&bridge->view);
        Py_DECREF(obj);
        PyMem_Free(bridge);
        memset(view, 0, sizeof(*view));
        PyErr_SetString(PyExc_BufferError, "requested writable buffer from readonly object");
        return -1;
    }
    return 0;
}

static inline void PyBuffer_Release(Py_buffer *view) {
    _MoltPyBufferBridge *bridge;
    if (view == NULL) {
        return;
    }
    bridge = (_MoltPyBufferBridge *)view->internal;
    if (bridge != NULL) {
        (void)molt_buffer_release(&bridge->view);
        PyMem_Free(bridge);
    }
    Py_XDECREF(view->obj);
    memset(view, 0, sizeof(*view));
}

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

static inline Py_ssize_t PyBytes_Size(PyObject *value) {
    if (value == NULL) {
        PyErr_SetString(PyExc_TypeError, "bytes object must not be NULL");
        return -1;
    }
    return _molt_pybytes_get_size(value);
}

#define PyString_Check(op) PyBytes_Check((PyObject *)(op))
#define PyString_FromString PyBytes_FromString
#define PyString_AsString PyBytes_AsString
#define PyString_AS_STRING(op) PyBytes_AsString((PyObject *)(op))
#define PyString_GET_SIZE(op) PyBytes_Size((PyObject *)(op))

static inline PyObject *PyByteArray_FromStringAndSize(const char *value, Py_ssize_t size) {
    if (value == NULL && size > 0) {
        PyErr_SetString(PyExc_TypeError, "bytearray source must not be NULL when size > 0");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_bytearray_from((const uint8_t *)value, size < 0 ? 0u : (uint64_t)size));
}

static inline char *PyByteArray_AsString(PyObject *value) {
    uint8_t *ptr = molt_bytearray_as_ptr(_molt_py_handle(value), NULL);
    if (ptr == NULL || molt_err_pending() != 0) {
        return NULL;
    }
    return (char *)ptr;
}

static inline Py_ssize_t _molt_pybytearray_get_size(PyObject *value) {
    uint64_t len = 0;
    uint8_t *ptr = molt_bytearray_as_ptr(_molt_py_handle(value), &len);
    if (ptr == NULL || molt_err_pending() != 0) {
        return -1;
    }
    return (Py_ssize_t)len;
}

static inline Py_ssize_t PyByteArray_Size(PyObject *value) {
    return _molt_pybytearray_get_size(value);
}

#define PyByteArray_AS_STRING(op)                                                  \
    ((char *)molt_bytearray_as_ptr(_molt_py_handle((PyObject *)(op)), NULL))
#define PyByteArray_GET_SIZE(op) _molt_pybytearray_get_size((PyObject *)(op))

static inline PyObject *PyUnicode_FromStringAndSize(const char *value, Py_ssize_t size) {
    if (value == NULL && size > 0) {
        PyErr_SetString(PyExc_TypeError, "unicode source must not be NULL when size > 0");
        return NULL;
    }
    if (size < 0) {
        if (value == NULL) {
            PyErr_SetString(PyExc_TypeError, "unicode source must not be NULL");
            return NULL;
        }
        return _molt_pyobject_from_result(
            molt_string_from((const uint8_t *)value, (uint64_t)strlen(value)));
    }
    return _molt_pyobject_from_result(
        molt_string_from((const uint8_t *)value, (uint64_t)size));
}

static inline PyObject *PyUnicode_FromFormat(const char *format, ...) {
    char stack_buf[1024];
    va_list ap;
    int needed;
    if (format == NULL) {
        PyErr_SetString(PyExc_TypeError, "format must not be NULL");
        return NULL;
    }
    va_start(ap, format);
    needed = vsnprintf(stack_buf, sizeof(stack_buf), format, ap);
    va_end(ap);
    if (needed < 0) {
        PyErr_SetString(PyExc_ValueError, "failed to format Unicode string");
        return NULL;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        return PyUnicode_FromStringAndSize(stack_buf, (Py_ssize_t)needed);
    }
    {
        size_t cap = (size_t)needed + 1;
        char *heap_buf = (char *)PyMem_Malloc(cap);
        PyObject *out;
        if (heap_buf == NULL) {
            return NULL;
        }
        va_start(ap, format);
        (void)vsnprintf(heap_buf, cap, format, ap);
        va_end(ap);
        out = PyUnicode_FromStringAndSize(heap_buf, (Py_ssize_t)needed);
        PyMem_Free(heap_buf);
        return out;
    }
}

static inline PyObject *PyUnicode_FromFormatV(const char *format, va_list vargs) {
    char stack_buf[1024];
    va_list copy;
    int needed;
    if (format == NULL) {
        PyErr_SetString(PyExc_TypeError, "format must not be NULL");
        return NULL;
    }
    va_copy(copy, vargs);
    needed = vsnprintf(stack_buf, sizeof(stack_buf), format, copy);
    va_end(copy);
    if (needed < 0) {
        PyErr_SetString(PyExc_ValueError, "failed to format Unicode string");
        return NULL;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        return PyUnicode_FromStringAndSize(stack_buf, (Py_ssize_t)needed);
    }
    {
        size_t cap = (size_t)needed + 1;
        char *heap_buf = (char *)PyMem_Malloc(cap);
        PyObject *out;
        if (heap_buf == NULL) {
            return NULL;
        }
        va_copy(copy, vargs);
        (void)vsnprintf(heap_buf, cap, format, copy);
        va_end(copy);
        out = PyUnicode_FromStringAndSize(heap_buf, (Py_ssize_t)needed);
        PyMem_Free(heap_buf);
        return out;
    }
}

static inline PyObject *PyUnicode_FromEncodedObject(
    PyObject *obj,
    const char *encoding,
    const char *errors
) {
    char *bytes_ptr = NULL;
    Py_ssize_t bytes_len = 0;
    (void)errors;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    if (PyUnicode_Check(obj)) {
        Py_INCREF(obj);
        return obj;
    }
    if (encoding != NULL
        && strcmp(encoding, "utf-8") != 0
        && strcmp(encoding, "utf8") != 0
        && strcmp(encoding, "ascii") != 0) {
        PyErr_SetString(PyExc_ValueError, "only utf-8/ascii encoding is supported");
        return NULL;
    }
    if (PyBytes_AsStringAndSize(obj, &bytes_ptr, &bytes_len) < 0) {
        return NULL;
    }
    return PyUnicode_FromStringAndSize(bytes_ptr, bytes_len);
}

static inline PyObject *PyTuple_New(Py_ssize_t size) {
    MoltHandle bits = 0;
    if (size < 0) {
        PyErr_SetString(PyExc_ValueError, "tuple size must be >= 0");
        return NULL;
    }
    if (size == 0) {
        bits = molt_tuple_from_array(NULL, 0);
        return _molt_pyobject_from_result(bits);
    }
    {
        MoltHandle *items = (MoltHandle *)calloc((size_t)size, sizeof(MoltHandle));
        Py_ssize_t i = 0;
        if (items == NULL) {
            PyErr_SetString(PyExc_RuntimeError, "out of memory");
            return NULL;
        }
        for (i = 0; i < size; i++) {
            items[i] = molt_none();
        }
        bits = molt_tuple_from_array(items, (uint64_t)size);
        free(items);
    }
    return _molt_pyobject_from_result(bits);
}

static inline PyObject *PyList_New(Py_ssize_t size) {
    MoltHandle bits = 0;
    if (size < 0) {
        PyErr_SetString(PyExc_ValueError, "list size must be >= 0");
        return NULL;
    }
    if (size == 0) {
        bits = molt_list_from_array(NULL, 0);
        return _molt_pyobject_from_result(bits);
    }
    {
        MoltHandle *items = (MoltHandle *)calloc((size_t)size, sizeof(MoltHandle));
        Py_ssize_t i = 0;
        if (items == NULL) {
            return PyErr_NoMemory();
        }
        for (i = 0; i < size; i++) {
            items[i] = molt_none();
        }
        bits = molt_list_from_array(items, (uint64_t)size);
        free(items);
    }
    return _molt_pyobject_from_result(bits);
}

static inline Py_ssize_t PyList_Size(PyObject *list) {
    return (Py_ssize_t)molt_sequence_length(_molt_py_handle(list));
}

static inline PyObject *PyList_GetItem(PyObject *list, Py_ssize_t index) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    MoltHandle out;
    PyObject *result;
    if (molt_err_pending() != 0) {
        return NULL;
    }
    out = molt_sequence_getitem(_molt_py_handle(list), key);
    molt_handle_decref(key);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) {
        return NULL;
    }
    Py_DECREF(result);
    return result;
}

static inline PyObject *PyList_GetItemRef(PyObject *list, Py_ssize_t index) {
    PyObject *item = PyList_GetItem(list, index);
    if (item != NULL) {
        Py_INCREF(item);
    }
    return item;
}

static inline int PyList_SetItem(PyObject *list, Py_ssize_t index, PyObject *value) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    int rc;
    if (molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_sequence_setitem(_molt_py_handle(list), key, _molt_py_handle(value));
    molt_handle_decref(key);
    if (rc == 0 && value != NULL) {
        Py_DECREF(value);
    }
    return rc;
}

static inline int PyList_Append(PyObject *list, PyObject *item) {
    PyObject *append = PyObject_GetAttrString(list, "append");
    MoltHandle args_bits;
    PyObject *args_obj;
    PyObject *out;
    MoltHandle arg_bits;
    if (append == NULL) {
        return -1;
    }
    arg_bits = _molt_py_handle(item);
    args_bits = molt_tuple_from_array(&arg_bits, 1);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(append);
        return -1;
    }
    args_obj = _molt_pyobject_from_handle(args_bits);
    out = PyObject_CallObject(append, args_obj);
    molt_handle_decref(args_bits);
    Py_DECREF(append);
    if (out == NULL) {
        return -1;
    }
    Py_DECREF(out);
    return 0;
}

static inline Py_ssize_t PyTuple_Size(PyObject *tuple) {
    return (Py_ssize_t)molt_sequence_length(_molt_py_handle(tuple));
}

static inline PyObject *PyTuple_GetItem(PyObject *tuple, Py_ssize_t index) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    MoltHandle out;
    PyObject *result;
    if (molt_err_pending() != 0) {
        return NULL;
    }
    out = molt_sequence_getitem(_molt_py_handle(tuple), key);
    molt_handle_decref(key);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) {
        return NULL;
    }
    /*
     * CPython returns a borrowed reference for PyTuple_GetItem.
     * Drop one owned reference to match that contract.
     */
    Py_DECREF(result);
    return result;
}

static inline int PyTuple_SetItem(PyObject *tuple, Py_ssize_t index, PyObject *value) {
    MoltHandle key = molt_int_from_i64((int64_t)index);
    int rc;
    if (molt_err_pending() != 0) {
        return -1;
    }
    rc = molt_sequence_setitem(_molt_py_handle(tuple), key, _molt_py_handle(value));
    molt_handle_decref(key);
    if (rc == 0 && value != NULL) {
        /*
         * CPython PyTuple_SetItem steals the reference to value on success.
         */
        Py_DECREF(value);
    }
    return rc;
}

static inline PyObject *PyList_AsTuple(PyObject *list) {
    if (list == NULL) {
        PyErr_SetString(PyExc_TypeError, "list must not be NULL");
        return NULL;
    }
    return PySequence_Tuple(list);
}

static inline int PyList_SetSlice(
    PyObject *list,
    Py_ssize_t low,
    Py_ssize_t high,
    PyObject *itemlist
) {
    PyObject *start_obj;
    PyObject *stop_obj;
    PyObject *slice_obj;
    PyObject *result;
    if (list == NULL) {
        PyErr_SetString(PyExc_TypeError, "list must not be NULL");
        return -1;
    }
    start_obj = PyLong_FromSsize_t(low);
    if (start_obj == NULL) {
        return -1;
    }
    stop_obj = PyLong_FromSsize_t(high);
    if (stop_obj == NULL) {
        Py_DECREF(start_obj);
        return -1;
    }
    slice_obj = PySlice_New(start_obj, stop_obj, Py_None);
    Py_DECREF(stop_obj);
    Py_DECREF(start_obj);
    if (slice_obj == NULL) {
        return -1;
    }
    if (itemlist == NULL) {
        result = PyObject_CallMethod(list, "__delitem__", "O", slice_obj);
    } else {
        result = PyObject_CallMethod(list, "__setitem__", "OO", slice_obj, itemlist);
    }
    Py_DECREF(slice_obj);
    if (result == NULL) {
        return -1;
    }
    Py_DECREF(result);
    return 0;
}

static inline PyObject *PyTuple_GetSlice(PyObject *tuple, Py_ssize_t low, Py_ssize_t high) {
    Py_ssize_t tuple_size;
    Py_ssize_t slice_size;
    Py_ssize_t i;
    PyObject *slice;
    if (tuple == NULL) {
        PyErr_SetString(PyExc_TypeError, "tuple must not be NULL");
        return NULL;
    }
    if (!PyTuple_Check(tuple)) {
        PyErr_SetString(PyExc_TypeError, "expected tuple");
        return NULL;
    }
    tuple_size = PyTuple_Size(tuple);
    if (tuple_size < 0) {
        return NULL;
    }
    if (low < 0) {
        low += tuple_size;
    }
    if (high < 0) {
        high += tuple_size;
    }
    if (low < 0) {
        low = 0;
    }
    if (low > tuple_size) {
        low = tuple_size;
    }
    if (high < low) {
        high = low;
    }
    if (high > tuple_size) {
        high = tuple_size;
    }
    slice_size = high - low;
    slice = PyTuple_New(slice_size);
    if (slice == NULL) {
        return NULL;
    }
    for (i = 0; i < slice_size; i++) {
        PyObject *item = PyTuple_GetItem(tuple, low + i);
        if (item == NULL) {
            Py_DECREF(slice);
            return NULL;
        }
        Py_INCREF(item);
        if (PyTuple_SetItem(slice, i, item) < 0) {
            Py_DECREF(item);
            Py_DECREF(slice);
            return NULL;
        }
    }
    return slice;
}

static inline PyObject *PySlice_New(PyObject *start, PyObject *stop, PyObject *step) {
    PyTypeObject *slice_type = _molt_builtin_type_object_borrowed("slice");
    PyObject *args;
    PyObject *out;
    if (slice_type == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "slice type is unavailable");
        return NULL;
    }
    if (start == NULL) {
        start = Py_None;
    }
    if (stop == NULL) {
        stop = Py_None;
    }
    if (step == NULL) {
        step = Py_None;
    }
    args = PyTuple_Pack(3, start, stop, step);
    if (args == NULL) {
        return NULL;
    }
    out = PyObject_CallObject((PyObject *)slice_type, args);
    Py_DECREF(args);
    return out;
}

static inline int PySlice_AdjustIndices(
    Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t step) {
    Py_ssize_t slicelen;
    if (step == 0 || start == NULL || stop == NULL) {
        return 0;
    }
    if (*start < 0) {
        *start += length;
        if (*start < 0) {
            *start = step < 0 ? -1 : 0;
        }
    } else if (*start >= length) {
        *start = step < 0 ? length - 1 : length;
    }
    if (*stop < 0) {
        *stop += length;
        if (*stop < 0) {
            *stop = step < 0 ? -1 : 0;
        }
    } else if (*stop >= length) {
        *stop = step < 0 ? length - 1 : length;
    }
    if (step < 0) {
        if (*stop < *start) {
            slicelen = (*start - *stop - 1) / (-step) + 1;
        } else {
            slicelen = 0;
        }
    } else {
        if (*start < *stop) {
            slicelen = (*stop - *start - 1) / step + 1;
        } else {
            slicelen = 0;
        }
    }
    return (int)slicelen;
}

static inline int PySlice_GetIndicesEx(
    PyObject *slice,
    Py_ssize_t length,
    Py_ssize_t *start,
    Py_ssize_t *stop,
    Py_ssize_t *step,
    Py_ssize_t *slicelength
) {
    PyObject *indices;
    Py_ssize_t local_start;
    Py_ssize_t local_stop;
    Py_ssize_t local_step;
    if (slice == NULL || start == NULL || stop == NULL || step == NULL || slicelength == NULL) {
        PyErr_SetString(PyExc_TypeError, "slice and output pointers must not be NULL");
        return -1;
    }
    indices = PyObject_CallMethod(slice, "indices", "n", length);
    if (indices == NULL) {
        return -1;
    }
    if (!PyTuple_Check(indices) || PyTuple_Size(indices) != 3) {
        Py_DECREF(indices);
        PyErr_SetString(PyExc_TypeError, "slice.indices() must return a 3-tuple");
        return -1;
    }
    local_start = PyLong_AsSsize_t(PyTuple_GetItem(indices, 0));
    if (molt_err_pending() != 0) {
        Py_DECREF(indices);
        return -1;
    }
    local_stop = PyLong_AsSsize_t(PyTuple_GetItem(indices, 1));
    if (molt_err_pending() != 0) {
        Py_DECREF(indices);
        return -1;
    }
    local_step = PyLong_AsSsize_t(PyTuple_GetItem(indices, 2));
    Py_DECREF(indices);
    if (molt_err_pending() != 0) {
        return -1;
    }
    if (local_step == 0) {
        PyErr_SetString(PyExc_ValueError, "slice step cannot be zero");
        return -1;
    }
    *start = local_start;
    *stop = local_stop;
    *step = local_step;
    if ((local_step < 0 && local_stop >= local_start) || (local_step > 0 && local_start >= local_stop)) {
        *slicelength = 0;
    } else if (local_step < 0) {
        *slicelength = (local_start - local_stop - 1) / (-local_step) + 1;
    } else {
        *slicelength = (local_stop - local_start - 1) / local_step + 1;
    }
    return 0;
}

static inline PyObject *PyTuple_Pack(Py_ssize_t n, ...) {
    va_list ap;
    MoltHandle *items;
    Py_ssize_t i;
    PyObject *out;
    if (n < 0) {
        PyErr_SetString(PyExc_ValueError, "tuple size must be >= 0");
        return NULL;
    }
    items = (MoltHandle *)PyMem_Calloc((size_t)n, sizeof(MoltHandle));
    if (items == NULL) {
        return NULL;
    }
    va_start(ap, n);
    for (i = 0; i < n; i++) {
        PyObject *item = va_arg(ap, PyObject *);
        if (item == NULL) {
            va_end(ap);
            PyMem_Free(items);
            PyErr_SetString(PyExc_TypeError, "PyTuple_Pack received NULL item");
            return NULL;
        }
        items[i] = _molt_py_handle(item);
    }
    va_end(ap);
    out = _molt_pyobject_from_result(molt_tuple_from_array(items, (uint64_t)n));
    PyMem_Free(items);
    return out;
}

#define PyTuple_GET_SIZE(op) PyTuple_Size((PyObject *)(op))
#define PyTuple_GET_ITEM(op, index) PyTuple_GetItem((PyObject *)(op), (index))
#define PyTuple_SET_ITEM(op, index, value)                                         \
    PyTuple_SetItem((PyObject *)(op), (index), (PyObject *)(value))

static inline size_t PyVectorcall_NARGS(size_t nargsf) {
    return nargsf & ~PY_VECTORCALL_ARGUMENTS_OFFSET;
}

static inline PyObject *PyObject_Vectorcall(
    PyObject *callable,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwnames
) {
    Py_ssize_t nargs = (Py_ssize_t)PyVectorcall_NARGS(nargsf);
    PyObject *call_args;
    PyObject *result;
    Py_ssize_t i;
    if (kwnames != NULL && PyTuple_Size(kwnames) != 0) {
        PyErr_SetString(
            PyExc_RuntimeError,
            "PyObject_Vectorcall keyword arguments are not yet implemented in Molt's C-API layer");
        return NULL;
    }
    if (nargs == 0) {
        return PyObject_CallObject(callable, NULL);
    }
    call_args = PyTuple_New(nargs);
    if (call_args == NULL) {
        return NULL;
    }
    for (i = 0; i < nargs; i++) {
        PyObject *item = args != NULL ? args[i] : NULL;
        if (item == NULL) {
            Py_DECREF(call_args);
            PyErr_SetString(PyExc_TypeError, "PyObject_Vectorcall received NULL positional arg");
            return NULL;
        }
        Py_INCREF(item);
        if (PyTuple_SetItem(call_args, i, item) < 0) {
            Py_DECREF(item);
            Py_DECREF(call_args);
            return NULL;
        }
    }
    result = PyObject_CallObject(callable, call_args);
    Py_DECREF(call_args);
    return result;
}

static inline PyObject *PyObject_VectorcallMethod(
    PyObject *name,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwnames
) {
    Py_ssize_t nargs = (Py_ssize_t)PyVectorcall_NARGS(nargsf);
    PyObject *method;
    PyObject *result;
    if (nargs <= 0 || args == NULL || args[0] == NULL) {
        PyErr_SetString(PyExc_TypeError, "PyObject_VectorcallMethod requires self argument");
        return NULL;
    }
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "method name must not be NULL");
        return NULL;
    }
    method = PyObject_GetAttr(args[0], name);
    if (method == NULL) {
        return NULL;
    }
    result = PyObject_Vectorcall(method, args + 1, (size_t)(nargs - 1), kwnames);
    Py_DECREF(method);
    return result;
}

static inline PyObject *PyVectorcall_Call(PyObject *callable, PyObject *tuple, PyObject *dict) {
    if (callable == NULL) {
        PyErr_SetString(PyExc_TypeError, "callable must not be NULL");
        return NULL;
    }
    if (tuple == NULL) {
        tuple = PyTuple_New(0);
        if (tuple == NULL) {
            return NULL;
        }
        if (dict == NULL) {
            PyObject *result = PyObject_CallObject(callable, tuple);
            Py_DECREF(tuple);
            return result;
        }
        {
            PyObject *result = PyObject_Call(callable, tuple, dict);
            Py_DECREF(tuple);
            return result;
        }
    }
    if (!PyTuple_Check(tuple)) {
        PyErr_SetString(PyExc_TypeError, "second argument must be a tuple");
        return NULL;
    }
    if (dict == NULL) {
        return PyObject_CallObject(callable, tuple);
    }
    return PyObject_Call(callable, tuple, dict);
}

#define PyList_GET_SIZE(op) PyList_Size((PyObject *)(op))
#define PyList_GET_ITEM(op, index) PyList_GetItem((PyObject *)(op), (index))
#define PyList_SET_ITEM(op, index, value)                                          \
    PyList_SetItem((PyObject *)(op), (index), (PyObject *)(value))

static inline int _molt_parse_int64_arg(MoltHandle arg_bits, int64_t *out) {
    int64_t value = molt_int_as_i64(arg_bits);
    if (molt_err_pending() != 0) {
        return 0;
    }
    *out = value;
    return 1;
}

static inline int _molt_parse_int64_range_arg(
    MoltHandle arg_bits,
    int64_t min_value,
    int64_t max_value,
    int64_t *out,
    char code,
    const char *api_name
) {
    int64_t value = 0;
    if (!_molt_parse_int64_arg(arg_bits, &value)) {
        return 0;
    }
    if (value < min_value || value > max_value) {
        PyErr_Format(
            PyExc_OverflowError,
            "integer argument out of range for '%c' in %s",
            code,
            api_name);
        return 0;
    }
    if (out != NULL) {
        *out = value;
    }
    return 1;
}

static inline int _molt_parse_uint64_range_arg(
    MoltHandle arg_bits,
    uint64_t max_value,
    uint64_t *out,
    char code,
    const char *api_name
) {
    int64_t value = 0;
    if (!_molt_parse_int64_arg(arg_bits, &value)) {
        return 0;
    }
    if (value < 0 || (uint64_t)value > max_value) {
        PyErr_Format(
            PyExc_OverflowError,
            "integer argument out of range for '%c' in %s",
            code,
            api_name);
        return 0;
    }
    if (out != NULL) {
        *out = (uint64_t)value;
    }
    return 1;
}

static inline int _molt_pyarg_get_positional_item(
    PyObject *args,
    int64_t index,
    MoltHandle *out_bits
) {
    MoltHandle key_bits = molt_int_from_i64(index);
    MoltHandle item_bits;
    if (key_bits == 0 || molt_err_pending() != 0) {
        if (key_bits != 0 && key_bits != molt_none()) {
            molt_handle_decref(key_bits);
        }
        return 0;
    }
    item_bits = molt_sequence_getitem(_molt_py_handle(args), key_bits);
    molt_handle_decref(key_bits);
    if (molt_err_pending() != 0) {
        if (item_bits != 0 && item_bits != molt_none()) {
            molt_handle_decref(item_bits);
        }
        return 0;
    }
    *out_bits = item_bits;
    return 1;
}

static inline int _molt_pyarg_object_matches_type(MoltHandle obj_bits, MoltHandle expected_type_bits) {
    MoltHandle class_bits = 0;
    MoltHandle mro_bits = 0;
    int matched = 0;
    if (expected_type_bits == 0 || expected_type_bits == molt_none()) {
        return 0;
    }
    class_bits = molt_object_getattr_bytes(obj_bits, (const uint8_t *)"__class__", 9);
    if (molt_err_pending() != 0 || class_bits == 0 || class_bits == molt_none()) {
        PyErr_Clear();
        goto done;
    }
    {
        int same_type = molt_object_equal(class_bits, expected_type_bits);
        if (molt_err_pending() != 0) {
            PyErr_Clear();
            goto done;
        }
        if (same_type != 0) {
            matched = 1;
            goto done;
        }
    }
    mro_bits = molt_object_getattr_bytes(class_bits, (const uint8_t *)"__mro__", 7);
    if (molt_err_pending() != 0 || mro_bits == 0 || mro_bits == molt_none()) {
        PyErr_Clear();
        goto done;
    }
    {
        int64_t mro_len = molt_sequence_length(mro_bits);
        int64_t mro_idx = 0;
        if (mro_len < 0 || molt_err_pending() != 0) {
            PyErr_Clear();
            goto done;
        }
        while (mro_idx < mro_len) {
            MoltHandle idx_bits = molt_int_from_i64(mro_idx);
            MoltHandle mro_item_bits = 0;
            int same_type = 0;
            if (idx_bits == 0 || molt_err_pending() != 0) {
                if (idx_bits != 0 && idx_bits != molt_none()) {
                    molt_handle_decref(idx_bits);
                }
                PyErr_Clear();
                break;
            }
            mro_item_bits = molt_sequence_getitem(mro_bits, idx_bits);
            molt_handle_decref(idx_bits);
            if (molt_err_pending() != 0 || mro_item_bits == 0 || mro_item_bits == molt_none()) {
                if (mro_item_bits != 0 && mro_item_bits != molt_none()) {
                    molt_handle_decref(mro_item_bits);
                }
                PyErr_Clear();
                break;
            }
            same_type = molt_object_equal(mro_item_bits, expected_type_bits);
            molt_handle_decref(mro_item_bits);
            if (molt_err_pending() != 0) {
                PyErr_Clear();
                break;
            }
            if (same_type != 0) {
                matched = 1;
                break;
            }
            mro_idx++;
        }
    }
done:
    if (mro_bits != 0 && mro_bits != molt_none()) {
        molt_handle_decref(mro_bits);
    }
    if (class_bits != 0 && class_bits != molt_none()) {
        molt_handle_decref(class_bits);
    }
    return matched;
}

static inline MoltHandle _molt_builtin_type_handle_cached(const char *name) {
    static struct {
        const char *name;
        MoltHandle bits;
    } cache[] = {
        {"bool", 0},
        {"int", 0},
        {"float", 0},
        {"complex", 0},
        {"tuple", 0},
        {"list", 0},
        {"dict", 0},
        {"str", 0},
        {"bytes", 0},
        {"bytearray", 0},
        {"set", 0},
        {"frozenset", 0},
        {"type", 0},
    };
    size_t i;
    for (i = 0; i < sizeof(cache) / sizeof(cache[0]); i++) {
        if (strcmp(cache[i].name, name) != 0) {
            continue;
        }
        if (cache[i].bits == 0) {
            PyObject *type_obj = _molt_builtin_class_lookup_utf8(cache[i].name);
            if (type_obj == NULL) {
                PyErr_Clear();
                return 0;
            }
            cache[i].bits = _molt_py_handle(type_obj);
        }
        return cache[i].bits;
    }
    return 0;
}

static inline PyTypeObject *_molt_builtin_type_object_borrowed(const char *name) {
    MoltHandle bits = _molt_builtin_type_handle_cached(name);
    if (bits == 0) {
        return NULL;
    }
    return (PyTypeObject *)_molt_pyobject_from_handle(bits);
}

static inline PyTypeObject *_molt_mappingproxy_type_object_borrowed(void) {
    static MoltHandle cached = 0;
    static PyTypeObject unavailable = {0};
    if (cached == 0) {
        PyObject *types_module = PyImport_ImportModule("types");
        PyObject *mappingproxy_type;
        if (types_module == NULL) {
            PyErr_Clear();
            return &unavailable;
        }
        mappingproxy_type = PyObject_GetAttrString(types_module, "MappingProxyType");
        Py_DECREF(types_module);
        if (mappingproxy_type == NULL) {
            PyErr_Clear();
            return &unavailable;
        }
        cached = _molt_py_handle(mappingproxy_type);
        Py_DECREF(mappingproxy_type);
    }
    return (PyTypeObject *)_molt_pyobject_from_handle(cached);
}

static inline PyTypeObject *_molt_cfunction_type_object_borrowed(void) {
    static MoltHandle cached = 0;
    static PyTypeObject unavailable = {0};
    if (cached == 0) {
        PyObject *builtins_module = PyImport_ImportModule("builtins");
        PyObject *len_obj;
        PyObject *type_obj;
        if (builtins_module == NULL) {
            PyErr_Clear();
            return &unavailable;
        }
        len_obj = PyObject_GetAttrString(builtins_module, "len");
        Py_DECREF(builtins_module);
        if (len_obj == NULL) {
            PyErr_Clear();
            return &unavailable;
        }
        type_obj = PyObject_Type(len_obj);
        Py_DECREF(len_obj);
        if (type_obj == NULL) {
            PyErr_Clear();
            return &unavailable;
        }
        cached = _molt_py_handle(type_obj);
        Py_DECREF(type_obj);
    }
    return (PyTypeObject *)_molt_pyobject_from_handle(cached);
}

#define PyLong_Type (*_molt_builtin_type_object_borrowed("int"))
#define PyIntUArrType_Type PyLong_Type
#define PyFloat_Type (*_molt_builtin_type_object_borrowed("float"))
#define PyBool_Type (*_molt_builtin_type_object_borrowed("bool"))
#define PyCFunction_Type (*_molt_cfunction_type_object_borrowed())
#define PyBytes_Type (*_molt_builtin_type_object_borrowed("bytes"))
#define PyByteArray_Type (*_molt_builtin_type_object_borrowed("bytearray"))
#define PyDict_Type (*_molt_builtin_type_object_borrowed("dict"))
#define PyList_Type (*_molt_builtin_type_object_borrowed("list"))
#define PySlice_Type (*_molt_builtin_type_object_borrowed("slice"))
#define PyUnicode_Type (*_molt_builtin_type_object_borrowed("str"))
#define PyComplex_Type (*_molt_builtin_type_object_borrowed("complex"))
#define PyTuple_Type (*_molt_builtin_type_object_borrowed("tuple"))
#define PyType_Type (*_molt_builtin_type_object_borrowed("type"))
#define PyBaseObject_Type (*_molt_builtin_type_object_borrowed("object"))
#define PyMemoryView_Type (*_molt_builtin_type_object_borrowed("memoryview"))
#define PyDictProxy_Type (*_molt_mappingproxy_type_object_borrowed())
#define PySet_Type (*_molt_builtin_type_object_borrowed("set"))
#define PyFrozenSet_Type (*_molt_builtin_type_object_borrowed("frozenset"))
#define PyGetSetDescr_Type PyBaseObject_Type
#define PyMemberDescr_Type PyBaseObject_Type
#define PyMethodDescr_Type PyBaseObject_Type
#define PyList_CheckExact(op) Py_IS_TYPE((op), &PyList_Type)
#define PyDictProxy_Check(op) Py_IS_TYPE((op), &PyDictProxy_Type)
#define PyMemoryView_Check(op) Py_IS_TYPE((op), &PyMemoryView_Type)
#define PySlice_Check(op) Py_IS_TYPE((op), &PySlice_Type)
#define PyUnicode_CheckExact(op) Py_IS_TYPE((op), &PyUnicode_Type)
#define PyCode_Check(op) PyObject_HasAttrString((PyObject *)(op), "co_code")
#define PyFrame_Check(op) PyObject_HasAttrString((PyObject *)(op), "f_code")
#define PyUnicode_GET_LENGTH(op) PyUnicode_GetLength((PyObject *)(op))
#define PyUnicode_IS_ASCII(op) _molt_pyunicode_is_ascii((PyObject *)(op))
#define PyUnicode_DATA(op) _molt_pyunicode_data((PyObject *)(op))
#define PyMemoryView_GET_BASE(op) Py_None
#define PyMemoryView_GET_BUFFER(op) NULL
#define PyFloat_AS_DOUBLE(op) PyFloat_AsDouble((PyObject *)(op))

static inline PyObject *PyMemoryView_FromObject(PyObject *obj) {
    PyObject *call_args;
    PyObject *result;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    call_args = PyTuple_New(1);
    if (call_args == NULL) {
        return NULL;
    }
    Py_INCREF(obj);
    if (PyTuple_SetItem(call_args, 0, obj) < 0) {
        Py_DECREF(obj);
        Py_DECREF(call_args);
        return NULL;
    }
    result = PyObject_CallObject((PyObject *)&PyMemoryView_Type, call_args);
    Py_DECREF(call_args);
    return result;
}

static inline PyObject *PyMethod_New(PyObject *func, PyObject *self) {
    PyObject *types_module;
    PyObject *method_type;
    PyObject *out;
    if (func == NULL) {
        PyErr_SetString(PyExc_TypeError, "function must not be NULL");
        return NULL;
    }
    if (self == NULL) {
        self = Py_None;
    }
    types_module = PyImport_ImportModule("types");
    if (types_module == NULL) {
        return NULL;
    }
    method_type = PyObject_GetAttrString(types_module, "MethodType");
    Py_DECREF(types_module);
    if (method_type == NULL) {
        return NULL;
    }
    out = PyObject_CallFunctionObjArgs(method_type, func, self, NULL);
    Py_DECREF(method_type);
    return out;
}

static inline int PyObject_TypeCheck(const void *ob, PyTypeObject *type) {
    if (ob == NULL || type == NULL) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(
        _molt_py_handle((PyObject *)ob), _molt_py_handle((PyObject *)type));
}

#define PyType_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyType_Type)

static inline int PyTuple_Check(PyObject *obj) {
    MoltHandle tuple_bits = _molt_builtin_type_handle_cached("tuple");
    if (tuple_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), tuple_bits);
}

static inline int PyList_Check(PyObject *obj) {
    MoltHandle list_bits = _molt_builtin_type_handle_cached("list");
    if (list_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), list_bits);
}

static inline int PyDict_Check(PyObject *obj) {
    MoltHandle dict_bits = _molt_builtin_type_handle_cached("dict");
    if (dict_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), dict_bits);
}

static inline int PyDict_CheckExact(PyObject *obj) {
    return PyDict_Check(obj);
}

static inline int PyUnicode_Check(PyObject *obj) {
    MoltHandle str_bits = _molt_builtin_type_handle_cached("str");
    if (str_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), str_bits);
}

static inline int PyBytes_Check(PyObject *obj) {
    MoltHandle bytes_bits = _molt_builtin_type_handle_cached("bytes");
    if (bytes_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), bytes_bits);
}

static inline int PyByteArray_Check(PyObject *obj) {
    MoltHandle bytearray_bits = _molt_builtin_type_handle_cached("bytearray");
    if (bytearray_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), bytearray_bits);
}

static inline int PyBool_Check(PyObject *obj) {
    MoltHandle bool_bits = _molt_builtin_type_handle_cached("bool");
    if (bool_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), bool_bits);
}

static inline int PyLong_Check(PyObject *obj) {
    MoltHandle int_bits = _molt_builtin_type_handle_cached("int");
    if (int_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), int_bits);
}

static inline int PyFloat_Check(PyObject *obj) {
    MoltHandle float_bits = _molt_builtin_type_handle_cached("float");
    if (float_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), float_bits);
}

static inline int PyComplex_Check(PyObject *obj) {
    MoltHandle complex_bits = _molt_builtin_type_handle_cached("complex");
    if (complex_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), complex_bits);
}

static inline double PyComplex_RealAsDouble(PyObject *obj) {
    PyObject *real_obj;
    double out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "complex object must not be NULL");
        return -1.0;
    }
    real_obj = PyObject_GetAttrString(obj, "real");
    if (real_obj == NULL) {
        return -1.0;
    }
    out = PyFloat_AsDouble(real_obj);
    Py_DECREF(real_obj);
    return out;
}

static inline double PyComplex_ImagAsDouble(PyObject *obj) {
    PyObject *imag_obj;
    double out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "complex object must not be NULL");
        return -1.0;
    }
    imag_obj = PyObject_GetAttrString(obj, "imag");
    if (imag_obj == NULL) {
        return -1.0;
    }
    out = PyFloat_AsDouble(imag_obj);
    Py_DECREF(imag_obj);
    return out;
}

static inline Py_complex PyComplex_AsCComplex(PyObject *obj) {
    Py_complex out;
    out.real = PyComplex_RealAsDouble(obj);
    out.imag = PyComplex_ImagAsDouble(obj);
    return out;
}

static inline PyObject *PyComplex_FromCComplex(Py_complex value) {
    PyObject *real_obj = PyFloat_FromDouble(value.real);
    PyObject *imag_obj;
    PyObject *out;
    if (real_obj == NULL) {
        return NULL;
    }
    imag_obj = PyFloat_FromDouble(value.imag);
    if (imag_obj == NULL) {
        Py_DECREF(real_obj);
        return NULL;
    }
    out = PyObject_CallFunctionObjArgs((PyObject *)&PyComplex_Type, real_obj, imag_obj, NULL);
    Py_DECREF(imag_obj);
    Py_DECREF(real_obj);
    return out;
}

static inline int PySet_Check(PyObject *obj) {
    MoltHandle set_bits = _molt_builtin_type_handle_cached("set");
    if (set_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), set_bits);
}

static inline int PyFrozenSet_Check(PyObject *obj) {
    MoltHandle frozenset_bits = _molt_builtin_type_handle_cached("frozenset");
    if (frozenset_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), frozenset_bits);
}

static inline int PyAnySet_Check(PyObject *obj) {
    return PySet_Check(obj) || PyFrozenSet_Check(obj);
}

static inline int PyTuple_CheckExact(PyObject *obj) {
    MoltHandle tuple_bits = _molt_builtin_type_handle_cached("tuple");
    if (tuple_bits == 0) {
        return 0;
    }
    return _molt_py_handle((PyObject *)Py_TYPE(obj)) == tuple_bits;
}

static inline int PyLong_CheckExact(PyObject *obj) {
    MoltHandle int_bits = _molt_builtin_type_handle_cached("int");
    if (int_bits == 0) {
        return 0;
    }
    return _molt_py_handle((PyObject *)Py_TYPE(obj)) == int_bits;
}

static inline int PyFloat_CheckExact(PyObject *obj) {
    MoltHandle float_bits = _molt_builtin_type_handle_cached("float");
    if (float_bits == 0) {
        return 0;
    }
    return _molt_py_handle((PyObject *)Py_TYPE(obj)) == float_bits;
}

static inline int PyComplex_CheckExact(PyObject *obj) {
    MoltHandle complex_bits = _molt_builtin_type_handle_cached("complex");
    if (complex_bits == 0) {
        return 0;
    }
    return _molt_py_handle((PyObject *)Py_TYPE(obj)) == complex_bits;
}

static inline int PyType_IsSubtype(PyTypeObject *a, PyTypeObject *b) {
    if (a == NULL || b == NULL) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(
        _molt_py_handle((PyObject *)a), _molt_py_handle((PyObject *)b));
}

static inline int PySequence_Check(PyObject *obj) {
    int has_getitem;
    if (obj == NULL) {
        return 0;
    }
    has_getitem = PyObject_HasAttrString(obj, "__getitem__");
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return has_getitem;
}

static inline int PyObject_AsFileDescriptor(PyObject *obj) {
    long fd = PyLong_AsLong(obj);
    if (molt_err_pending() != 0) {
        return -1;
    }
    if (fd < INT_MIN || fd > INT_MAX) {
        PyErr_SetString(PyExc_OverflowError, "file descriptor out of range");
        return -1;
    }
    return (int)fd;
}

static inline int PyCallable_Check(PyObject *obj) {
    int has_call;
    if (obj == NULL) {
        return 0;
    }
    has_call = PyObject_HasAttrString(obj, "__call__");
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return has_call;
}

static inline int PyIter_Check(PyObject *obj) {
    int has_next;
    if (obj == NULL) {
        return 0;
    }
    has_next = PyObject_HasAttrString(obj, "__next__");
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return has_next;
}

static inline PyObject *PyObject_GetIter(PyObject *obj) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_object_get_iter(_molt_py_handle(obj)));
}

static inline PyObject *PyIter_Next(PyObject *obj) {
    MoltHandle out = molt_none();
    int32_t rc;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "iterator must not be NULL");
        return NULL;
    }
    rc = molt_iterator_next(_molt_py_handle(obj), &out);
    if (rc <= 0) {
        return NULL;
    }
    return _molt_pyobject_from_result(out);
}

static inline Py_ssize_t PyObject_Length(PyObject *obj) {
    PyObject *len_fn;
    PyObject *len_obj;
    Py_ssize_t result;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return -1;
    }
    len_fn = PyObject_GetAttrString(obj, "__len__");
    if (len_fn == NULL) {
        return -1;
    }
    len_obj = PyObject_CallObject(len_fn, NULL);
    Py_DECREF(len_fn);
    if (len_obj == NULL) {
        return -1;
    }
    result = PyLong_AsSsize_t(len_obj);
    Py_DECREF(len_obj);
    return result;
}

static inline Py_ssize_t PyObject_Size(PyObject *obj) {
    return PyObject_Length(obj);
}

static inline Py_ssize_t PyObject_LengthHint(PyObject *obj, Py_ssize_t defaultvalue) {
    Py_ssize_t length = PyObject_Length(obj);
    if (length >= 0) {
        return length;
    }
    PyErr_Clear();
    if (defaultvalue < 0) {
        PyErr_SetString(PyExc_ValueError, "default length hint must be >= 0");
        return -1;
    }
    return defaultvalue;
}

static inline int PyObject_Not(PyObject *obj) {
    int truthy = PyObject_IsTrue(obj);
    if (truthy < 0) {
        return -1;
    }
    return truthy == 0;
}

static inline PyObject *PyObject_SelfIter(PyObject *obj) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    Py_INCREF(obj);
    return obj;
}

static inline PyObject *PyObject_Type(PyObject *obj) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "object must not be NULL");
        return NULL;
    }
    return PyObject_GetAttrString(obj, "__class__");
}

static inline int PyObject_IsInstance(PyObject *obj, PyObject *cls) {
    MoltHandle cls_type_bits = _molt_builtin_type_handle_cached("type");
    if (obj == NULL || cls == NULL) {
        PyErr_SetString(PyExc_TypeError, "object and class must not be NULL");
        return -1;
    }
    if (cls_type_bits != 0
        && !_molt_pyarg_object_matches_type(_molt_py_handle(cls), cls_type_bits)) {
        PyErr_SetString(PyExc_TypeError, "second argument must be a type");
        return -1;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), _molt_py_handle(cls));
}

static inline int PyObject_IsSubclass(PyObject *derived, PyObject *cls) {
    PyObject *builtins_module;
    PyObject *issubclass_fn;
    PyObject *call_args;
    PyObject *result_obj;
    int truthy;
    if (derived == NULL || cls == NULL) {
        PyErr_SetString(PyExc_TypeError, "derived and class must not be NULL");
        return -1;
    }
    builtins_module = PyImport_ImportModule("builtins");
    if (builtins_module == NULL) {
        return -1;
    }
    issubclass_fn = PyObject_GetAttrString(builtins_module, "issubclass");
    Py_DECREF(builtins_module);
    if (issubclass_fn == NULL) {
        return -1;
    }
    call_args = PyTuple_Pack(2, derived, cls);
    if (call_args == NULL) {
        Py_DECREF(issubclass_fn);
        return -1;
    }
    result_obj = PyObject_CallObject(issubclass_fn, call_args);
    Py_DECREF(call_args);
    Py_DECREF(issubclass_fn);
    if (result_obj == NULL) {
        return -1;
    }
    truthy = PyObject_IsTrue(result_obj);
    Py_DECREF(result_obj);
    return truthy;
}

static inline int PyNumber_Check(PyObject *obj) {
    if (obj == NULL) {
        return 0;
    }
    if (PyBool_Check(obj) || PyLong_Check(obj) || PyFloat_Check(obj) || PyComplex_Check(obj)) {
        return 1;
    }
    if (PyObject_HasAttrString(obj, "__index__") > 0) {
        return 1;
    }
    PyErr_Clear();
    if (PyObject_HasAttrString(obj, "__int__") > 0) {
        return 1;
    }
    PyErr_Clear();
    if (PyObject_HasAttrString(obj, "__float__") > 0) {
        return 1;
    }
    PyErr_Clear();
    return 0;
}

static inline PyObject *PySequence_Fast(PyObject *obj, const char *msg) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, msg != NULL ? msg : "expected a sequence");
        return NULL;
    }
    if (PyList_Check(obj) || PyTuple_Check(obj)) {
        Py_INCREF(obj);
        return obj;
    }
    if (!PySequence_Check(obj)) {
        PyErr_SetString(PyExc_TypeError, msg != NULL ? msg : "expected a sequence");
        return NULL;
    }
    {
        MoltHandle list_type = _molt_builtin_type_handle_cached("list");
        MoltHandle args_value = _molt_py_handle(obj);
        MoltHandle args_bits = molt_tuple_from_array(&args_value, 1);
        PyObject *out;
        if (list_type == 0 || args_bits == 0 || molt_err_pending() != 0) {
            if (args_bits != 0 && args_bits != molt_none()) {
                molt_handle_decref(args_bits);
            }
            return NULL;
        }
        out = _molt_pyobject_from_result(molt_object_call(list_type, args_bits, molt_none()));
        molt_handle_decref(args_bits);
        return out;
    }
}

#define PySequence_Fast_GET_SIZE(obj) PySequence_Size((PyObject *)(obj))
static inline PyObject *_molt_sequence_fast_get_item_borrowed(PyObject *seq, Py_ssize_t index) {
    PyObject *item = PySequence_GetItem(seq, index);
    if (item == NULL) {
        return NULL;
    }
    Py_DECREF(item);
    return item;
}
#define PySequence_Fast_GET_ITEM(obj, index)                                       \
    _molt_sequence_fast_get_item_borrowed((PyObject *)(obj), (index))
#define PySequence_Fast_ITEMS(obj) ((PyObject **)NULL)

static inline int _molt_rich_compare_call_dunder(
    PyObject *lhs,
    PyObject *rhs,
    const char *dunder_name
) {
    PyObject *dunder;
    MoltHandle arg_bits;
    MoltHandle args_bits;
    PyObject *args_obj;
    PyObject *out;
    int truthy;
    dunder = PyObject_GetAttrString(lhs, dunder_name);
    if (dunder == NULL) {
        return -1;
    }
    arg_bits = _molt_py_handle(rhs);
    args_bits = molt_tuple_from_array(&arg_bits, 1);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(dunder);
        return -1;
    }
    args_obj = _molt_pyobject_from_handle(args_bits);
    out = PyObject_CallObject(dunder, args_obj);
    molt_handle_decref(args_bits);
    Py_DECREF(dunder);
    if (out == NULL) {
        return -1;
    }
    truthy = PyObject_IsTrue(out);
    Py_DECREF(out);
    return truthy;
}

static inline int PyObject_RichCompareBool(PyObject *v, PyObject *w, int op) {
    switch (op) {
        case Py_EQ:
            return molt_object_equal(_molt_py_handle(v), _molt_py_handle(w));
        case Py_NE:
            return molt_object_not_equal(_molt_py_handle(v), _molt_py_handle(w));
        case Py_LT:
            return _molt_rich_compare_call_dunder(v, w, "__lt__");
        case Py_LE:
            return _molt_rich_compare_call_dunder(v, w, "__le__");
        case Py_GT:
            return _molt_rich_compare_call_dunder(v, w, "__gt__");
        case Py_GE:
            return _molt_rich_compare_call_dunder(v, w, "__ge__");
        default:
            PyErr_SetString(PyExc_ValueError, "invalid rich-compare opcode");
            return -1;
    }
}

static inline PyObject *PyObject_RichCompare(PyObject *v, PyObject *w, int op) {
    int rc = PyObject_RichCompareBool(v, w, op);
    if (rc < 0) {
        return NULL;
    }
    if (rc != 0) {
        Py_INCREF(Py_True);
        return Py_True;
    }
    Py_INCREF(Py_False);
    return Py_False;
}

static inline PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...) {
    va_list ap;
    MoltHandle *items = NULL;
    size_t capacity = 0;
    size_t len = 0;
    MoltHandle args_bits;
    PyObject *out;
    if (callable == NULL) {
        PyErr_SetString(PyExc_TypeError, "callable must not be NULL");
        return NULL;
    }
    capacity = 8;
    items = (MoltHandle *)PyMem_Malloc(sizeof(MoltHandle) * capacity);
    if (items == NULL) {
        return NULL;
    }
    va_start(ap, callable);
    for (;;) {
        PyObject *arg = va_arg(ap, PyObject *);
        if (arg == NULL) {
            break;
        }
        if (len == capacity) {
            MoltHandle *grown;
            size_t new_capacity = capacity * 2;
            grown = (MoltHandle *)PyMem_Realloc(items, sizeof(MoltHandle) * new_capacity);
            if (grown == NULL) {
                va_end(ap);
                PyMem_Free(items);
                return NULL;
            }
            items = grown;
            capacity = new_capacity;
        }
        items[len++] = _molt_py_handle(arg);
    }
    va_end(ap);
    args_bits = molt_tuple_from_array(items, (uint64_t)len);
    PyMem_Free(items);
    if (args_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    out = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle(callable), args_bits, molt_none()));
    molt_handle_decref(args_bits);
    return out;
}

static inline void _molt_buildvalue_skip_separators(const char **cursor) {
    while (**cursor == ' '
        || **cursor == '\t'
        || **cursor == '\n'
        || **cursor == '\r'
        || **cursor == ',') {
        (*cursor)++;
    }
}

static inline int _molt_buildvalue_push(
    MoltHandle **items,
    size_t *capacity,
    size_t *len,
    MoltHandle value
) {
    if (*len == *capacity) {
        size_t new_capacity = (*capacity == 0) ? 8 : (*capacity * 2);
        MoltHandle *grown =
            (MoltHandle *)PyMem_Realloc(*items, sizeof(MoltHandle) * new_capacity);
        if (grown == NULL) {
            return 0;
        }
        *items = grown;
        *capacity = new_capacity;
    }
    (*items)[(*len)++] = value;
    return 1;
}

static inline int _molt_buildvalue_parse_item(
    const char **cursor,
    va_list *ap,
    MoltHandle *out_bits
);

static inline int _molt_buildvalue_parse_sequence(
    const char **cursor,
    va_list *ap,
    char close_ch,
    int as_list,
    MoltHandle *out_bits
) {
    MoltHandle *items = NULL;
    size_t capacity = 0;
    size_t len = 0;
    MoltHandle built = 0;
    int ok = 0;
    for (;;) {
        MoltHandle item = 0;
        _molt_buildvalue_skip_separators(cursor);
        if (**cursor == close_ch) {
            (*cursor)++;
            break;
        }
        if (**cursor == '\0') {
            PyErr_SetString(PyExc_TypeError, "unterminated container format in Py_BuildValue");
            goto done;
        }
        if (!_molt_buildvalue_parse_item(cursor, ap, &item)) {
            goto done;
        }
        if (!_molt_buildvalue_push(&items, &capacity, &len, item)) {
            if (item != 0 && item != molt_none()) {
                molt_handle_decref(item);
            }
            goto done;
        }
        _molt_buildvalue_skip_separators(cursor);
    }
    built = as_list ? molt_list_from_array(items, (uint64_t)len)
                    : molt_tuple_from_array(items, (uint64_t)len);
    if (built == 0 || molt_err_pending() != 0) {
        goto done;
    }
    *out_bits = built;
    ok = 1;
done:
    if (items != NULL) {
        size_t i = 0;
        while (i < len) {
            if (items[i] != 0 && items[i] != molt_none()) {
                molt_handle_decref(items[i]);
            }
            i++;
        }
    }
    PyMem_Free(items);
    return ok;
}

static inline int _molt_buildvalue_parse_dict(
    const char **cursor,
    va_list *ap,
    MoltHandle *out_bits
) {
    MoltHandle *keys = NULL;
    MoltHandle *values = NULL;
    size_t capacity = 0;
    size_t len = 0;
    MoltHandle built = 0;
    int ok = 0;
    for (;;) {
        MoltHandle key_item = 0;
        MoltHandle value_item = 0;
        _molt_buildvalue_skip_separators(cursor);
        if (**cursor == '}') {
            (*cursor)++;
            break;
        }
        if (**cursor == '\0') {
            PyErr_SetString(PyExc_TypeError, "unterminated dict format in Py_BuildValue");
            goto done;
        }
        if (!_molt_buildvalue_parse_item(cursor, ap, &key_item)) {
            goto done;
        }
        _molt_buildvalue_skip_separators(cursor);
        if (**cursor == ':') {
            (*cursor)++;
        }
        if (!_molt_buildvalue_parse_item(cursor, ap, &value_item)) {
            if (key_item != 0 && key_item != molt_none()) {
                molt_handle_decref(key_item);
            }
            goto done;
        }
        if (len == capacity) {
            size_t new_capacity = capacity == 0 ? 8 : (capacity * 2);
            MoltHandle *grown_keys =
                (MoltHandle *)PyMem_Realloc(keys, sizeof(MoltHandle) * new_capacity);
            MoltHandle *grown_values =
                (MoltHandle *)PyMem_Realloc(values, sizeof(MoltHandle) * new_capacity);
            if (grown_keys == NULL || grown_values == NULL) {
                if (grown_keys != NULL) {
                    keys = grown_keys;
                }
                if (grown_values != NULL) {
                    values = grown_values;
                }
                if (key_item != 0 && key_item != molt_none()) {
                    molt_handle_decref(key_item);
                }
                if (value_item != 0 && value_item != molt_none()) {
                    molt_handle_decref(value_item);
                }
                goto done;
            }
            keys = grown_keys;
            values = grown_values;
            capacity = new_capacity;
        }
        keys[len] = key_item;
        values[len] = value_item;
        len++;
        _molt_buildvalue_skip_separators(cursor);
    }
    built = molt_dict_from_pairs(keys, values, (uint64_t)len);
    if (built == 0 || molt_err_pending() != 0) {
        goto done;
    }
    *out_bits = built;
    ok = 1;
done:
    if (keys != NULL) {
        size_t i = 0;
        while (i < len) {
            if (keys[i] != 0 && keys[i] != molt_none()) {
                molt_handle_decref(keys[i]);
            }
            if (values != NULL && values[i] != 0 && values[i] != molt_none()) {
                molt_handle_decref(values[i]);
            }
            i++;
        }
    }
    PyMem_Free(values);
    PyMem_Free(keys);
    return ok;
}

static inline int _molt_buildvalue_parse_item(
    const char **cursor,
    va_list *ap,
    MoltHandle *out_bits
) {
    char code;
    _molt_buildvalue_skip_separators(cursor);
    code = **cursor;
    if (code == '\0') {
        PyErr_SetString(PyExc_TypeError, "unexpected end of format in Py_BuildValue");
        return 0;
    }
    if (code == '(') {
        (*cursor)++;
        return _molt_buildvalue_parse_sequence(cursor, ap, ')', 0, out_bits);
    }
    if (code == '[') {
        (*cursor)++;
        return _molt_buildvalue_parse_sequence(cursor, ap, ']', 1, out_bits);
    }
    if (code == '{') {
        (*cursor)++;
        return _molt_buildvalue_parse_dict(cursor, ap, out_bits);
    }
    (*cursor)++;
    switch (code) {
        case 'O': {
            PyObject *obj = va_arg(*ap, PyObject *);
            if (obj == NULL) {
                PyErr_SetString(PyExc_TypeError, "Py_BuildValue 'O' received NULL");
                return 0;
            }
            *out_bits = _molt_py_handle(obj);
            molt_handle_incref(*out_bits);
            return 1;
        }
        case 'N': {
            PyObject *obj = va_arg(*ap, PyObject *);
            if (obj == NULL) {
                PyErr_SetString(PyExc_TypeError, "Py_BuildValue 'N' received NULL");
                return 0;
            }
            *out_bits = _molt_py_handle(obj);
            return 1;
        }
        case 'i': {
            int value = va_arg(*ap, int);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'l': {
            long value = va_arg(*ap, long);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'n': {
            Py_ssize_t value = va_arg(*ap, Py_ssize_t);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'k': {
            unsigned long value = va_arg(*ap, unsigned long);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'K': {
            unsigned long long value = va_arg(*ap, unsigned long long);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'L': {
            long long value = va_arg(*ap, long long);
            *out_bits = molt_int_from_i64((int64_t)value);
            return molt_err_pending() == 0;
        }
        case 'd': {
            double value = va_arg(*ap, double);
            *out_bits = molt_float_from_f64(value);
            return molt_err_pending() == 0;
        }
        case 'f': {
            double value = va_arg(*ap, double);
            *out_bits = molt_float_from_f64(value);
            return molt_err_pending() == 0;
        }
        case 'p': {
            int value = va_arg(*ap, int);
            *out_bits = molt_bool_from_i32(value != 0 ? 1 : 0);
            return molt_err_pending() == 0;
        }
        case 'c': {
            int ch = va_arg(*ap, int);
            unsigned char byte = (unsigned char)ch;
            *out_bits = molt_bytes_from(&byte, 1);
            return molt_err_pending() == 0;
        }
        case 's':
        case 'z':
        case 'y': {
            const char *text = va_arg(*ap, const char *);
            int has_len = (**cursor == '#');
            uint64_t len = 0;
            if (text == NULL && code == 'z') {
                *out_bits = molt_none();
                return 1;
            }
            if (text == NULL) {
                PyErr_SetString(PyExc_TypeError, "Py_BuildValue string argument is NULL");
                return 0;
            }
            if (has_len) {
                Py_ssize_t value_len;
                (*cursor)++;
                value_len = va_arg(*ap, Py_ssize_t);
                if (value_len < 0) {
                    PyErr_SetString(PyExc_ValueError, "Py_BuildValue received negative length");
                    return 0;
                }
                len = (uint64_t)value_len;
            } else {
                len = (uint64_t)strlen(text);
            }
            if (code == 'y') {
                *out_bits = molt_bytes_from((const uint8_t *)text, len);
            } else {
                *out_bits = molt_string_from((const uint8_t *)text, len);
            }
            return molt_err_pending() == 0;
        }
        default:
            PyErr_Format(
                PyExc_TypeError,
                "unsupported format unit '%c' in Py_BuildValue",
                code);
            return 0;
    }
}

static inline PyObject *_molt_buildvalue_from_va_list(const char *format, va_list *ap) {
    const char *cursor;
    MoltHandle *items = NULL;
    size_t capacity = 0;
    size_t len = 0;
    PyObject *out = NULL;
    if (format == NULL) {
        PyErr_SetString(PyExc_TypeError, "format must not be NULL");
        return NULL;
    }
    cursor = format;
    for (;;) {
        MoltHandle item = 0;
        _molt_buildvalue_skip_separators(&cursor);
        if (*cursor == '\0') {
            break;
        }
        if (!_molt_buildvalue_parse_item(&cursor, ap, &item)) {
            goto done;
        }
        if (!_molt_buildvalue_push(&items, &capacity, &len, item)) {
            if (item != 0 && item != molt_none()) {
                molt_handle_decref(item);
            }
            goto done;
        }
    }
    if (len == 0) {
        Py_INCREF(Py_None);
        out = Py_None;
        goto done;
    }
    if (len == 1) {
        out = _molt_pyobject_from_handle(items[0]);
        items[0] = 0;
        goto done;
    }
    {
        MoltHandle tuple_bits = molt_tuple_from_array(items, (uint64_t)len);
        out = _molt_pyobject_from_result(tuple_bits);
    }
done:
    if (items != NULL) {
        size_t i = 0;
        while (i < len) {
            if (items[i] != 0 && items[i] != molt_none()) {
                molt_handle_decref(items[i]);
            }
            i++;
        }
    }
    PyMem_Free(items);
    return out;
}

static inline PyObject *Py_BuildValue(const char *format, ...) {
    va_list ap;
    PyObject *out;
    va_start(ap, format);
    out = _molt_buildvalue_from_va_list(format, &ap);
    va_end(ap);
    return out;
}

static inline PyObject *_molt_call_with_format_args(
    PyObject *callable,
    const char *format,
    va_list *ap
) {
    PyObject *args_obj;
    PyObject *call_args = NULL;
    PyObject *out;
    MoltHandle arg_bits;
    MoltHandle tuple_bits;
    if (format == NULL || format[0] == '\0') {
        return PyObject_CallObject(callable, NULL);
    }
    args_obj = _molt_buildvalue_from_va_list(format, ap);
    if (args_obj == NULL) {
        return NULL;
    }
    if (PyTuple_Check(args_obj)) {
        call_args = args_obj;
        Py_INCREF(call_args);
    } else {
        arg_bits = _molt_py_handle(args_obj);
        tuple_bits = molt_tuple_from_array(&arg_bits, 1);
        if (tuple_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(args_obj);
            return NULL;
        }
        call_args = _molt_pyobject_from_handle(tuple_bits);
    }
    out = PyObject_CallObject(callable, call_args);
    Py_DECREF(call_args);
    Py_DECREF(args_obj);
    return out;
}

static inline PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...) {
    va_list ap;
    PyObject *out;
    va_start(ap, format);
    out = _molt_call_with_format_args(callable, format, &ap);
    va_end(ap);
    return out;
}

static inline PyObject *PyObject_CallMethod(
    PyObject *obj,
    const char *name,
    const char *format,
    ...
) {
    va_list ap;
    PyObject *method;
    PyObject *out;
    if (obj == NULL || name == NULL) {
        PyErr_SetString(PyExc_TypeError, "object and method name must not be NULL");
        return NULL;
    }
    method = PyObject_GetAttrString(obj, name);
    if (method == NULL) {
        return NULL;
    }
    va_start(ap, format);
    out = _molt_call_with_format_args(method, format, &ap);
    va_end(ap);
    Py_DECREF(method);
    return out;
}

static inline PyObject *Py_GenericAlias(PyObject *origin, PyObject *args) {
    PyObject *types_module;
    PyObject *generic_alias_type;
    PyObject *alias;
    if (origin == NULL || args == NULL) {
        PyErr_SetString(PyExc_TypeError, "origin and args must not be NULL");
        return NULL;
    }
    types_module = PyImport_ImportModule("types");
    if (types_module == NULL) {
        return NULL;
    }
    generic_alias_type = PyObject_GetAttrString(types_module, "GenericAlias");
    Py_DECREF(types_module);
    if (generic_alias_type == NULL) {
        return NULL;
    }
    alias = PyObject_CallFunctionObjArgs(generic_alias_type, origin, args, NULL);
    Py_DECREF(generic_alias_type);
    return alias;
}

static inline PyObject *PyEval_GetBuiltins(void) {
    PyObject *module = PyImport_ImportModule("builtins");
    PyObject *dict;
    if (module == NULL) {
        return NULL;
    }
    dict = PyModule_GetDict(module);
    if (dict != NULL) {
        Py_INCREF(dict);
    }
    Py_DECREF(module);
    return dict;
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

static inline PyObject *PyImport_Import(PyObject *name) {
    const char *module_name = PyUnicode_AsUTF8(name);
    if (module_name == NULL) {
        return NULL;
    }
    return PyImport_ImportModule(module_name);
}

static inline PyObject *PyImport_AddModule(const char *name) {
    return PyImport_ImportModule(name);
}

static inline int _Py_IsFinalizing(void) {
    return 0;
}

static inline int Py_EnterRecursiveCall(const char *where) {
    (void)where;
    PyErr_SetString(
        PyExc_RuntimeError,
        "Py_EnterRecursiveCall is not yet implemented in Molt's C-API layer");
    return -1;
}

static inline void Py_LeaveRecursiveCall(void) {}

static inline PyThreadState *_PyThreadState_UncheckedGet(void) {
    return PyThreadState_Get();
}

static inline PyObject *PySys_GetObject(const char *name) {
    PyObject *sys_module;
    PyObject *value;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "sys attribute name must not be empty");
        return NULL;
    }
    sys_module = PyImport_ImportModule("sys");
    if (sys_module == NULL) {
        return NULL;
    }
    value = PyObject_GetAttrString(sys_module, name);
    Py_DECREF(sys_module);
    if (value == NULL) {
        return NULL;
    }
    Py_DECREF(value);
    return value;
}

static inline int PyContextVar_Get(PyObject *var, PyObject *default_value, PyObject **value) {
    PyObject *result;
    if (value == NULL) {
        PyErr_SetString(PyExc_TypeError, "value output pointer must not be NULL");
        return -1;
    }
    *value = NULL;
    if (var == NULL) {
        PyErr_SetString(PyExc_TypeError, "context variable must not be NULL");
        return -1;
    }
    if (default_value != NULL) {
        result = PyObject_CallMethod(var, "get", "O", default_value);
    } else {
        result = PyObject_CallMethod(var, "get", NULL);
    }
    if (result == NULL) {
        return -1;
    }
    *value = result;
    return 0;
}

static inline int PyContextVar_Set(PyObject *var, PyObject *value) {
    PyObject *token;
    if (var == NULL) {
        PyErr_SetString(PyExc_TypeError, "context variable must not be NULL");
        return -1;
    }
    token = PyObject_CallMethod(var, "set", "O", value != NULL ? value : Py_None);
    if (token == NULL) {
        return -1;
    }
    Py_DECREF(token);
    return 0;
}

static inline PyObject *PyContextVar_New(const char *name, PyObject *default_value) {
    PyObject *contextvars_module;
    PyObject *contextvar_type;
    PyObject *name_obj;
    PyObject *call_args;
    PyObject *result;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "context variable name must not be empty");
        return NULL;
    }
    contextvars_module = PyImport_ImportModule("contextvars");
    if (contextvars_module == NULL) {
        return NULL;
    }
    contextvar_type = PyObject_GetAttrString(contextvars_module, "ContextVar");
    Py_DECREF(contextvars_module);
    if (contextvar_type == NULL) {
        return NULL;
    }
    name_obj = PyUnicode_FromString(name);
    if (name_obj == NULL) {
        Py_DECREF(contextvar_type);
        return NULL;
    }
    if (default_value != NULL) {
        call_args = PyTuple_New(2);
        if (call_args == NULL) {
            Py_DECREF(name_obj);
            Py_DECREF(contextvar_type);
            return NULL;
        }
        Py_INCREF(name_obj);
        if (PyTuple_SetItem(call_args, 0, name_obj) < 0) {
            Py_DECREF(name_obj);
            Py_DECREF(call_args);
            Py_DECREF(contextvar_type);
            return NULL;
        }
        Py_INCREF(default_value);
        if (PyTuple_SetItem(call_args, 1, default_value) < 0) {
            Py_DECREF(default_value);
            Py_DECREF(call_args);
            Py_DECREF(contextvar_type);
            return NULL;
        }
    } else {
        call_args = PyTuple_New(1);
        if (call_args == NULL) {
            Py_DECREF(name_obj);
            Py_DECREF(contextvar_type);
            return NULL;
        }
        Py_INCREF(name_obj);
        if (PyTuple_SetItem(call_args, 0, name_obj) < 0) {
            Py_DECREF(name_obj);
            Py_DECREF(call_args);
            Py_DECREF(contextvar_type);
            return NULL;
        }
    }
    Py_DECREF(name_obj);
    result = PyObject_CallObject(contextvar_type, call_args);
    Py_DECREF(call_args);
    Py_DECREF(contextvar_type);
    return result;
}

static inline PyObject *PyCapsule_New(
    void *pointer,
    const char *name,
    PyCapsule_Destructor destructor
) {
    if (pointer == NULL) {
        PyErr_SetString(PyExc_ValueError, "PyCapsule_New called with NULL pointer");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_capsule_new(
        (uintptr_t)pointer,
        (const uint8_t *)name,
        name != NULL ? (uint64_t)strlen(name) : 0,
        (uintptr_t)destructor
    ));
}

static inline const char *PyCapsule_GetName(PyObject *capsule) {
    uint64_t name_len = 0;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return NULL;
    }
    return (const char *)molt_capsule_get_name_ptr(_molt_py_handle(capsule), &name_len);
}

static inline int PyCapsule_SetName(PyObject *capsule, const char *name) {
    (void)capsule;
    (void)name;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyCapsule_SetName is not yet implemented in Molt's C-API layer");
    return -1;
}

static inline void *PyCapsule_GetPointer(PyObject *capsule, const char *name) {
    uintptr_t raw_ptr;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return NULL;
    }
    raw_ptr = molt_capsule_get_pointer(
        _molt_py_handle(capsule),
        (const uint8_t *)name,
        name != NULL ? (uint64_t)strlen(name) : 0
    );
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)raw_ptr;
}

static inline int PyCapsule_IsValid(PyObject *capsule, const char *name) {
    int rc;
    if (capsule == NULL) {
        PyErr_Clear();
        return 0;
    }
    rc = molt_capsule_is_valid(
        _molt_py_handle(capsule),
        (const uint8_t *)name,
        name != NULL ? (uint64_t)strlen(name) : 0
    );
    if (rc == 0 && molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return rc != 0;
}

static inline int PyCapsule_CheckExact(PyObject *capsule) {
    return PyCapsule_IsValid(capsule, NULL);
}

static inline void *PyCapsule_GetContext(PyObject *capsule) {
    uintptr_t raw_ptr;
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return NULL;
    }
    raw_ptr = molt_capsule_get_context(_molt_py_handle(capsule));
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)raw_ptr;
}

static inline int PyCapsule_SetContext(PyObject *capsule, void *context) {
    if (capsule == NULL) {
        PyErr_SetString(PyExc_TypeError, "capsule must not be NULL");
        return -1;
    }
    return molt_capsule_set_context(_molt_py_handle(capsule), (uintptr_t)context);
}

static inline void *PyCapsule_Import(const char *name, int no_block) {
    uintptr_t ptr;
    (void)no_block;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "capsule import name must not be empty");
        return NULL;
    }
    ptr = molt_capsule_import((const uint8_t *)name, (uint64_t)strlen(name));
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)ptr;
}

typedef struct _molt_pyarg_kw_slot {
    const char *name;
    Py_ssize_t index;
} _molt_pyarg_kw_slot;

static inline size_t _molt_pyarg_hash_cstr(const char *text) {
    size_t hash = (size_t)1469598103934665603ULL;
    const unsigned char *cursor = (const unsigned char *)text;
    while (cursor != NULL && *cursor != '\0') {
        hash ^= (size_t)(*cursor++);
        hash *= (size_t)1099511628211ULL;
    }
    return hash;
}

static inline size_t _molt_pyarg_hash_bytes(const uint8_t *text, size_t len) {
    size_t hash = (size_t)1469598103934665603ULL;
    size_t i = 0;
    while (i < len) {
        hash ^= (size_t)text[i++];
        hash *= (size_t)1099511628211ULL;
    }
    return hash;
}

static inline size_t _molt_pyarg_kw_table_capacity(size_t item_count) {
    size_t cap = 8;
    while (cap < (item_count * 2 + 1)) {
        cap <<= 1;
    }
    return cap;
}

/*
 * Returns:
 *  1 on inserted
 *  0 on duplicate keyword
 * -1 on missing table capacity
 */
static inline int _molt_pyarg_kw_table_insert(
    _molt_pyarg_kw_slot *slots,
    size_t capacity,
    const char *name,
    Py_ssize_t index
) {
    size_t mask;
    size_t slot_idx;
    if (slots == NULL || capacity == 0) {
        return -1;
    }
    mask = capacity - 1;
    slot_idx = _molt_pyarg_hash_cstr(name) & mask;
    while (slots[slot_idx].name != NULL) {
        if (strcmp(slots[slot_idx].name, name) == 0) {
            return 0;
        }
        slot_idx = (slot_idx + 1) & mask;
    }
    slots[slot_idx].name = name;
    slots[slot_idx].index = index;
    return 1;
}

static inline int _molt_pyarg_kw_table_lookup(
    const _molt_pyarg_kw_slot *slots,
    size_t capacity,
    const uint8_t *name,
    size_t name_len,
    Py_ssize_t *index_out
) {
    size_t mask;
    size_t slot_idx;
    if (slots == NULL || capacity == 0 || name == NULL) {
        return 0;
    }
    mask = capacity - 1;
    slot_idx = _molt_pyarg_hash_bytes(name, name_len) & mask;
    while (slots[slot_idx].name != NULL) {
        const char *candidate = slots[slot_idx].name;
        size_t candidate_len = strlen(candidate);
        if (candidate_len == name_len && memcmp(candidate, name, name_len) == 0) {
            if (index_out != NULL) {
                *index_out = slots[slot_idx].index;
            }
            return 1;
        }
        slot_idx = (slot_idx + 1) & mask;
    }
    return 0;
}

static inline int _molt_pyarg_convert_value(
    MoltHandle item_bits,
    int has_value,
    char code,
    const char **cursor,
    va_list *ap,
    const char *api_name
) {
    switch (code) {
        case 'O': {
            int expect_type = (**cursor == '!');
            if (expect_type) {
                PyTypeObject *type_obj = va_arg(*ap, PyTypeObject *);
                PyObject **out = va_arg(*ap, PyObject **);
                (*cursor)++;
                if (!has_value) {
                    break;
                }
                if (type_obj == NULL) {
                    PyErr_SetString(PyExc_TypeError, "O! type object must not be NULL");
                    return 0;
                }
                if (!_molt_pyarg_object_matches_type(
                        item_bits,
                        _molt_py_handle((PyObject *)type_obj))) {
                    PyErr_SetString(PyExc_TypeError, "argument has incorrect type for O!");
                    return 0;
                }
                if (out != NULL) {
                    *out = _molt_pyobject_from_handle(item_bits);
                }
                break;
            }
            {
                PyObject **out = va_arg(*ap, PyObject **);
                if (has_value && out != NULL) {
                    *out = _molt_pyobject_from_handle(item_bits);
                }
            }
            break;
        }
        case 'b': {
            unsigned char *out = va_arg(*ap, unsigned char *);
            if (has_value) {
                uint64_t value = 0;
                if (!_molt_parse_uint64_range_arg(
                        item_bits, UCHAR_MAX, &value, code, api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned char)value;
                }
            }
            break;
        }
        case 'B': {
            unsigned char *out = va_arg(*ap, unsigned char *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_arg(item_bits, &value)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned char)((uint64_t)value);
                }
            }
            break;
        }
        case 'h': {
            short *out = va_arg(*ap, short *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_range_arg(
                        item_bits, SHRT_MIN, SHRT_MAX, &value, code, api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (short)value;
                }
            }
            break;
        }
        case 'H': {
            unsigned short *out = va_arg(*ap, unsigned short *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_arg(item_bits, &value)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned short)((uint64_t)value);
                }
            }
            break;
        }
        case 'i': {
            int *out = va_arg(*ap, int *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_range_arg(
                        item_bits, INT_MIN, INT_MAX, &value, code, api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (int)value;
                }
            }
            break;
        }
        case 'I': {
            unsigned int *out = va_arg(*ap, unsigned int *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_arg(item_bits, &value)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned int)((uint64_t)value);
                }
            }
            break;
        }
        case 'l': {
            long *out = va_arg(*ap, long *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_range_arg(
                        item_bits, LONG_MIN, LONG_MAX, &value, code, api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (long)value;
                }
            }
            break;
        }
        case 'k': {
            unsigned long *out = va_arg(*ap, unsigned long *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_arg(item_bits, &value)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned long)((uint64_t)value);
                }
            }
            break;
        }
        case 'L': {
            long long *out = va_arg(*ap, long long *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_range_arg(
                        item_bits, LLONG_MIN, LLONG_MAX, &value, code, api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (long long)value;
                }
            }
            break;
        }
        case 'K': {
            unsigned long long *out = va_arg(*ap, unsigned long long *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_arg(item_bits, &value)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (unsigned long long)((uint64_t)value);
                }
            }
            break;
        }
        case 'n': {
            Py_ssize_t *out = va_arg(*ap, Py_ssize_t *);
            if (has_value) {
                int64_t value = 0;
                if (!_molt_parse_int64_range_arg(
                        item_bits,
                        (int64_t)INTPTR_MIN,
                        (int64_t)INTPTR_MAX,
                        &value,
                        code,
                        api_name)) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (Py_ssize_t)value;
                }
            }
            break;
        }
        case 'c': {
            char *out = va_arg(*ap, char *);
            if (!has_value) {
                break;
            }
            {
                uint64_t len = 0;
                const uint8_t *ptr = molt_bytes_as_ptr(item_bits, &len);
                if ((ptr == NULL || molt_err_pending() != 0) && PyErr_ExceptionMatches(PyExc_TypeError)) {
                    PyErr_Clear();
                    ptr = (const uint8_t *)molt_bytearray_as_ptr(item_bits, &len);
                }
                if (ptr == NULL || molt_err_pending() != 0) {
                    return 0;
                }
                if (len != 1) {
                    PyErr_SetString(PyExc_TypeError, "c format requires bytes-like object of length 1");
                    return 0;
                }
                if (out != NULL) {
                    *out = (char)ptr[0];
                }
            }
            break;
        }
        case 'd': {
            double *out = va_arg(*ap, double *);
            if (has_value) {
                double value = molt_float_as_f64(item_bits);
                if (molt_err_pending() != 0) {
                    return 0;
                }
                if (out != NULL) {
                    *out = value;
                }
            }
            break;
        }
        case 'f': {
            float *out = va_arg(*ap, float *);
            if (has_value) {
                double value = molt_float_as_f64(item_bits);
                if (molt_err_pending() != 0) {
                    return 0;
                }
                if (out != NULL) {
                    *out = (float)value;
                }
            }
            break;
        }
        case 'p': {
            int *out = va_arg(*ap, int *);
            if (has_value) {
                int truth = molt_object_truthy(item_bits);
                if (truth < 0 || molt_err_pending() != 0) {
                    return 0;
                }
                if (out != NULL) {
                    *out = truth != 0;
                }
            }
            break;
        }
        case 's':
        case 'z': {
            const char **out_str = va_arg(*ap, const char **);
            int with_len = (**cursor == '#');
            Py_ssize_t *out_len = NULL;
            if (with_len) {
                out_len = va_arg(*ap, Py_ssize_t *);
                (*cursor)++;
            }
            if (!has_value) {
                break;
            }
            if (code == 'z' && item_bits == molt_none()) {
                if (out_str != NULL) {
                    *out_str = NULL;
                }
                if (out_len != NULL) {
                    *out_len = 0;
                }
                break;
            }
            {
                uint64_t len = 0;
                const uint8_t *ptr = molt_string_as_ptr(item_bits, &len);
                if (ptr == NULL || molt_err_pending() != 0) {
                    return 0;
                }
                if (!with_len && memchr(ptr, '\0', (size_t)len) != NULL) {
                    PyErr_SetString(PyExc_ValueError, "embedded NUL in string argument");
                    return 0;
                }
                if (out_str != NULL) {
                    *out_str = (const char *)ptr;
                }
                if (out_len != NULL) {
                    *out_len = (Py_ssize_t)len;
                }
            }
            break;
        }
        case 'y': {
            const char **out_str;
            Py_ssize_t *out_len;
            if (**cursor != '#') {
                PyErr_Format(PyExc_TypeError, "only y# is supported by %s", api_name);
                return 0;
            }
            (*cursor)++;
            out_str = va_arg(*ap, const char **);
            out_len = va_arg(*ap, Py_ssize_t *);
            if (!has_value) {
                break;
            }
            {
                uint64_t len = 0;
                const uint8_t *ptr = molt_bytes_as_ptr(item_bits, &len);
                if (ptr == NULL || molt_err_pending() != 0) {
                    return 0;
                }
                if (out_str != NULL) {
                    *out_str = (const char *)ptr;
                }
                if (out_len != NULL) {
                    *out_len = (Py_ssize_t)len;
                }
            }
            break;
        }
        default:
            PyErr_Format(
                PyExc_TypeError,
                "unsupported format unit '%c' in %s",
                code,
                api_name);
            return 0;
    }
    return 1;
}

/*
 * Minimal O(n) parser for common extension fast paths.
 * Supported format units: O, O!, b, B, h, H, i, I, l, k, L, K, n, c, d, f, p,
 * s, s#, z, z#, y#, and markers '|', '$', ':', ';'.
 */
static inline int _molt_pyarg_parse_tuple_va(PyObject *args, const char *format, va_list *ap) {
    int64_t argc;
    int64_t arg_index = 0;
    int optional = 0;
    const char *cursor = format;
    if (args == NULL || format == NULL) {
        PyErr_SetString(PyExc_TypeError, "args/format must not be NULL");
        return 0;
    }
    argc = (int64_t)molt_sequence_length(_molt_py_handle(args));
    if (argc < 0 || molt_err_pending() != 0) {
        return 0;
    }
    while (*cursor != '\0') {
        char code = *cursor++;
        MoltHandle item_bits;
        if (code == ' ' || code == '\t' || code == '\n' || code == '\r') {
            continue;
        }
        if (code == '|') {
            optional = 1;
            continue;
        }
        if (code == '$') {
            continue;
        }
        if (code == ':' || code == ';') {
            break;
        }
        if (arg_index >= argc) {
            if (optional) {
                if (!_molt_pyarg_convert_value(
                        0, 0, code, &cursor, ap, "PyArg_ParseTuple")) {
                    return 0;
                }
                continue;
            }
            PyErr_SetString(PyExc_TypeError, "not enough arguments for format");
            return 0;
        }
        if (!_molt_pyarg_get_positional_item(args, arg_index, &item_bits)) {
            return 0;
        }
        arg_index++;
        if (!_molt_pyarg_convert_value(item_bits, 1, code, &cursor, ap, "PyArg_ParseTuple")) {
            molt_handle_decref(item_bits);
            return 0;
        }
        molt_handle_decref(item_bits);
    }
    if (arg_index < argc) {
        PyErr_SetString(PyExc_TypeError, "too many positional arguments");
        return 0;
    }
    return 1;
}

static inline int PyArg_ParseTuple(PyObject *args, const char *format, ...) {
    int out;
    va_list ap;
    va_start(ap, format);
    out = _molt_pyarg_parse_tuple_va(args, format, &ap);
    va_end(ap);
    return out;
}

static inline int PyArg_UnpackTuple(
    PyObject *args,
    const char *name,
    Py_ssize_t min,
    Py_ssize_t max,
    ...
) {
    const char *api_name = (name != NULL && name[0] != '\0') ? name : "function";
    Py_ssize_t argc;
    Py_ssize_t i;
    va_list ap;
    if (args == NULL) {
        PyErr_SetString(PyExc_TypeError, "args must not be NULL");
        return 0;
    }
    if (!PyTuple_Check(args)) {
        PyErr_Format(PyExc_TypeError, "%s argument list must be a tuple", api_name);
        return 0;
    }
    if (min < 0 || max < min) {
        PyErr_Format(
            PyExc_SystemError,
            "%s called with invalid min/max bounds",
            api_name);
        return 0;
    }
    argc = PyTuple_Size(args);
    if (argc < min || argc > max) {
        PyErr_Format(
            PyExc_TypeError,
            "%s expected %zd to %zd arguments, got %zd",
            api_name,
            min,
            max,
            argc);
        return 0;
    }
    va_start(ap, max);
    for (i = 0; i < max; i++) {
        PyObject **out_item = va_arg(ap, PyObject **);
        if (out_item == NULL) {
            continue;
        }
        if (i < argc) {
            *out_item = PyTuple_GetItem(args, i);
        } else {
            *out_item = NULL;
        }
    }
    va_end(ap);
    return 1;
}

static inline int PyArg_VaParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    va_list vargs
) {
    (void)args;
    (void)kwargs;
    (void)format;
    (void)kwlist;
    (void)vargs;
    PyErr_SetString(
        PyExc_RuntimeError,
        "PyArg_VaParseTupleAndKeywords is not yet implemented in Molt");
    return 0;
}

static inline int PyArg_ParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    ...
) {
    int out = 0;
    int64_t argc;
    int64_t arg_index = 0;
    int optional = 0;
    int keyword_only = 0;
    int param_index = 0;
    int format_param_count = 0;
    Py_ssize_t kw_size = 0;
    unsigned char *kw_present = NULL;
    MoltHandle *kw_values = NULL;
    _molt_pyarg_kw_slot *kw_slots = NULL;
    size_t kw_slot_capacity = 0;
    MoltHandle kw_keys_bits = 0;
    int va_started = 0;
    const char *cursor = format;
    const char *scan = format;
    va_list ap;
    if (args == NULL || format == NULL || kwlist == NULL) {
        PyErr_SetString(PyExc_TypeError, "args/format/kwlist must not be NULL");
        return 0;
    }
    argc = (int64_t)molt_sequence_length(_molt_py_handle(args));
    if (argc < 0 || molt_err_pending() != 0) {
        return 0;
    }
    if (kwargs != NULL && kwargs != Py_None) {
        kw_size = PyMapping_Size(kwargs);
        if (kw_size < 0) {
            return 0;
        }
    }
    while (*scan != '\0') {
        char code = *scan++;
        if (code == ' ' || code == '\t' || code == '\n' || code == '\r') {
            continue;
        }
        if (code == '|') {
            continue;
        }
        if (code == '$') {
            continue;
        }
        if (code == ':' || code == ';') {
            break;
        }
        switch (code) {
            case 'O':
                if (*scan == '!') {
                    scan++;
                }
                break;
            case 'b':
            case 'B':
            case 'h':
            case 'H':
            case 'i':
            case 'I':
            case 'l':
            case 'k':
            case 'L':
            case 'K':
            case 'n':
            case 'c':
            case 'd':
            case 'f':
            case 'p':
                break;
            case 's':
            case 'z':
            case 'y':
                if (*scan == '#') {
                    scan++;
                }
                break;
            default:
                PyErr_Format(
                    PyExc_TypeError,
                    "unsupported format unit '%c' in PyArg_ParseTupleAndKeywords",
                    code);
                goto done;
        }
        format_param_count++;
    }
    if (format_param_count > 0) {
        kw_present = (unsigned char *)calloc((size_t)format_param_count, sizeof(unsigned char));
        kw_values = (MoltHandle *)calloc((size_t)format_param_count, sizeof(MoltHandle));
        kw_slot_capacity = _molt_pyarg_kw_table_capacity((size_t)format_param_count);
        kw_slots =
            (_molt_pyarg_kw_slot *)calloc(kw_slot_capacity, sizeof(_molt_pyarg_kw_slot));
        if (kw_present == NULL || kw_values == NULL || kw_slots == NULL) {
            PyErr_SetString(PyExc_RuntimeError, "out of memory");
            goto done;
        }
    }
    while (param_index < format_param_count) {
        const char *kwname = kwlist[param_index];
        int insert_rc;
        if (kwname == NULL) {
            PyErr_SetString(PyExc_TypeError, "kwlist is shorter than format string");
            goto done;
        }
        if (kwname[0] != '\0') {
            insert_rc = _molt_pyarg_kw_table_insert(
                kw_slots, kw_slot_capacity, kwname, (Py_ssize_t)param_index);
            if (insert_rc == 0) {
                PyErr_Format(PyExc_TypeError, "duplicate keyword name '%s' in kwlist", kwname);
                goto done;
            }
            if (insert_rc < 0) {
                PyErr_SetString(PyExc_RuntimeError, "invalid keyword table state");
                goto done;
            }
        }
        param_index++;
    }
    param_index = 0;
    if (kw_size > 0 && format_param_count > 0) {
        PyObject *kw_keys_obj;
        int64_t kw_count;
        int64_t kw_i = 0;
        kw_keys_bits = molt_mapping_keys(_molt_py_handle(kwargs));
        if (molt_err_pending() != 0 || kw_keys_bits == 0 || kw_keys_bits == molt_none()) {
            goto done;
        }
        kw_keys_obj = _molt_pyobject_from_handle(kw_keys_bits);
        kw_count = (int64_t)molt_sequence_length(_molt_py_handle(kw_keys_obj));
        if (kw_count < 0 || molt_err_pending() != 0) {
            goto done;
        }
        while (kw_i < kw_count) {
            MoltHandle key_bits = 0;
            MoltHandle value_bits = 0;
            uint64_t key_len = 0;
            const uint8_t *key_ptr;
            Py_ssize_t key_index = -1;
            if (!_molt_pyarg_get_positional_item(kw_keys_obj, kw_i, &key_bits)) {
                goto done;
            }
            key_ptr = molt_string_as_ptr(key_bits, &key_len);
            if (key_ptr == NULL || molt_err_pending() != 0) {
                PyErr_Clear();
                molt_handle_decref(key_bits);
                PyErr_SetString(PyExc_TypeError, "keywords must be strings");
                goto done;
            }
            if (!_molt_pyarg_kw_table_lookup(
                    kw_slots, kw_slot_capacity, key_ptr, (size_t)key_len, &key_index)) {
                PyErr_Format(
                    PyExc_TypeError,
                    "unexpected keyword argument '%.*s'",
                    (int)key_len,
                    (const char *)key_ptr);
                molt_handle_decref(key_bits);
                goto done;
            }
            if (kw_present[(size_t)key_index] != 0) {
                PyErr_Format(
                    PyExc_TypeError,
                    "multiple values for keyword argument '%s'",
                    kwlist[(size_t)key_index]);
                molt_handle_decref(key_bits);
                goto done;
            }
            value_bits = molt_mapping_getitem(_molt_py_handle(kwargs), key_bits);
            molt_handle_decref(key_bits);
            if (molt_err_pending() != 0 || value_bits == 0) {
                if (value_bits != 0 && value_bits != molt_none()) {
                    molt_handle_decref(value_bits);
                }
                goto done;
            }
            kw_present[(size_t)key_index] = 1;
            kw_values[(size_t)key_index] = value_bits;
            kw_i++;
        }
    } else if (kw_size > 0 && format_param_count == 0) {
        PyErr_SetString(PyExc_TypeError, "unexpected keyword argument");
        goto done;
    }
    va_start(ap, kwlist);
    va_started = 1;
    while (*cursor != '\0') {
        char code = *cursor++;
        const char *kwname;
        MoltHandle item_bits = 0;
        int has_value = 0;
        int uses_keyword_value = 0;
        if (code == ' ' || code == '\t' || code == '\n' || code == '\r') {
            continue;
        }
        if (code == '|') {
            optional = 1;
            continue;
        }
        if (code == '$') {
            if (!optional) {
                PyErr_SetString(
                    PyExc_TypeError,
                    "'$' marker in format must appear after '|'");
                goto done;
            }
            keyword_only = 1;
            continue;
        }
        if (code == ':' || code == ';') {
            break;
        }

        if (param_index >= format_param_count) {
            PyErr_SetString(PyExc_TypeError, "format/kwlist arity mismatch");
            goto done;
        }
        kwname = kwlist[param_index];
        if (kwname == NULL) {
            PyErr_SetString(PyExc_TypeError, "kwlist is shorter than format string");
            goto done;
        }
        param_index++;

        if (!keyword_only && arg_index < argc) {
            if (kw_present[(size_t)(param_index - 1)] != 0 && kwname[0] != '\0') {
                PyErr_Format(
                    PyExc_TypeError,
                    "argument '%s' given by name and position",
                    kwname);
                goto done;
            }
            if (!_molt_pyarg_get_positional_item(args, arg_index, &item_bits)) {
                goto done;
            }
            arg_index++;
            has_value = 1;
        } else {
            if (kw_present[(size_t)(param_index - 1)] != 0 && kwname[0] != '\0') {
                item_bits = kw_values[(size_t)(param_index - 1)];
                has_value = 1;
                uses_keyword_value = 1;
            }
            if (!has_value && !optional) {
                if (kwname[0] != '\0') {
                    PyErr_Format(PyExc_TypeError, "missing required argument '%s'", kwname);
                } else {
                    PyErr_SetString(PyExc_TypeError, "missing required positional argument");
                }
                goto done;
            }
        }

        if (!_molt_pyarg_convert_value(
                item_bits, has_value, code, &cursor, &ap, "PyArg_ParseTupleAndKeywords")) {
            if (has_value && !uses_keyword_value && item_bits != 0 && item_bits != molt_none()) {
                molt_handle_decref(item_bits);
            }
            goto done;
        }
        if (has_value && !uses_keyword_value && item_bits != 0 && item_bits != molt_none()) {
            molt_handle_decref(item_bits);
        }
    }
    if (arg_index < argc) {
        PyErr_SetString(PyExc_TypeError, "too many positional arguments");
        goto done;
    }
    out = 1;
done:
    if (kw_keys_bits != 0 && kw_keys_bits != molt_none()) {
        molt_handle_decref(kw_keys_bits);
    }
    if (kw_values != NULL) {
        int i = 0;
        while (i < format_param_count) {
            if (kw_values[i] != 0 && kw_values[i] != molt_none()) {
                molt_handle_decref(kw_values[i]);
            }
            i++;
        }
    }
    free(kw_slots);
    free(kw_values);
    free(kw_present);
    if (va_started) {
        va_end(ap);
    }
    return out;
}

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MOLT_C_API_PYTHON_H */
