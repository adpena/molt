/*
 * Python.h — Molt CPython ABI compatibility header (SECONDARY).
 *
 * WARNING: This is NOT the canonical Molt Python.h header. Most users should
 * use the top-level header instead:
 *
 *   cc -I include myext.c          (uses include/Python.h -> include/molt/Python.h)
 *
 * This header exists ONLY for extensions that link against the standalone
 * libmolt_cpython_abi shared library. It uses extern declarations and a
 * traditional CPython struct layout (ob_refcnt, ob_type) that is NOT
 * compatible with the main include/molt/Python.h header.
 *
 * If you are unsure which to use, use the top-level one:
 *   cc -O2 -shared -fPIC -I include myext.c -o _myext.so
 *
 * Extensions compiled against THIS header link against:
 *   libmolt_cpython_abi.dylib  (macOS)
 *   libmolt_cpython_abi.so     (Linux)
 *   molt_cpython_abi.dll       (Windows)
 *
 * ABI note: struct layouts match CPython 3.12 x86-64 / aarch64.
 */
#pragma once
#ifndef Py_PYTHON_H
#define Py_PYTHON_H

#ifndef _POSIX_C_SOURCE
#define _POSIX_C_SOURCE 200809L
#endif
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif

#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

#ifndef PyAPI_FUNC
#define PyAPI_FUNC(RTYPE) extern RTYPE
#endif
#ifndef PyAPI_DATA
#define PyAPI_DATA(RTYPE) extern RTYPE
#endif

#define MOLT_CPYTHON_ABI 1
/*
 * Source-recompiled extensions sometimes vendor pythoncapi-compat to emulate
 * newer CPython APIs with static CPython-private field access. The Molt ABI
 * owns that surface as linkable primitives instead, so those vendored polyfills
 * must not create a duplicate local authority.
 */
#ifndef PYTHONCAPI_COMPAT
#define PYTHONCAPI_COMPAT 1
#define MOLT_CPYTHON_ABI_OWNS_PYTHONCAPI_COMPAT 1
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ── Version ──────────────────────────────────────────────────────────────── */

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0
#define PY_RELEASE_LEVEL 0xF
#define PY_RELEASE_SERIAL 0
#define PY_VERSION_HEX ((PY_MAJOR_VERSION << 24) | (PY_MINOR_VERSION << 16) | (PY_MICRO_VERSION << 8) | (PY_RELEASE_LEVEL << 4) | PY_RELEASE_SERIAL)
#define PY_VERSION "3.12.0 (Molt runtime)"

#define CO_OPTIMIZED 0x0001
#define CO_NEWLOCALS 0x0002
#define CO_VARARGS 0x0004
#define CO_VARKEYWORDS 0x0008
#define CO_GENERATOR 0x0020
#define CO_COROUTINE 0x0080
#define CO_ASYNC_GENERATOR 0x0200

/* ── Primitive types ──────────────────────────────────────────────────────── */

typedef intptr_t Py_ssize_t;
typedef intptr_t Py_hash_t;
typedef size_t  Py_uhash_t;
typedef int64_t PY_INT64_T;
typedef uint64_t PY_UINT64_T;
typedef uint8_t Py_UCS1;
typedef uint16_t Py_UCS2;
typedef uint32_t Py_UCS4;
typedef wchar_t Py_UNICODE;
typedef uintptr_t Py_uintptr_t;
typedef intptr_t Py_intptr_t;
typedef uint32_t digit;
typedef int32_t sdigit;

#define Py_USING_UNICODE 1

typedef struct {
    double real;
    double imag;
} Py_complex;

#define PY_LONG_LONG long long
#define PYLONG_BITS_IN_DIGIT 30
#define PyLong_SHIFT 30
#define PyLong_BASE ((digit)1 << PyLong_SHIFT)
#define PyLong_MASK ((digit)(PyLong_BASE - 1))

#define PY_SSIZE_T_MAX  ((Py_ssize_t)(((size_t)-1)>>1))
#define PY_SSIZE_T_MIN  (-PY_SSIZE_T_MAX - 1)

#define _Py_IMMORTAL_REFCNT_LOCAL ((Py_ssize_t)(1 << 30))
#define _Py_IMMORTAL_INITIAL_REFCNT _Py_IMMORTAL_REFCNT_LOCAL

#define Py_ASNATIVEBYTES_DEFAULTS -1
#define Py_ASNATIVEBYTES_BIG_ENDIAN 0
#define Py_ASNATIVEBYTES_LITTLE_ENDIAN 1
#define Py_ASNATIVEBYTES_NATIVE_ENDIAN 3
#define Py_ASNATIVEBYTES_UNSIGNED_BUFFER 4
#define Py_ASNATIVEBYTES_REJECT_NEGATIVE 8
#define Py_ASNATIVEBYTES_ALLOW_INDEX 16
#ifndef SIZEOF_VOID_P
#define SIZEOF_VOID_P sizeof(void *)
#endif

/* ── Forward declarations ─────────────────────────────────────────────────── */

typedef struct _object      PyObject;
typedef struct _typeobject  PyTypeObject;
typedef struct _longobject  PyLongObject;
typedef struct PyCodeObject PyCodeObject;
typedef struct PyFrameObject PyFrameObject;

/* ── Ob_refcnt / Ob_type helpers ──────────────────────────────────────────── */

/* Inline refcount — matches CPython 3.12 non-Py_GIL_DISABLED layout. */
#define PyObject_HEAD       \
    Py_ssize_t ob_refcnt;   \
    PyTypeObject *ob_type;

#define PyObject_VAR_HEAD   \
    PyObject_HEAD           \
    Py_ssize_t ob_size;

#define PyObject_HEAD_INIT(type) 1, (type),
#define PyVarObject_HEAD_INIT(type, size) PyObject_HEAD_INIT(type) (size),

/* ── PyObject ─────────────────────────────────────────────────────────────── */

struct _object {
    PyObject_HEAD
};

typedef struct {
    PyObject_VAR_HEAD
} PyVarObject;

typedef struct {
    uintptr_t lv_tag;
    digit ob_digit[1];
} _PyLongValue;

struct _longobject {
    PyObject_HEAD
    _PyLongValue long_value;
};

struct PyCodeObject {
    PyObject_HEAD
    int _co_firsttraceable;
};

struct PyFrameObject {
    PyObject_HEAD
    PyFrameObject *f_back;
    PyCodeObject *f_code;
    PyObject *f_globals;
    PyObject *f_locals;
    int f_lineno;
};

/* ── Function pointer typedefs ────────────────────────────────────────────── */

typedef PyObject *(*unaryfunc)   (PyObject *);
typedef PyObject *(*binaryfunc)  (PyObject *, PyObject *);
typedef PyObject *(*ternaryfunc) (PyObject *, PyObject *, PyObject *);
typedef int       (*inquiry)     (PyObject *);
typedef Py_ssize_t(*lenfunc)     (PyObject *);
typedef PyObject *(*ssizeargfunc)(PyObject *, Py_ssize_t);
typedef PyObject *(*ssizessizeargfunc)(PyObject *, Py_ssize_t, Py_ssize_t);
typedef int (*ssizeobjargproc)   (PyObject *, Py_ssize_t, PyObject *);
typedef int (*ssizessizeobjargproc)(PyObject *, Py_ssize_t, Py_ssize_t, PyObject *);
typedef int (*objobjargproc)     (PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)        (PyObject *, PyObject *);
typedef void(*destructor)        (PyObject *);
typedef PyObject *(*reprfunc)    (PyObject *);
typedef PyObject *(*richcmpfunc) (PyObject *, PyObject *, int);
typedef PyObject *(*getattrofunc)(PyObject *, PyObject *);
typedef int (*setattrofunc)      (PyObject *, PyObject *, PyObject *);
typedef PyObject *(*descrgetfunc)(PyObject *, PyObject *, PyObject *);
typedef int (*descrsetfunc)      (PyObject *, PyObject *, PyObject *);
typedef Py_hash_t (*hashfunc)    (PyObject *);
typedef int (*visitproc)         (PyObject *, void *);
typedef int (*traverseproc)      (PyObject *, visitproc, void *);
typedef PyObject *(*newfunc)     (PyTypeObject *, PyObject *, PyObject *);
typedef PyObject *(*allocfunc)   (PyTypeObject *, Py_ssize_t);
typedef int (*initproc)          (PyObject *, PyObject *, PyObject *);
typedef void (*freefunc)         (void *);
typedef PyObject *(*getiterfunc) (PyObject *);
typedef PyObject *(*iternextfunc)(PyObject *);
typedef PyObject *(*getter)      (PyObject *, void *);
typedef int (*setter)            (PyObject *, PyObject *, void *);
typedef PyObject *(*sendfunc)    (PyObject *, PyObject *, int *);
typedef PyObject *(*vectorcallfunc)(PyObject *, PyObject *const *, size_t, PyObject *);

/* ── Number / Sequence / Mapping protocol structs ────────────────────────── */

typedef struct {
    binaryfunc  nb_add;
    binaryfunc  nb_subtract;
    binaryfunc  nb_multiply;
    binaryfunc  nb_remainder;
    binaryfunc  nb_divmod;
    ternaryfunc nb_power;
    unaryfunc   nb_negative;
    unaryfunc   nb_positive;
    unaryfunc   nb_absolute;
    inquiry     nb_bool;
    unaryfunc   nb_invert;
    binaryfunc  nb_lshift;
    binaryfunc  nb_rshift;
    binaryfunc  nb_and;
    binaryfunc  nb_xor;
    binaryfunc  nb_or;
    unaryfunc   nb_int;
    void       *nb_reserved;
    unaryfunc   nb_float;
    binaryfunc  nb_inplace_add;
    binaryfunc  nb_inplace_subtract;
    binaryfunc  nb_inplace_multiply;
    binaryfunc  nb_inplace_remainder;
    ternaryfunc nb_inplace_power;
    binaryfunc  nb_inplace_lshift;
    binaryfunc  nb_inplace_rshift;
    binaryfunc  nb_inplace_and;
    binaryfunc  nb_inplace_xor;
    binaryfunc  nb_inplace_or;
    binaryfunc  nb_floor_divide;
    binaryfunc  nb_true_divide;
    binaryfunc  nb_inplace_floor_divide;
    binaryfunc  nb_inplace_true_divide;
    unaryfunc   nb_index;
    binaryfunc  nb_matrix_multiply;
    binaryfunc  nb_inplace_matrix_multiply;
} PyNumberMethods;

typedef struct {
    lenfunc      sq_length;
    binaryfunc   sq_concat;
    ssizeargfunc sq_repeat;
    ssizeargfunc sq_item;
    void        *was_sq_slice;
    ssizeobjargproc sq_ass_item;
    void        *was_sq_ass_slice;
    objobjproc  sq_contains;
    binaryfunc  sq_inplace_concat;
    ssizeargfunc sq_inplace_repeat;
} PySequenceMethods;

typedef struct {
    lenfunc     mp_length;
    binaryfunc  mp_subscript;
    objobjargproc mp_ass_subscript;
} PyMappingMethods;

typedef struct bufferinfo {
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

typedef struct {
    PyObject_HEAD
    Py_buffer view;
    PyObject *base;
} PyMemoryViewObject;

typedef int (*getbufferproc)(PyObject *, Py_buffer *, int);
typedef void (*releasebufferproc)(PyObject *, Py_buffer *);

typedef struct {
    getbufferproc bf_getbuffer;
    releasebufferproc bf_releasebuffer;
} PyBufferProcs;

#define PyBUF_SIMPLE 0
#define PyBUF_WRITABLE 0x0001
#define PyBUF_WRITEABLE PyBUF_WRITABLE
#define PyBUF_FORMAT 0x0004
#define PyBUF_ND 0x0008
#define PyBUF_STRIDES (0x0010 | PyBUF_ND)
#define PyBUF_C_CONTIGUOUS (0x0020 | PyBUF_STRIDES)
#define PyBUF_F_CONTIGUOUS (0x0040 | PyBUF_STRIDES)
#define PyBUF_ANY_CONTIGUOUS (0x0080 | PyBUF_STRIDES)
#define PyBUF_INDIRECT (0x0100 | PyBUF_STRIDES)
#define PyBUF_CONTIG_RO (PyBUF_ND)
#define PyBUF_CONTIG (PyBUF_ND | PyBUF_WRITABLE)
#define PyBUF_RECORDS_RO (PyBUF_STRIDES | PyBUF_FORMAT)
#define PyBUF_RECORDS (PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE)
#define PyBUF_FULL_RO (PyBUF_INDIRECT | PyBUF_FORMAT)
#define PyBUF_FULL (PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE)
#define PyBUF_READ 0x100
#define PyBUF_WRITE 0x200
#define Py_CLEANUP_SUPPORTED 0x20000
#ifndef Py_ABS
#define Py_ABS(x) ((x) >= 0 ? (x) : -(x))
#endif
#ifndef Py_MIN
#define Py_MIN(x, y) (((x) > (y)) ? (y) : (x))
#endif
#ifndef Py_MAX
#define Py_MAX(x, y) (((x) > (y)) ? (x) : (y))
#endif
#ifndef _Py_STATIC_CAST
#ifdef __cplusplus
#define _Py_STATIC_CAST(type, expr) static_cast<type>(expr)
#else
#define _Py_STATIC_CAST(type, expr) ((type)(expr))
#endif
#endif
#ifndef Py_SAFE_DOWNCAST
#ifdef Py_DEBUG
#define Py_SAFE_DOWNCAST(VALUE, WIDE, NARROW) \
    (assert(_Py_STATIC_CAST(WIDE, _Py_STATIC_CAST(NARROW, (VALUE))) == (VALUE)), \
     _Py_STATIC_CAST(NARROW, (VALUE)))
#else
#define Py_SAFE_DOWNCAST(VALUE, WIDE, NARROW) _Py_STATIC_CAST(NARROW, (VALUE))
#endif
#endif
#define Py_CHARMASK(c) ((unsigned char)((c) & 0xff))

typedef struct {
    unaryfunc am_await;
    unaryfunc am_aiter;
    unaryfunc am_anext;
    sendfunc am_send;
} PyAsyncMethods;

typedef struct {
    PyObject_HEAD
    Py_ssize_t ma_used;
    uint64_t ma_version_tag;
} PyDictObject;

typedef struct {
    PyObject_VAR_HEAD
    PyObject **ob_item;
} PyTupleObject;

typedef struct {
    PyObject_VAR_HEAD
    PyObject **ob_item;
    Py_ssize_t allocated;
} PyListObject;

typedef struct {
    PyObject_VAR_HEAD
    Py_hash_t ob_shash;
    char ob_sval[1];
} PyBytesObject;

typedef struct {
    PyObject_VAR_HEAD
    Py_ssize_t ob_alloc;
    char *ob_bytes;
    char *ob_start;
    Py_ssize_t ob_exports;
} PyByteArrayObject;

typedef struct {
    PyObject_HEAD
    Py_complex cval;
} PyComplexObject;

typedef struct {
    PyObject_HEAD
    PyObject *start;
    PyObject *stop;
    PyObject *step;
} PySliceObject;

typedef struct {
    PyObject_HEAD
    PyObject *mapping;
} PyDictProxyObject;

typedef struct {
    PyObject_HEAD
    PyObject *origin;
    PyObject *args;
} PyGenericAliasObject;

/* ── PyTypeObject ─────────────────────────────────────────────────────────── */

struct _typeobject {
    /* ob_base (PyVarObject) */
    Py_ssize_t          ob_refcnt;
    PyTypeObject       *ob_type;
    Py_ssize_t          ob_size;

    const char         *tp_name;
    Py_ssize_t          tp_basicsize;
    Py_ssize_t          tp_itemsize;
    destructor          tp_dealloc;
    Py_ssize_t          tp_vectorcall_offset;
    void               *tp_getattr;
    void               *tp_setattr;
    void               *tp_as_async;
    reprfunc            tp_repr;
    PyNumberMethods    *tp_as_number;
    PySequenceMethods  *tp_as_sequence;
    PyMappingMethods   *tp_as_mapping;
    hashfunc            tp_hash;
    ternaryfunc         tp_call;
    reprfunc            tp_str;
    getattrofunc        tp_getattro;
    setattrofunc        tp_setattro;
    PyBufferProcs      *tp_as_buffer;
    unsigned long       tp_flags;
    const char         *tp_doc;
    traverseproc        tp_traverse;
    inquiry             tp_clear;
    richcmpfunc         tp_richcompare;
    Py_ssize_t          tp_weaklistoffset;
    unaryfunc           tp_iter;
    unaryfunc           tp_iternext;
    struct PyMethodDef *tp_methods;
    struct PyMemberDef *tp_members;
    struct PyGetSetDef *tp_getset;
    PyTypeObject       *tp_base;
    PyObject           *tp_dict;
    descrgetfunc        tp_descr_get;
    descrsetfunc        tp_descr_set;
    Py_ssize_t          tp_dictoffset;
    initproc            tp_init;
    allocfunc           tp_alloc;
    newfunc             tp_new;
    freefunc            tp_free;
    inquiry             tp_is_gc;
    PyObject           *tp_bases;
    PyObject           *tp_mro;
    PyObject           *tp_cache;
    void               *tp_subclasses;
    PyObject           *tp_weaklist;
    destructor          tp_del;
    unsigned int        tp_version_tag;
    destructor          tp_finalize;
    void               *tp_vectorcall;
    unsigned char       tp_watched;
};

struct _dictkeysobject;
struct _specialization_cache {
    PyObject *getitem;
    uint32_t getitem_version;
};

typedef struct _heaptypeobject {
    PyTypeObject ht_type;
    PyAsyncMethods as_async;
    PyNumberMethods as_number;
    PyMappingMethods as_mapping;
    PySequenceMethods as_sequence;
    PyBufferProcs as_buffer;
    PyObject *ht_name;
    PyObject *ht_slots;
    PyObject *ht_qualname;
    struct _dictkeysobject *ht_cached_keys;
    PyObject *ht_module;
    char *_ht_tpname;
    struct _specialization_cache _spec_cache;
} PyHeapTypeObject;

/* ── tp_flags ─────────────────────────────────────────────────────────────── */

#define Py_TPFLAGS_DEFAULT      (0)
#define Py_TPFLAGS_HEAPTYPE     (1UL << 9)
#define Py_TPFLAGS_BASETYPE     (1UL << 10)
#define Py_TPFLAGS_HAVE_VECTORCALL (1UL << 11)
#define _Py_TPFLAGS_HAVE_VECTORCALL Py_TPFLAGS_HAVE_VECTORCALL
#define Py_TPFLAGS_METHOD_DESCRIPTOR (1UL << 17)
#define Py_TPFLAGS_MANAGED_DICT (1UL << 4)
#define Py_TPFLAGS_HAVE_GC      (1UL << 14)
#define Py_TPFLAGS_READY        (1UL << 12)
#define Py_TPFLAGS_HAVE_VERSION_TAG (1UL << 18)
#define Py_TPFLAGS_CHECKTYPES   (0)
#define Py_TPFLAGS_HAVE_NEWBUFFER (0)
#define Py_TPFLAGS_IS_ABSTRACT  (1UL << 20)
#define Py_TPFLAGS_UNICODE_SUBCLASS (1UL << 28)
#define Py_TPFLAGS_BASE_EXC_SUBCLASS (1UL << 30)

#define Py_T_SHORT              0
#define Py_T_INT                1
#define Py_T_LONG               2
#define Py_T_FLOAT              3
#define Py_T_DOUBLE             4
#define Py_T_STRING             5
#define _Py_T_OBJECT            6
#define Py_T_OBJECT             _Py_T_OBJECT
#define Py_T_CHAR               7
#define Py_T_BYTE               8
#define Py_T_UBYTE              9
#define Py_T_USHORT             10
#define Py_T_UINT               11
#define Py_T_ULONG              12
#define Py_T_STRING_INPLACE     13
#define Py_T_BOOL               14
#define Py_T_OBJECT_EX          16
#define Py_T_LONGLONG           17
#define Py_T_ULONGLONG          18
#define Py_T_PYSSIZET           19
#define _Py_T_NONE              20
#define Py_T_NONE               _Py_T_NONE
#define Py_READONLY             1
#define Py_AUDIT_READ           2
#define _Py_WRITE_RESTRICTED    4

/* ── PyMethodDef ──────────────────────────────────────────────────────────── */

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*_PyCFunctionFast)(PyObject *, PyObject *const *, Py_ssize_t);
typedef PyObject *(*_PyCFunctionFastWithKeywords)(PyObject *, PyObject *const *, Py_ssize_t, PyObject *);
typedef PyObject *(*PyCMethod)(PyObject *, PyTypeObject *, PyObject *const *, size_t, PyObject *);

#define METH_VARARGS    0x0001
#define METH_KEYWORDS   0x0002
#define METH_NOARGS     0x0004
#define METH_O          0x0008
#define METH_CLASS      0x0010
#define METH_STATIC     0x0020
#define METH_COEXIST    0x0040
#define METH_FASTCALL   0x0080
#define METH_METHOD     0x0200

typedef struct PyMethodDef {
    const char  *ml_name;
    PyCFunction  ml_meth;
    int          ml_flags;
    const char  *ml_doc;
} PyMethodDef;

/* Sentinel that terminates a PyMethodDef array. */
#define PY_METHODDEF_SENTINEL   { NULL, NULL, 0, NULL }

typedef struct {
    PyObject_HEAD
    PyMethodDef *m_ml;
    PyObject *m_self;
    PyObject *m_module;
    PyObject *m_weakreflist;
    vectorcallfunc vectorcall;
} PyCFunctionObject;

typedef struct {
    PyCFunctionObject func;
    PyTypeObject *mm_class;
} PyCMethodObject;

typedef struct {
    PyObject_HEAD
    Py_ssize_t length;
    Py_hash_t hash;
    unsigned int state;
    wchar_t *wstr;
} PyASCIIObject;

typedef struct {
    PyASCIIObject _base;
    Py_ssize_t utf8_length;
    char *utf8;
    Py_ssize_t wstr_length;
} PyCompactUnicodeObject;

typedef struct {
    PyCompactUnicodeObject _base;
    union {
        void *any;
        Py_UCS1 *latin1;
        Py_UCS2 *ucs2;
        Py_UCS4 *ucs4;
    } data;
} PyUnicodeObject;

typedef struct PyMutex {
    uintptr_t _bits;
} PyMutex;

typedef void *PyThread_type_lock;

#define WAIT_LOCK 1
#define NOWAIT_LOCK 0

static inline PyThread_type_lock PyThread_allocate_lock(void) {
    return (PyThread_type_lock)1;
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
    (void)lock;
}

#define Py_BEGIN_CRITICAL_SECTION(op) do { (void)(op); } while (0)
#define Py_END_CRITICAL_SECTION() do { } while (0)
#define Py_BEGIN_CRITICAL_SECTION2(op1, op2) do { (void)(op1); (void)(op2); } while (0)
#define Py_END_CRITICAL_SECTION2() do { } while (0)

typedef struct _is {
    int _molt_reserved;
} PyInterpreterState;

typedef struct _err_stackitem {
    PyObject *exc_type;
    PyObject *exc_value;
    PyObject *exc_traceback;
    struct _err_stackitem *previous_item;
} _PyErr_StackItem;

typedef struct _ts {
    PyInterpreterState *interp;
    PyObject *current_exception;
    _PyErr_StackItem *exc_info;
    _PyErr_StackItem exc_state;
    int _molt_reserved;
} PyThreadState;

typedef enum {
    PyGILState_LOCKED,
    PyGILState_UNLOCKED
} PyGILState_STATE;

typedef struct {
    PyObject_HEAD
    PyObject *dict;
    PyObject *args;
    PyObject *notes;
    PyObject *traceback;
    PyObject *context;
    PyObject *cause;
    char suppress_context;
} PyBaseExceptionObject;

typedef struct {
    PyObject_HEAD
    PyTypeObject *d_type;
    PyMethodDef *d_method;
} PyMethodDescrObject;

typedef struct {
    PyObject_HEAD
    PyTypeObject *d_type;
    PyObject *d_name;
    PyObject *d_qualname;
    struct PyMemberDef *d_member;
} PyMemberDescrObject;

typedef struct {
    PyObject_HEAD
    PyTypeObject *d_type;
    PyObject *d_name;
    PyObject *d_qualname;
    struct PyGetSetDef *d_getset;
} PyGetSetDescrObject;

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

#define Py_tp_dealloc 52
#define Py_mp_subscript 5
#define Py_tp_repr 66
#define Py_tp_call 50
#define Py_tp_traverse 71
#define Py_tp_clear 51
#define Py_tp_methods 64
#define Py_tp_members 72
#define Py_tp_getset 73
#define Py_tp_descr_get 54
#define Py_tp_setattro 69
#define Py_tp_new 65

typedef struct PyModuleDef_Slot {
    int slot;
    void *value;
} PyModuleDef_Slot;

/* ── PyModuleDef ──────────────────────────────────────────────────────────── */

typedef struct PyModuleDef_Base {
    PyObject_HEAD
    PyObject *(*m_init)(void);
    Py_ssize_t m_index;
    PyObject *m_copy;
} PyModuleDef_Base;

#define PyModuleDef_HEAD_INIT   { 1, NULL, NULL, 0, NULL }

typedef struct PyModuleDef {
    PyModuleDef_Base    m_base;
    const char         *m_name;
    const char         *m_doc;
    Py_ssize_t          m_size;
    PyMethodDef        *m_methods;
    PyModuleDef_Slot   *m_slots;
    traverseproc        m_traverse;
    inquiry             m_clear;
    freefunc            m_free;
} PyModuleDef;

#define Py_mod_create 1
#define Py_mod_exec 2
#define Py_mod_multiple_interpreters 3
#define Py_mod_gil 4
#define Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED 0
#define Py_MOD_MULTIPLE_INTERPRETERS_SUPPORTED 1
#define Py_MOD_PER_INTERPRETER_GIL_SUPPORTED 2
#define Py_MOD_GIL_USED 0
#define Py_MOD_GIL_NOT_USED 1

/* ── PyMemberDef / PyGetSetDef (forward decl) ─────────────────────────────── */

typedef struct PyMemberDef {
    const char *name;
    int         type;
    Py_ssize_t  offset;
    int         flags;
    const char *doc;
} PyMemberDef;

typedef struct PyGetSetDef {
    const char *name;
    PyObject *(*get)(PyObject *, void *);
    int (*set)(PyObject *, PyObject *, void *);
    const char *doc;
    void *closure;
} PyGetSetDef;

/* ── PyMODINIT_FUNC ───────────────────────────────────────────────────────── */

#define PyMODINIT_FUNC  __attribute__((visibility("default"))) PyObject *

/* ── Singleton singletons ─────────────────────────────────────────────────── */

extern PyObject Py_None;
extern PyObject Py_True;
extern PyObject Py_False;
extern PyObject Py_NotImplementedSentinel;
extern PyObject Py_EllipsisObject;

extern PyTypeObject PyLong_Type;
extern PyTypeObject PyFloat_Type;
extern PyTypeObject PyComplex_Type;
extern PyTypeObject PyUnicode_Type;
extern PyTypeObject PyBytes_Type;
extern PyTypeObject PyByteArray_Type;
extern PyTypeObject PyBool_Type;
extern PyTypeObject PyList_Type;
extern PyTypeObject PyTuple_Type;
extern PyTypeObject PyDict_Type;
extern PyTypeObject PyDictProxy_Type;
extern PyTypeObject Py_GenericAliasType;
extern PyTypeObject PyContextVar_Type;
extern PyTypeObject PySet_Type;
extern PyTypeObject PyFrozenSet_Type;
extern PyTypeObject PyMemoryView_Type;
extern PyTypeObject PyModule_Type;
extern PyTypeObject PyType_Type;
extern PyTypeObject PyBaseObject_Type;
extern PyTypeObject PyNone_Type;
extern PyTypeObject PyNotImplemented_Type;
extern PyTypeObject PyCFunction_Type;
extern PyTypeObject PyMethod_Type;
extern PyTypeObject PyMethodDescr_Type;
extern PyTypeObject PyMemberDescr_Type;
extern PyTypeObject PyGetSetDescr_Type;
extern PyTypeObject PyCapsule_Type;
extern PyTypeObject PySlice_Type;

static inline PyTypeObject *_molt_builtin_type_object_borrowed(const char *name) {
    if (name == NULL) return &PyBaseObject_Type;
    if (strcmp(name, "int") == 0) return &PyLong_Type;
    if (strcmp(name, "float") == 0) return &PyFloat_Type;
    if (strcmp(name, "bool") == 0) return &PyBool_Type;
    if (strcmp(name, "bytes") == 0) return &PyBytes_Type;
    if (strcmp(name, "bytearray") == 0) return &PyByteArray_Type;
    if (strcmp(name, "str") == 0) return &PyUnicode_Type;
    if (strcmp(name, "complex") == 0) return &PyComplex_Type;
    if (strcmp(name, "list") == 0) return &PyList_Type;
    if (strcmp(name, "tuple") == 0) return &PyTuple_Type;
    if (strcmp(name, "dict") == 0) return &PyDict_Type;
    if (strcmp(name, "mappingproxy") == 0) return &PyDictProxy_Type;
    if (strcmp(name, "set") == 0) return &PySet_Type;
    if (strcmp(name, "frozenset") == 0) return &PyFrozenSet_Type;
    if (strcmp(name, "memoryview") == 0) return &PyMemoryView_Type;
    if (strcmp(name, "module") == 0) return &PyModule_Type;
    if (strcmp(name, "type") == 0) return &PyType_Type;
    if (strcmp(name, "object") == 0) return &PyBaseObject_Type;
    if (strcmp(name, "builtin_function_or_method") == 0) return &PyCFunction_Type;
    if (strcmp(name, "method") == 0) return &PyMethod_Type;
    if (strcmp(name, "method_descriptor") == 0) return &PyMethodDescr_Type;
    if (strcmp(name, "member_descriptor") == 0) return &PyMemberDescr_Type;
    if (strcmp(name, "getset_descriptor") == 0) return &PyGetSetDescr_Type;
    if (strcmp(name, "PyCapsule") == 0) return &PyCapsule_Type;
    if (strcmp(name, "slice") == 0) return &PySlice_Type;
    if (strcmp(name, "NoneType") == 0) return &PyNone_Type;
    if (strcmp(name, "NotImplementedType") == 0) return &PyNotImplemented_Type;
    return &PyBaseObject_Type;
}

#define Py_None  (&Py_None)
#define Py_True  (&Py_True)
#define Py_False (&Py_False)
#define Py_NotImplemented (&Py_NotImplementedSentinel)
#define Py_Ellipsis (&Py_EllipsisObject)

/* ── Refcount macros ──────────────────────────────────────────────────────── */

extern void Py_INCREF(PyObject *op);
extern void Py_DECREF(PyObject *op);
extern PyObject *Py_NewRef(PyObject *op);
extern PyObject *Py_XNewRef(PyObject *op);

#ifndef _PyObject_CAST
#define _PyObject_CAST(op) ((PyObject *)(op))
#endif

#ifndef Py_NewRef
#define Py_NewRef(op) Py_NewRef(_PyObject_CAST(op))
#endif

#ifndef Py_XNewRef
#define Py_XNewRef(op) Py_XNewRef(_PyObject_CAST(op))
#endif

#define Py_INCREF(op) Py_INCREF((PyObject *)(op))
#define Py_DECREF(op) Py_DECREF((PyObject *)(op))
#define Py_XINCREF(op) do { if ((op) != NULL) Py_INCREF((PyObject *)(op)); } while(0)
#define Py_XDECREF(op) do { if ((op) != NULL) Py_DECREF((PyObject *)(op)); } while(0)

#define Py_SETREF(dst, src) do {          \
    PyObject *_py_tmp = (PyObject *)(dst); \
    (dst) = (src);                         \
    Py_DECREF(_py_tmp);                    \
} while(0)

#define Py_XSETREF(dst, src) do {         \
    PyObject *_py_tmp = (PyObject *)(dst); \
    (dst) = (src);                         \
    Py_XDECREF(_py_tmp);                   \
} while(0)

#define Py_CLEAR(op) do {          \
    PyObject *_py_tmp = (PyObject *)(op); \
    if (_py_tmp != NULL) {          \
        (op) = NULL;                \
        Py_DECREF(_py_tmp);         \
    }                               \
} while(0)

#define Py_IS_TYPE(ob, type) (Py_TYPE(ob) == (type))
#define Py_Is(x, y) ((x) == (y))
#define Py_IsNone(ob) Py_Is((ob), Py_None)
#define Py_IsTrue(ob) Py_Is((ob), Py_True)
#define Py_IsFalse(ob) Py_Is((ob), Py_False)

#define PY_VECTORCALL_ARGUMENTS_OFFSET ((size_t)1 << (8 * sizeof(size_t) - 1))
#define PyVectorcall_NARGS(n) ((Py_ssize_t)((n) & ~PY_VECTORCALL_ARGUMENTS_OFFSET))

/* ── Return helpers ───────────────────────────────────────────────────────── */

#define Py_RETURN_NONE                \
    do { Py_INCREF(Py_None); return Py_None; } while(0)

#define Py_RETURN_TRUE                \
    do { Py_INCREF(Py_True); return Py_True; } while(0)

#define Py_RETURN_FALSE               \
    do { Py_INCREF(Py_False); return Py_False; } while(0)

#define Py_RETURN_NOTIMPLEMENTED      \
    do { Py_INCREF(Py_NotImplemented); return Py_NotImplemented; } while(0)

/* ── Numeric comparison ops ───────────────────────────────────────────────── */

#define Py_LT 0
#define Py_LE 1
#define Py_EQ 2
#define Py_NE 3
#define Py_GT 4
#define Py_GE 5

/* ── API declarations ─────────────────────────────────────────────────────── */

/* Object / type */
extern int          PyType_Ready        (PyTypeObject *tp);
extern PyObject    *PyType_GenericAlloc (PyTypeObject *tp, Py_ssize_t nitems);
extern PyObject    *PyType_GenericNew   (PyTypeObject *tp, PyObject *args, PyObject *kwds);
extern PyObject    *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases);
extern PyObject    *PyType_FromModuleAndSpec(PyObject *module, PyType_Spec *spec, PyObject *bases);
extern PyObject    *PyType_FromMetaclass(PyTypeObject *metaclass, PyObject *module, PyType_Spec *spec, PyObject *bases);
extern unsigned long PyType_GetFlags    (PyTypeObject *tp);
extern int          PyType_HasFeature   (PyTypeObject *tp, unsigned long feature);
extern int          PyType_Check        (PyObject *op);
extern int          PyType_IsSubtype    (PyTypeObject *a, PyTypeObject *b);
extern PyObject    *PyType_GetQualName  (PyTypeObject *tp);
extern void         PyType_Modified     (PyTypeObject *tp);
extern PyObject    *_PyType_Lookup      (PyTypeObject *tp, PyObject *name);
extern PyObject    *PyObject_Type       (PyObject *op);
extern int          PyObject_TypeCheck  (PyObject *op, PyTypeObject *tp);
extern PyObject    *PyObject_Repr       (PyObject *op);
extern PyObject    *PyObject_Str        (PyObject *op);
extern PyObject    *PyObject_Format     (PyObject *op, PyObject *format_spec);
extern PyObject    *PyObject_Bytes      (PyObject *op);
extern PyObject    *PyObject_ASCII      (PyObject *op);
extern Py_hash_t    PyObject_Hash       (PyObject *op);
extern int          PyObject_IsInstance (PyObject *inst, PyObject *cls);
extern int          PyObject_IsSubclass (PyObject *derived, PyObject *cls);
extern int          PyObject_IsTrue     (PyObject *op);
extern int          PyObject_Not        (PyObject *op);
extern int          PyCallable_Check    (PyObject *op);
extern int          PyObject_Print      (PyObject *op, FILE *fp, int flags);
extern PyObject    *PyObject_RichCompare(PyObject *v, PyObject *w, int op);
extern int          PyObject_RichCompareBool(PyObject *v, PyObject *w, int op);
extern PyObject    *PyObject_GetAttr    (PyObject *op, PyObject *name);
extern PyObject    *PyObject_GetAttrString(PyObject *op, const char *name);
extern int          PyObject_HasAttr    (PyObject *op, PyObject *name);
extern int          PyObject_HasAttrString(PyObject *op, const char *name);
extern int          PyObject_HasAttrWithError(PyObject *op, PyObject *name);
extern int          PyObject_HasAttrStringWithError(PyObject *op, const char *name);
extern int          PyObject_SetAttr    (PyObject *op, PyObject *name, PyObject *value);
extern int          PyObject_SetAttrString(PyObject *op, const char *name, PyObject *value);
extern PyObject    *PyObject_GetItem    (PyObject *op, PyObject *key);
extern int          PyObject_SetItem    (PyObject *op, PyObject *key, PyObject *value);
extern Py_ssize_t   PyObject_Size       (PyObject *op);
extern Py_ssize_t   PyObject_LengthHint (PyObject *op, Py_ssize_t defaultvalue);
extern PyObject    *PyObject_Call       (PyObject *callable, PyObject *args, PyObject *kwargs);
extern PyObject    *PyObject_CallObject (PyObject *callable, PyObject *args);
extern PyObject    *PyObject_CallNoArgs (PyObject *callable);
extern PyObject    *PyObject_CallOneArg (PyObject *callable, PyObject *arg);
extern PyObject    *PyObject_CallMethodNoArgs(PyObject *obj, PyObject *name);
extern PyObject    *PyObject_CallMethodOneArg(PyObject *obj, PyObject *name, PyObject *arg);
extern PyObject    *Py_GenericAlias     (PyObject *origin, PyObject *args);
extern PyObject    *PyObject_Vectorcall (PyObject *callable, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject    *PyObject_VectorcallDict(PyObject *callable, PyObject *const *args, size_t nargs, PyObject *kwargs);
extern PyObject    *PyObject_VectorcallMethod(PyObject *name, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject    *PyVectorcall_Call   (PyObject *callable, PyObject *args, PyObject *kwargs);
extern PyObject    *_PyObject_Vectorcall(PyObject *callable, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject    *PyObject_CallMethodObjArgs(PyObject *callable, PyObject *name, ...);
extern PyObject    *PyObject_CallMethod(PyObject *callable, const char *name, const char *format, ...);
extern PyObject    *PyObject_CallFunction(PyObject *callable, const char *format, ...);
extern PyObject    *PyObject_CallFunctionObjArgs(PyObject *callable, ...);
extern int          PyObject_AsFileDescriptor(PyObject *op);
extern int          PyDescr_IsData      (PyObject *descr);
extern PyObject    *PyDescr_NewGetSet   (PyTypeObject *type, PyGetSetDef *getset);
extern PyObject    *PyDescr_NewMember   (PyTypeObject *type, PyMemberDef *member);
extern PyObject    *PyObject_GenericGetAttr(PyObject *op, PyObject *name);
extern int          PyObject_GenericSetAttr(PyObject *op, PyObject *name, PyObject *value);
extern int          PyObject_GetOptionalAttr(PyObject *op, PyObject *name, PyObject **result);
extern int          PyObject_GetOptionalAttrString(PyObject *op, const char *name, PyObject **result);
extern PyObject    *_PyObject_GenericGetAttrWithDict(PyObject *op, PyObject *name, PyObject *dict, int suppress);
extern PyObject    *PyObject_GenericGetDict(PyObject *op, void *context);
extern int          PyObject_GenericSetDict(PyObject *op, PyObject *value, void *context);
extern PyObject  **_PyObject_GetDictPtr(PyObject *op);
extern void         _PyObject_ClearManagedDict(PyObject *op);
extern int          _PyObject_VisitManagedDict(PyObject *op, visitproc visit, void *arg);
extern void         PyObject_ClearWeakRefs(PyObject *op);
extern PyCodeObject *PyCode_NewEmpty(const char *filename, const char *funcname, int firstlineno);
extern PyFrameObject *PyFrame_New(PyThreadState *tstate, PyCodeObject *code, PyObject *globals, PyObject *locals);
extern PyCodeObject *PyFrame_GetCode(PyFrameObject *frame);
extern PyFrameObject *PyFrame_GetBack(PyFrameObject *frame);
extern int          PyTraceBack_Here(PyFrameObject *frame);
extern int          _PyObject_LookupAttr(PyObject *op, PyObject *name, PyObject **result);
extern PyObject    *PyObject_GetIter(PyObject *op);
extern int          PyIter_Check(PyObject *op);
extern PyObject    *PyIter_Next(PyObject *op);
extern PyObject    *PyObject_Next(PyObject *op);
extern PyObject    *PyObject_SelfIter(PyObject *op);
extern PyObject    *PySeqIter_New(PyObject *seq);
extern PyCodeObject *PyUnstable_Code_NewWithPosOnlyArgs(
    int argcount,
    int posonlyargcount,
    int kwonlyargcount,
    int nlocals,
    int stacksize,
    int flags,
    PyObject *code,
    PyObject *consts,
    PyObject *names,
    PyObject *varnames,
    PyObject *freevars,
    PyObject *cellvars,
    PyObject *filename,
    PyObject *name,
    PyObject *qualname,
    int firstlineno,
    PyObject *linetable,
    PyObject *exceptiontable);

#define Py_VISIT(op) do { if ((op) && visit((PyObject *)(op), arg)) return -1; } while (0)

/* Integer */
extern PyObject *PyLong_FromLong         (long v);
extern PyObject *PyLong_FromLongLong     (long long v);
extern PyObject *PyLong_FromSsize_t      (Py_ssize_t v);
extern PyObject *PyLong_FromSize_t       (size_t v);
extern PyObject *PyLong_FromDouble       (double v);
extern PyObject *PyLong_FromUnicodeObject(PyObject *u, int base);
extern PyObject *PyLong_FromUnsignedLong (unsigned long v);
extern PyObject *PyLong_FromUnsignedLongLong(unsigned long long v);
extern PyObject *PyLong_FromVoidPtr      (void *p);
extern long      PyLong_AsLong           (PyObject *op);
extern long      PyLong_AsLongAndOverflow(PyObject *op, int *overflow);
extern long long PyLong_AsLongLong       (PyObject *op);
extern long long PyLong_AsLongLongAndOverflow(PyObject *op, int *overflow);
extern Py_ssize_t PyLong_AsSsize_t       (PyObject *op);
extern unsigned long PyLong_AsUnsignedLong(PyObject *op);
extern unsigned long long PyLong_AsUnsignedLongLong(PyObject *op);
extern void     *PyLong_AsVoidPtr        (PyObject *op);
extern Py_ssize_t PyLong_AsNativeBytes   (PyObject *op, void *buffer, Py_ssize_t n_bytes, int flags);
extern PyObject *PyLong_FromNativeBytes  (const void *buffer, size_t n_bytes, int flags);
extern PyObject *PyLong_FromUnsignedNativeBytes(const void *buffer, size_t n_bytes, int flags);
extern int       PyLong_Check            (PyObject *op);
extern int       _PyLong_AsInt           (PyObject *op);
extern int       PyLong_AsInt            (PyObject *op);
extern PyObject *_PyLong_FromByteArray   (const unsigned char *bytes, size_t n, int little_endian, int is_signed);
extern int       _PyLong_AsByteArray     (PyLongObject *v, unsigned char *bytes, size_t n, int little_endian, int is_signed);

#define PyLong_CheckExact(op) PyLong_Check((PyObject *)(op))

/* Float */
extern PyObject *PyFloat_FromDouble (double v);
extern PyObject *PyFloat_FromString (PyObject *v);
extern double    PyFloat_AsDouble   (PyObject *op);
extern int       PyFloat_Check      (PyObject *op);
extern Py_hash_t _Py_HashDouble     (PyObject *inst, double v);
#define PyFloat_AS_DOUBLE(op) PyFloat_AsDouble((PyObject *)(op))
#define PyFloat_CheckExact(op) Py_IS_TYPE((PyObject *)(op), &PyFloat_Type)

/* Complex */
extern PyObject   *PyComplex_FromDoubles(double real, double imag);
extern PyObject   *PyComplex_FromCComplex(Py_complex value);
extern Py_complex  PyComplex_AsCComplex(PyObject *op);
extern double      PyComplex_RealAsDouble(PyObject *op);
extern double      PyComplex_ImagAsDouble(PyObject *op);
extern int       PyComplex_Check    (PyObject *op);
#define PyComplex_CheckExact(op) PyComplex_Check((PyObject *)(op))

/* Bool */
extern PyObject *PyBool_FromLong (long v);
extern int       PyBool_Check    (PyObject *op);

/* Unicode / str */
extern PyObject    *PyUnicode_FromString          (const char *s);
extern PyObject    *PyUnicode_FromStringAndSize   (const char *s, Py_ssize_t size);
extern PyObject    *PyUnicode_New                 (Py_ssize_t size, Py_UCS4 maxchar);
extern const char  *PyUnicode_AsUTF8              (PyObject *op);
extern PyObject    *PyUnicode_AsUTF8String        (PyObject *op);
extern PyObject    *PyUnicode_AsASCIIString       (PyObject *op);
extern PyObject    *PyUnicode_AsLatin1String      (PyObject *op);
extern const char  *PyUnicode_AsUTF8AndSize       (PyObject *op, Py_ssize_t *size);
extern Py_ssize_t   PyUnicode_GetLength           (PyObject *op);
extern int          PyUnicode_Check               (PyObject *op);
extern int          PyUnicode_CompareWithASCIIString(PyObject *op, const char *s);
extern PyObject    *PyUnicode_FromKindAndData     (int kind, const void *buffer, Py_ssize_t size);
extern Py_UCS4     *PyUnicode_AsUCS4              (PyObject *unicode, Py_UCS4 *target, Py_ssize_t targetsize, int copy_null);
extern Py_UCS4     *PyUnicode_AsUCS4Copy          (PyObject *unicode);
extern int          PyUnicode_Compare             (PyObject *left, PyObject *right);
extern PyObject    *PyUnicode_Concat              (PyObject *left, PyObject *right);
extern PyObject    *PyUnicode_Join                (PyObject *separator, PyObject *seq);
extern PyObject    *PyUnicode_Format              (PyObject *format, PyObject *args);
extern Py_ssize_t   PyUnicode_FindChar            (PyObject *unicode, Py_UCS4 ch, Py_ssize_t start, Py_ssize_t end, int direction);
extern Py_ssize_t   PyUnicode_Tailmatch          (PyObject *str, PyObject *substr, Py_ssize_t start, Py_ssize_t end, int direction);
extern PyObject    *PyUnicode_Replace            (PyObject *str, PyObject *substr, PyObject *repl, Py_ssize_t maxcount);
extern PyObject    *PyUnicode_Substring          (PyObject *str, Py_ssize_t start, Py_ssize_t end);
extern int          PyUnicode_Contains           (PyObject *container, PyObject *element);
extern PyObject    *PyUnicode_Decode              (const char *s, Py_ssize_t size, const char *encoding, const char *errors);
extern PyObject    *PyUnicode_DecodeUTF8          (const char *s, Py_ssize_t size, const char *errors);
extern PyObject    *PyUnicode_DecodeLatin1        (const char *s, Py_ssize_t size, const char *errors);
extern PyObject    *PyUnicode_FromOrdinal         (int ordinal);
extern PyObject    *PyUnicode_FromEncodedObject   (PyObject *obj, const char *encoding, const char *errors);
extern PyObject    *PyUnicode_AsEncodedString     (PyObject *unicode, const char *encoding, const char *errors);
extern PyObject    *PyUnicode_FromFormat          (const char *format, ...);
extern PyObject    *PyUnicode_FromFormatV         (const char *format, va_list vargs);
extern void         PyUnicode_InternInPlace       (PyObject **p);
extern PyObject    *PyUnicode_InternFromString    (const char *s);

#define PyUnicode_GET_LENGTH(op) PyUnicode_GetLength(op)
#define PyUnicode_CheckExact(op) PyUnicode_Check(op)
#define PyUnicode_1BYTE_KIND 1
#define PyUnicode_2BYTE_KIND 2
#define PyUnicode_4BYTE_KIND 4
#define PyUnicode_KIND(op) ((void)(op), 1)
#define PyUnicode_DATA(op) ((void *)PyUnicode_AsUTF8(op))
#define PyUnicode_1BYTE_DATA(op) ((Py_UCS1 *)PyUnicode_DATA(op))
#define PyUnicode_2BYTE_DATA(op) ((Py_UCS2 *)PyUnicode_DATA(op))
#define PyUnicode_4BYTE_DATA(op) ((Py_UCS4 *)PyUnicode_DATA(op))
#define PyUnicode_READ(kind, data, index) \
    ((kind) == PyUnicode_1BYTE_KIND ? (Py_UCS4)((const uint8_t *)(data))[(index)] : \
     (kind) == PyUnicode_2BYTE_KIND ? (Py_UCS4)((const uint16_t *)(data))[(index)] : \
     (Py_UCS4)((const uint32_t *)(data))[(index)])
#define PyUnicode_WRITE(kind, data, index, value) ((void)(kind), (void)(data), (void)(index), (void)(value))
#define PyUnicode_READ_CHAR(op, index) PyUnicode_READ(PyUnicode_KIND(op), PyUnicode_DATA(op), (index))
#define PyUnicode_IS_READY(op) ((void)(op), 1)
#define PyUnicode_READY(op) ((void)(op), 0)
#define PyUnicode_MAX_CHAR_VALUE(op) ((void)(op), 0x10ffffU)
extern int _PyUnicode_IsLowercase(Py_UCS4 ch);
extern int _PyUnicode_IsUppercase(Py_UCS4 ch);
extern int _PyUnicode_IsTitlecase(Py_UCS4 ch);
extern int _PyUnicode_IsWhitespace(const Py_UCS4 ch);
extern int _PyUnicode_IsLinebreak(const Py_UCS4 ch);
extern int _PyUnicode_IsDecimalDigit(Py_UCS4 ch);
extern int _PyUnicode_IsDigit(Py_UCS4 ch);
extern int _PyUnicode_IsNumeric(Py_UCS4 ch);
extern int _PyUnicode_IsPrintable(Py_UCS4 ch);
extern int _PyUnicode_IsAlpha(Py_UCS4 ch);

static inline int Py_UNICODE_ISSPACE(Py_UCS4 ch) {
    if (ch < 128U) {
        return ch == 0x20U || (ch >= 0x09U && ch <= 0x0dU) || (ch >= 0x1cU && ch <= 0x1fU);
    }
    return _PyUnicode_IsWhitespace(ch);
}

#define Py_UNICODE_ISLOWER(ch) _PyUnicode_IsLowercase((Py_UCS4)(ch))
#define Py_UNICODE_ISUPPER(ch) _PyUnicode_IsUppercase((Py_UCS4)(ch))
#define Py_UNICODE_ISTITLE(ch) _PyUnicode_IsTitlecase((Py_UCS4)(ch))
#define Py_UNICODE_ISLINEBREAK(ch) _PyUnicode_IsLinebreak((Py_UCS4)(ch))
#define Py_UNICODE_ISDECIMAL(ch) _PyUnicode_IsDecimalDigit((Py_UCS4)(ch))
#define Py_UNICODE_ISDIGIT(ch) _PyUnicode_IsDigit((Py_UCS4)(ch))
#define Py_UNICODE_ISNUMERIC(ch) _PyUnicode_IsNumeric((Py_UCS4)(ch))
#define Py_UNICODE_ISPRINTABLE(ch) _PyUnicode_IsPrintable((Py_UCS4)(ch))
#define Py_UNICODE_ISALPHA(ch) _PyUnicode_IsAlpha((Py_UCS4)(ch))
static inline int Py_UNICODE_ISALNUM(Py_UCS4 ch) {
    return Py_UNICODE_ISALPHA(ch)
        || Py_UNICODE_ISDECIMAL(ch)
        || Py_UNICODE_ISDIGIT(ch)
        || Py_UNICODE_ISNUMERIC(ch);
}

/* Bytes */
extern PyObject *PyBytes_FromString        (const char *s);
extern PyObject *PyBytes_FromStringAndSize (const char *s, Py_ssize_t len);
extern int       PyBytes_AsStringAndSize   (PyObject *op, char **buf, Py_ssize_t *length);
extern Py_ssize_t PyBytes_Size             (PyObject *op);
extern int       PyBytes_Check             (PyObject *op);
extern char     *PyBytes_AsString          (PyObject *op);
extern char     *PyBytes_AS_STRING         (PyObject *op);

#define PyBytes_GET_SIZE(op) PyBytes_Size(op)
#define PyBytes_CheckExact(op) PyBytes_Check(op)

/* Bytearray */
extern PyObject *PyByteArray_FromStringAndSize(const char *s, Py_ssize_t len);
extern int       PyByteArray_Check(PyObject *op);
extern char     *PyByteArray_AsString(PyObject *op);
extern Py_ssize_t PyByteArray_Size(PyObject *op);

#define PyByteArray_AS_STRING(op) PyByteArray_AsString((PyObject *)(op))
#define PyByteArray_GET_SIZE(op) PyByteArray_Size((PyObject *)(op))
#define PyByteArray_CheckExact(op) PyByteArray_Check((PyObject *)(op))

/* Memoryview / buffer */
extern PyObject *PyMemoryView_FromMemory(char *mem, Py_ssize_t size, int flags);
extern PyObject *PyMemoryView_FromObject(PyObject *op);
extern int       PyMemoryView_Check(PyObject *op);
extern PyObject *PyMemoryView_GET_BASE(PyObject *op);
extern Py_buffer *PyMemoryView_GET_BUFFER(PyObject *op);
extern int       PyObject_GetBuffer(PyObject *obj, Py_buffer *view, int flags);
extern int       PyObject_CheckBuffer(PyObject *obj);
extern void      PyBuffer_Release(Py_buffer *view);
extern int       PyBuffer_IsContiguous(const Py_buffer *view, char order);
extern int       PyBuffer_FillInfo(Py_buffer *view, PyObject *obj, void *buf,
                                   Py_ssize_t len, int readonly, int flags);

/* Abstract sequence protocol */
extern int         PySequence_Check    (PyObject *op);
extern Py_ssize_t  PySequence_Size     (PyObject *op);
extern Py_ssize_t  PySequence_Length   (PyObject *op);
extern PyObject   *PySequence_GetItem  (PyObject *op, Py_ssize_t i);
extern PyObject   *PySequence_Concat   (PyObject *op, PyObject *other);
extern PyObject   *PySequence_Repeat   (PyObject *op, Py_ssize_t count);
extern int         PySequence_Contains (PyObject *op, PyObject *value);
extern Py_ssize_t  PySequence_Count    (PyObject *op, PyObject *value);
extern Py_ssize_t  PySequence_Index    (PyObject *op, PyObject *value);
extern PyObject   *PySequence_InPlaceConcat(PyObject *op, PyObject *other);
extern PyObject   *PySequence_InPlaceRepeat(PyObject *op, Py_ssize_t count);
extern PyObject   *PySequence_Fast     (PyObject *op, const char *message);
extern Py_ssize_t  PySequence_Fast_GET_SIZE(PyObject *op);
extern PyObject   *PySequence_Fast_GET_ITEM(PyObject *op, Py_ssize_t i);
extern PyObject  **PySequence_Fast_ITEMS(PyObject *op);
extern PyObject   *PySequence_Tuple    (PyObject *op);
extern PyObject   *PySequence_List     (PyObject *op);
extern int         PySequence_SetItem   (PyObject *op, Py_ssize_t i, PyObject *value);

#define PyObject_Length(op) PyObject_Size((PyObject *)(op))
#define PySequence_ITEM(op, i) PySequence_GetItem((PyObject *)(op), (i))

/* Slice */
extern PyObject   *PySlice_New(PyObject *start, PyObject *stop, PyObject *step);
extern int         PySlice_Check(PyObject *op);
extern int         PySlice_GetIndices(PyObject *slice, Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step);
extern int         PySlice_GetIndicesEx(PyObject *slice, Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step, Py_ssize_t *slicelength);
extern int         PySlice_Unpack(PyObject *slice, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step);
extern Py_ssize_t  PySlice_AdjustIndices(Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t step);

/* List */
extern PyObject   *PyList_New     (Py_ssize_t size);
extern int         PyList_Append  (PyObject *list, PyObject *item);
extern PyObject   *PyList_GetItem (PyObject *op, Py_ssize_t i);
extern PyObject   *PyList_GetItemRef(PyObject *op, Py_ssize_t i);
extern int         PyList_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern Py_ssize_t  PyList_Size    (PyObject *op);
extern int         PyList_Check   (PyObject *op);
extern PyObject   *PyList_GetSlice(PyObject *op, Py_ssize_t low, Py_ssize_t high);
extern int         PyList_Sort    (PyObject *op);
extern int         PyList_Reverse (PyObject *op);
extern PyObject   *PyList_AsTuple (PyObject *op);
extern int         PyList_Insert  (PyObject *op, Py_ssize_t where, PyObject *v);
extern int         PyList_SetSlice(PyObject *op, Py_ssize_t low, Py_ssize_t high, PyObject *itemlist);

#define PyList_GET_ITEM(op, i)  PyList_GetItem(op, i)
#define PyList_SET_ITEM(op, i, v) PyList_SetItem(op, i, v)
#define PyList_GET_SIZE(op)     PyList_Size(op)
#define PyList_CheckExact(op)   PyList_Check(op)

/* Tuple */
extern PyObject   *PyTuple_New     (Py_ssize_t size);
extern PyObject   *PyTuple_Pack    (Py_ssize_t n, ...);
extern PyObject   *PyTuple_GetItem (PyObject *op, Py_ssize_t i);
extern PyObject   *PyTuple_GetSlice(PyObject *op, Py_ssize_t start, Py_ssize_t end);
extern int         PyTuple_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern Py_ssize_t  PyTuple_Size    (PyObject *op);
extern int         PyTuple_Check   (PyObject *op);

#define PyTuple_GET_ITEM(op, i) (((PyTupleObject *)(op))->ob_item[(i)])
#define PyTuple_GET_SIZE(op)    (((PyTupleObject *)(op))->ob_size)
#define PyTuple_SET_ITEM(op, i, v) (PyTuple_GET_ITEM((op), (i)) = (v))
#define PyTuple_CheckExact(op)  PyTuple_Check(op)

/* Set */
extern int         PySet_Check    (PyObject *op);
extern int         PyFrozenSet_Check(PyObject *op);
extern PyObject   *PySet_New      (PyObject *iterable);
extern Py_ssize_t  PySet_Size     (PyObject *anyset);
extern int         PySet_Contains (PyObject *anyset, PyObject *key);
extern int         PySet_Add      (PyObject *anyset, PyObject *key);
extern int         PySet_Discard  (PyObject *anyset, PyObject *key);

#define PySet_GET_SIZE(op) PySet_Size((PyObject *)(op))
#define PyAnySet_Check(op) (PySet_Check((PyObject *)(op)) || PyFrozenSet_Check((PyObject *)(op)))
#define PySet_CheckExact(op) PySet_Check((PyObject *)(op))
#define PyFrozenSet_CheckExact(op) PyFrozenSet_Check((PyObject *)(op))

/* Dict */
extern PyObject   *PyDict_New           (void);
extern PyObject   *_PyDict_NewPresized  (Py_ssize_t minused);
extern int         PyDict_SetItem       (PyObject *op, PyObject *key, PyObject *val);
extern int         PyDict_SetItemString (PyObject *op, const char *key, PyObject *val);
extern int         PyDict_Merge         (PyObject *op, PyObject *other, int override);
extern PyObject   *PyDictProxy_New      (PyObject *mapping);
extern PyObject   *PyDict_GetItem       (PyObject *op, PyObject *key);
extern PyObject   *PyDict_GetItemWithError(PyObject *op, PyObject *key);
extern int         PyDict_GetItemRef     (PyObject *op, PyObject *key, PyObject **result);
extern int         PyDict_GetItemStringRef(PyObject *op, const char *key, PyObject **result);
extern PyObject   *_PyDict_GetItem_KnownHash(PyObject *op, PyObject *key, Py_hash_t hash);
extern PyObject   *PyDict_GetItemString (PyObject *op, const char *key);
extern PyObject   *PyDict_SetDefault    (PyObject *op, PyObject *key, PyObject *default_value);
extern int         PyDict_SetDefaultRef (PyObject *op, PyObject *key, PyObject *default_value, PyObject **result);
extern int         PyDict_DelItem       (PyObject *op, PyObject *key);
extern int         PyDict_DelItemString (PyObject *op, const char *key);
extern Py_ssize_t  PyDict_Size          (PyObject *op);
extern int         PyDict_Next          (PyObject *op, Py_ssize_t *pos, PyObject **key, PyObject **value);
extern int         PyDict_Check         (PyObject *op);

#define PyDict_CheckExact(op) PyDict_Check((PyObject *)(op))
extern int         PyDict_Contains      (PyObject *op, PyObject *key);
extern int         PyDict_ContainsString(PyObject *op, const char *key);
extern PyObject   *PyDict_Copy          (PyObject *op);
extern PyObject   *PyDict_Keys          (PyObject *op);
extern PyObject   *PyDict_Values        (PyObject *op);

#define PyDict_GET_SIZE(op) PyDict_Size((PyObject *)(op))

/* Module */
extern PyObject *PyModule_New          (const char *name);
extern PyObject *PyModule_GetDict      (PyObject *module);
extern int       PyModule_AddObject    (PyObject *module, const char *name, PyObject *value);
extern int       PyModule_AddIntConstant   (PyObject *module, const char *name, long value);
extern int       PyModule_AddStringConstant(PyObject *module, const char *name, const char *value);
extern int       PyModule_AddObjectRef     (PyObject *module, const char *name, PyObject *value);
extern PyObject *PyModuleDef_Init      (PyModuleDef *def);
extern PyObject *PyModule_Create2      (PyModuleDef *def, int module_api_version);
extern PyObject *PyModule_NewObject    (PyObject *name);
extern int       PyModule_Check        (PyObject *module);
extern const char *PyModule_GetName    (PyObject *module);
extern void     *PyModule_GetState     (PyObject *module);
extern int       PyState_AddModule     (PyObject *module, PyModuleDef *def);
extern PyObject *PyState_FindModule    (PyModuleDef *def);
extern int       PyState_RemoveModule  (PyModuleDef *def);

#define PyModule_Create(def) PyModule_Create2(def, 1013)

/* Errors */
extern void      PyErr_SetString  (PyObject *exc_type, const char *message);
extern void      PyErr_SetNone    (PyObject *exc_type);
extern PyObject *PyErr_Occurred   (void);
extern void      PyErr_Clear      (void);
extern int32_t   molt_err_pending (void);
extern void      PyErr_Print      (void);
extern PyObject *PyErr_Format     (PyObject *exc_type, const char *format, ...);
extern PyObject *PyErr_SetFromErrno(PyObject *exc_type);
extern void      PyErr_SetObject  (PyObject *exc_type, PyObject *value);
extern void      PyErr_BadInternalCall(void);
extern int       PyErr_ExceptionMatches(PyObject *exc);
extern int       PyErr_GivenExceptionMatches(PyObject *given, PyObject *exc);
extern void      PyErr_Fetch      (PyObject **type, PyObject **value, PyObject **traceback);
extern void      PyErr_Restore    (PyObject *type, PyObject *value, PyObject *traceback);
extern void      PyErr_NormalizeException(PyObject **type, PyObject **value, PyObject **traceback);
extern int       PyException_SetTraceback(PyObject *exc, PyObject *tb);
extern void      PyException_SetContext(PyObject *exc, PyObject *context);
extern void      PyException_SetCause(PyObject *exc, PyObject *cause);
extern PyObject *PyErr_NoMemory  (void);
extern int       PyErr_WarnEx    (PyObject *category, const char *message, Py_ssize_t stack_level);
extern int       PyErr_WarnFormat(PyObject *category, Py_ssize_t stack_level, const char *format, ...);
extern PyObject *PyErr_FormatV   (PyObject *exc_type, const char *format, va_list vargs);
extern void      PyErr_FormatUnraisable(const char *format, ...);
extern void      PyErr_WriteUnraisable(PyObject *obj);
extern int       PyErr_CheckSignals(void);
extern PyObject *PyException_GetTraceback(PyObject *exc);

/* System, import, memory, and process-fatal ABI */
extern PyObject *PySys_GetObject       (const char *name);
extern void      PySys_WriteStderr     (const char *format, ...);
extern const char *Py_GetVersion       (void);
extern PyObject *PyImport_ImportModule (const char *name);
extern PyObject *PyImport_AddModule    (const char *name);
extern PyObject *PyImport_GetModuleDict(void);
extern PyObject *PyImport_ImportModuleLevel(const char *name, PyObject *globals, PyObject *locals, PyObject *fromlist, int level);
extern PyObject *PyImport_ImportModuleLevelObject(PyObject *name, PyObject *globals, PyObject *locals, PyObject *fromlist, int level);
extern PyObject *PyImport_Import(PyObject *name);
extern void     *PyMem_Malloc          (size_t size);
extern void     *PyMem_Calloc          (size_t nelem, size_t elsize);
extern void     *PyMem_Realloc         (void *ptr, size_t new_size);
extern void      PyMem_Free            (void *ptr);
extern void     *PyMem_RawMalloc       (size_t size);
extern void     *PyMem_RawCalloc       (size_t nelem, size_t elsize);
extern void     *PyMem_RawRealloc      (void *ptr, size_t new_size);
extern void      PyMem_RawFree         (void *ptr);
extern void      PyObject_GC_Del       (void *ptr);
extern PyObject *PyObject_Init         (PyObject *op, PyTypeObject *typeobj);
extern PyVarObject *PyObject_InitVar   (PyVarObject *op, PyTypeObject *typeobj, Py_ssize_t size);
extern PyObject *_PyObject_New         (PyTypeObject *typeobj);
extern PyVarObject *_PyObject_NewVar   (PyTypeObject *typeobj, Py_ssize_t nitems);
extern PyObject *_PyObject_GC_New      (PyTypeObject *typeobj);
extern void      PyObject_GC_Track     (void *op);
extern void      PyObject_GC_UnTrack   (void *op);
extern int       PyObject_GC_IsFinalized(PyObject *op);
extern int       PyObject_CallFinalizerFromDealloc(PyObject *op);
extern int       PyGC_Disable          (void);
extern void      PyGC_Enable           (void);
extern void      Py_FatalError         (const char *message);
extern int       Py_EnterRecursiveCall (const char *where);
extern void      Py_LeaveRecursiveCall (void);

#define PyObject_INIT(op, typeobj) PyObject_Init((PyObject *)(op), (typeobj))
#define PyObject_INIT_VAR(op, typeobj, size) PyObject_InitVar((PyVarObject *)(op), (typeobj), (size))
#define PyObject_New(type, typeobj) ((type *)_PyObject_New((typeobj)))
#define PyObject_NewVar(type, typeobj, nitems) ((type *)_PyObject_NewVar((typeobj), (nitems)))
#define PyObject_GC_New(type, typeobj) ((type *)_PyObject_GC_New((typeobj)))
#define PyObject_Malloc(size) PyMem_Malloc(size)
#define PyObject_Calloc(nelem, elsize) PyMem_Calloc((nelem), (elsize))
#define PyObject_Realloc(ptr, size) PyMem_Realloc((ptr), (size))
#define PyObject_Free(ptr) PyMem_Free(ptr)
#define PyObject_Del(ptr) PyObject_Free(ptr)
#define PyObject_MALLOC(size) PyObject_Malloc(size)
#define PyObject_CALLOC(nelem, elsize) PyObject_Calloc((nelem), (elsize))
#define PyObject_REALLOC(ptr, size) PyObject_Realloc((ptr), (size))
#define PyObject_FREE(ptr) PyObject_Free(ptr)
#define PyMem_MALLOC(size) PyMem_Malloc(size)
#define PyMem_CALLOC(nelem, elsize) PyMem_Calloc((nelem), (elsize))
#define PyMem_REALLOC(ptr, size) PyMem_Realloc((ptr), (size))
#define PyMem_FREE(ptr) PyMem_Free(ptr)

static inline size_t _Py_SIZE_ROUND_UP(size_t n, size_t alignment) {
    return (n + alignment - 1U) & ~(alignment - 1U);
}

static inline size_t _PyObject_VAR_SIZE(PyTypeObject *typeobj, Py_ssize_t nitems) {
    size_t size;
    if (typeobj == NULL || nitems < 0) {
        return 0;
    }
    size = (size_t)typeobj->tp_basicsize;
    size += (size_t)nitems * (size_t)typeobj->tp_itemsize;
    return _Py_SIZE_ROUND_UP(size, SIZEOF_VOID_P);
}

extern int       PyTraceMalloc_Track   (unsigned int domain, uintptr_t ptr, size_t size);
extern int       PyTraceMalloc_Untrack (unsigned int domain, uintptr_t ptr);

/* Context variables */
extern PyObject *PyContextVar_New      (const char *name, PyObject *default_value);
extern int       PyContextVar_Get      (PyObject *var, PyObject *default_value, PyObject **value);
extern PyObject *PyContextVar_Set      (PyObject *var, PyObject *value);

/* Function objects */
extern int PyCFunction_Check(PyObject *op);
extern PyCFunction PyCFunction_GetFunction(PyObject *op);
extern PyObject *PyCFunction_GetSelf(PyObject *op);
extern int PyCFunction_GetFlags(PyObject *op);
extern PyObject *PyCFunction_New(PyMethodDef *ml, PyObject *self);
extern PyObject *PyCFunction_NewEx(PyMethodDef *ml, PyObject *self, PyObject *module);
extern PyObject *PyMethod_New(PyObject *func, PyObject *self);
extern int PyMethod_Check(PyObject *op);
extern PyObject *PyMethod_GET_FUNCTION(PyObject *op);
extern PyObject *PyMethod_GET_SELF(PyObject *op);

#define PyCFunction_GET_FUNCTION(op) PyCFunction_GetFunction((PyObject *)(op))
#define PyCFunction_GET_SELF(op)     PyCFunction_GetSelf((PyObject *)(op))
#define PyCFunction_GET_FLAGS(op)    PyCFunction_GetFlags((PyObject *)(op))

/* Capsules */
typedef void (*PyCapsule_Destructor)(PyObject *);
extern PyObject *PyCapsule_New(void *pointer, const char *name, PyCapsule_Destructor destructor);
extern int       PyCapsule_CheckExact(PyObject *capsule);
extern void     *PyCapsule_GetPointer(PyObject *capsule, const char *name);
extern const char *PyCapsule_GetName(PyObject *capsule);
extern void     *PyCapsule_GetContext(PyObject *capsule);
extern int       PyCapsule_IsValid(PyObject *capsule, const char *name);
extern int       PyCapsule_SetPointer(PyObject *capsule, void *pointer);
extern int       PyCapsule_SetContext(PyObject *capsule, void *context);
extern int       PyCapsule_SetName(PyObject *capsule, const char *name);
extern void     *PyCapsule_Import(const char *name, int no_block);

extern PyObject *Py_BuildValue(const char *format, ...);
extern PyObject *_Py_BuildValue_SizeT(const char *format, ...);
extern PyObject *Py_VaBuildValue(const char *format, va_list vargs);

/* Thread state */
extern int Py_IsInitialized(void);
extern PyThreadState *PyThreadState_Get(void);
extern PyThreadState *_PyThreadState_UncheckedGet(void);
extern PyGILState_STATE PyGILState_Ensure(void);
extern void PyGILState_Release(PyGILState_STATE state);
extern int PyGILState_Check(void);
extern PyThreadState *PyEval_SaveThread(void);
extern void PyEval_RestoreThread(PyThreadState *tstate);
extern PyInterpreterState *PyInterpreterState_Get(void);
extern PyInterpreterState *PyInterpreterState_Main(void);
extern PyInterpreterState *PyThreadState_GetInterpreter(PyThreadState *tstate);
extern PyFrameObject *PyThreadState_GetFrame(PyThreadState *tstate);
extern uint64_t PyThreadState_GetID(PyThreadState *tstate);
extern int64_t PyInterpreterState_GetID(PyInterpreterState *interp);
extern int64_t PyInterpreterState_GetIDFromThreadState(PyThreadState *tstate);
extern void PyMutex_Lock(PyMutex *mutex);
extern void PyMutex_Unlock(PyMutex *mutex);
extern PyObject *PyEval_GetBuiltins(void);
extern PyObject *PyEval_EvalCode(PyObject *co, PyObject *globals, PyObject *locals);
extern int _Py_IsFinalizing(void);
extern int Py_IsFinalizing(void);
#define PyThreadState_GET() PyThreadState_Get()

/* Argument parsing (variadic — implemented in C shim) */
extern int PyArg_ParseTuple             (PyObject *args, const char *format, ...);
extern int PyArg_ParseTupleAndKeywords  (PyObject *args, PyObject *kwds,
                                         const char *format, char **kwlist, ...);
extern int PyArg_VaParseTupleAndKeywords(PyObject *args, PyObject *kwds,
                                         const char *format, char **kwlist, va_list vargs);
extern int PyArg_UnpackTuple            (PyObject *args, const char *name,
                                         Py_ssize_t min, Py_ssize_t max, ...);

/* Standard exception singletons (non-null sentinels; exact type unimportant). */
extern PyObject PyExc_BaseException;
extern PyObject PyExc_Exception;
extern PyObject PyExc_ValueError;
extern PyObject PyExc_LookupError;
extern PyObject PyExc_AssertionError;
extern PyObject PyExc_TypeError;
extern PyObject PyExc_RuntimeError;
extern PyObject PyExc_MemoryError;
extern PyObject PyExc_IndexError;
extern PyObject PyExc_KeyError;
extern PyObject PyExc_AttributeError;
extern PyObject PyExc_OverflowError;
extern PyObject PyExc_ZeroDivisionError;
extern PyObject PyExc_ImportError;
extern PyObject PyExc_ModuleNotFoundError;
extern PyObject PyExc_StopIteration;
extern PyObject PyExc_NotImplementedError;
extern PyObject PyExc_OSError;
extern PyObject PyExc_IOError;
extern PyObject PyExc_FileNotFoundError;
extern PyObject PyExc_PermissionError;
extern PyObject PyExc_FileExistsError;
extern PyObject PyExc_IsADirectoryError;
extern PyObject PyExc_NotADirectoryError;
extern PyObject PyExc_TimeoutError;
extern PyObject PyExc_ArithmeticError;
extern PyObject PyExc_NameError;
extern PyObject PyExc_UnboundLocalError;
extern PyObject PyExc_SyntaxError;
extern PyObject PyExc_SystemError;
extern PyObject PyExc_SystemExit;
extern PyObject PyExc_BufferError;
extern PyObject PyExc_RecursionError;
extern PyObject PyExc_GeneratorExit;
extern PyObject PyExc_KeyboardInterrupt;
extern PyObject PyExc_ConnectionError;
extern PyObject PyExc_ConnectionResetError;
extern PyObject PyExc_BrokenPipeError;
extern PyObject PyExc_FloatingPointError;
extern PyObject PyExc_Warning;
extern PyObject PyExc_DeprecationWarning;
extern PyObject PyExc_RuntimeWarning;
extern PyObject PyExc_FutureWarning;
extern PyObject PyExc_ImportWarning;
extern PyObject PyExc_UserWarning;
extern PyObject PyExc_UnicodeError;
extern PyObject PyExc_UnicodeDecodeError;
extern PyObject PyExc_UnicodeEncodeError;

#define PyExc_BaseException        (&PyExc_BaseException)
#define PyExc_Exception            (&PyExc_Exception)
#define PyExc_ValueError           (&PyExc_ValueError)
#define PyExc_LookupError          (&PyExc_LookupError)
#define PyExc_AssertionError       (&PyExc_AssertionError)
#define PyExc_TypeError            (&PyExc_TypeError)
#define PyExc_RuntimeError         (&PyExc_RuntimeError)
#define PyExc_MemoryError          (&PyExc_MemoryError)
#define PyExc_IndexError           (&PyExc_IndexError)
#define PyExc_KeyError             (&PyExc_KeyError)
#define PyExc_AttributeError       (&PyExc_AttributeError)
#define PyExc_OverflowError        (&PyExc_OverflowError)
#define PyExc_ZeroDivisionError    (&PyExc_ZeroDivisionError)
#define PyExc_ImportError          (&PyExc_ImportError)
#define PyExc_ModuleNotFoundError  (&PyExc_ModuleNotFoundError)
#define PyExc_StopIteration        (&PyExc_StopIteration)
#define PyExc_NotImplementedError  (&PyExc_NotImplementedError)
#define PyExc_OSError              (&PyExc_OSError)
#define PyExc_IOError              (&PyExc_IOError)
#define PyExc_FileNotFoundError    (&PyExc_FileNotFoundError)
#define PyExc_PermissionError      (&PyExc_PermissionError)
#define PyExc_FileExistsError      (&PyExc_FileExistsError)
#define PyExc_IsADirectoryError    (&PyExc_IsADirectoryError)
#define PyExc_NotADirectoryError   (&PyExc_NotADirectoryError)
#define PyExc_TimeoutError         (&PyExc_TimeoutError)
#define PyExc_ArithmeticError      (&PyExc_ArithmeticError)
#define PyExc_NameError            (&PyExc_NameError)
#define PyExc_UnboundLocalError    (&PyExc_UnboundLocalError)
#define PyExc_SyntaxError          (&PyExc_SyntaxError)
#define PyExc_SystemError          (&PyExc_SystemError)
#define PyExc_SystemExit           (&PyExc_SystemExit)
#define PyExc_BufferError          (&PyExc_BufferError)
#define PyExc_RecursionError       (&PyExc_RecursionError)
#define PyExc_GeneratorExit        (&PyExc_GeneratorExit)
#define PyExc_KeyboardInterrupt    (&PyExc_KeyboardInterrupt)
#define PyExc_ConnectionError      (&PyExc_ConnectionError)
#define PyExc_ConnectionResetError (&PyExc_ConnectionResetError)
#define PyExc_BrokenPipeError      (&PyExc_BrokenPipeError)
#define PyExc_FloatingPointError   (&PyExc_FloatingPointError)
#define PyExc_Warning              (&PyExc_Warning)
#define PyExc_DeprecationWarning   (&PyExc_DeprecationWarning)
#define PyExc_RuntimeWarning       (&PyExc_RuntimeWarning)
#define PyExc_FutureWarning        (&PyExc_FutureWarning)
#define PyExc_ImportWarning        (&PyExc_ImportWarning)
#define PyExc_UserWarning          (&PyExc_UserWarning)
#define PyExc_UnicodeError         (&PyExc_UnicodeError)
#define PyExc_UnicodeDecodeError   (&PyExc_UnicodeDecodeError)
#define PyExc_UnicodeEncodeError   (&PyExc_UnicodeEncodeError)

/* ── Convenience macros ───────────────────────────────────────────────────── */

#define Py_TYPE(ob)     (((PyObject *)(ob))->ob_type)
#define Py_REFCNT(ob)   (((PyObject *)(ob))->ob_refcnt)
#define Py_SIZE(ob)     (((PyVarObject *)(ob))->ob_size)
#define Py_SET_REFCNT(ob, refcnt) (Py_REFCNT(ob) = (refcnt))
#define Py_SET_TYPE(ob, type) (Py_TYPE(ob) = (type))
#define Py_SET_SIZE(ob, size) (Py_SIZE(ob) = (size))
#define PyExceptionInstance_Class(x) ((PyObject *)Py_TYPE(x))

#ifndef _PyObject_CAST
#define _PyObject_CAST(op) ((PyObject *)(op))
#endif

#ifndef PyObject_TypeCheck
#define PyObject_TypeCheck(op, type) PyObject_TypeCheck(_PyObject_CAST(op), (type))
#endif

#ifndef PyType_Check
#define PyType_Check(op) PyType_Check(_PyObject_CAST(op))
#endif

/* Number check shorthands */
extern PyObject *PyNumber_Add(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Subtract(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Multiply(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_TrueDivide(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_FloorDivide(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Remainder(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Power(PyObject *o1, PyObject *o2, PyObject *o3);
extern PyObject *PyNumber_Negative(PyObject *op);
extern PyObject *PyNumber_Positive(PyObject *op);
extern PyObject *PyNumber_Absolute(PyObject *op);
extern PyObject *PyNumber_Invert(PyObject *op);
extern PyObject *PyNumber_Lshift(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Rshift(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_And(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Or(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Xor(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Long(PyObject *op);
extern PyObject *PyNumber_Int(PyObject *op);
extern PyObject *PyNumber_Float(PyObject *op);
extern PyObject *PyNumber_Index(PyObject *op);
extern int PyIndex_Check(PyObject *op);
extern Py_ssize_t PyNumber_AsSsize_t(PyObject *op, PyObject *exc);
extern PyObject *PyNumber_InPlaceAdd(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceSubtract(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceMultiply(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceTrueDivide(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceFloorDivide(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceRemainder(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceLshift(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceRshift(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceAnd(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceOr(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_InPlaceXor(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_Divmod(PyObject *o1, PyObject *o2);
extern PyObject *PyNumber_MatrixMultiply(PyObject *o1, PyObject *o2);
extern int PyMapping_HasKeyWithError(PyObject *obj, PyObject *key);
extern int PyMapping_HasKeyStringWithError(PyObject *obj, const char *key);
extern int PyMapping_Check(PyObject *obj);
extern Py_ssize_t PyMapping_Size(PyObject *obj);
extern Py_ssize_t PyMapping_Length(PyObject *obj);
extern int PyMapping_HasKey(PyObject *obj, PyObject *key);
extern int PyMapping_HasKeyString(PyObject *obj, const char *key);
extern PyObject *PyMapping_GetItemString(PyObject *obj, const char *key);
extern int PyMapping_GetOptionalItem(PyObject *obj, PyObject *key, PyObject **result);
extern int PyMapping_SetItemString(PyObject *obj, const char *key, PyObject *value);
extern PyObject *PyMapping_Keys(PyObject *obj);
extern PyObject *PyMapping_Values(PyObject *obj);
extern PyObject *PyMapping_Items(PyObject *obj);

#define PyNumber_Check(op)  (PyLong_Check(op) || PyFloat_Check(op) || PyBool_Check(op))
#define PyType_FastSubclass(type, flag) ((((PyTypeObject *)(type))->tp_flags & (flag)) != 0)
#define PyExceptionClass_Check(x) (PyType_Check((PyObject *)(x)) && PyType_FastSubclass((PyTypeObject *)(x), Py_TPFLAGS_BASE_EXC_SUBCLASS))

extern const unsigned long Py_Version;
extern int Py_OptimizeFlag;
extern int PyUnstable_Object_IsUniqueReferencedTemporary(PyObject *obj);
extern int PyUnstable_Object_IsUniquelyReferenced(PyObject *obj);
extern void PyUnstable_Object_EnableDeferredRefcount(PyObject *obj);
extern void PyUnstable_SetImmortal(PyObject *obj);
extern int _Py_IsOwnedByCurrentThread(PyObject *obj);
extern int PyUnstable_Module_SetGIL(PyObject *module, int gil);
extern int PyOS_snprintf(char *str, size_t size, const char *format, ...);
extern int PyOS_vsnprintf(char *str, size_t size, const char *format, va_list va);
extern double PyOS_string_to_double(const char *str, char **endptr, PyObject *overflow_exception);
extern long PyOS_strtol(const char *str, char **endptr, int base);
extern unsigned long PyOS_strtoul(const char *str, char **endptr, int base);
extern int PyWeakref_Check(PyObject *op);
extern PyObject *PyWeakref_GetObject(PyObject *ref);

/* ── Module API version used by PyModule_Create macro ────────────────────── */

#define PYTHON_API_VERSION 1013

#ifdef __cplusplus
}
#endif

#endif /* Py_PYTHON_H */
