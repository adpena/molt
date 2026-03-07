/*
 * Python.h — Molt CPython ABI compatibility header.
 *
 * Provides the minimal subset of CPython 3.12 types and macros needed to
 * compile native Python extension modules (.so / .pyd) against the Molt
 * runtime instead of CPython.
 *
 * Extensions compiled against this header link against:
 *   libmolt_cpython_abi.dylib  (macOS)
 *   libmolt_cpython_abi.so     (Linux)
 *   molt_cpython_abi.dll       (Windows)
 *
 * Compilation:
 *   cc -O2 -shared -fPIC -I<path>/include \
 *      myext.c -o _myext.cpython-312-darwin.so \
 *      -L<cargo-target>/release -lmolt_cpython_abi
 *
 * ABI note: struct layouts match CPython 3.12 x86-64 / aarch64.
 */
#pragma once
#ifndef Py_PYTHON_H
#define Py_PYTHON_H

#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Version ──────────────────────────────────────────────────────────────── */

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0
#define PY_RELEASE_LEVEL 'f'
#define PY_RELEASE_SERIAL 0
#define PY_VERSION "3.12.0 (Molt runtime)"

/* ── Primitive types ──────────────────────────────────────────────────────── */

typedef ssize_t Py_ssize_t;
typedef ssize_t Py_hash_t;
typedef size_t  Py_uhash_t;

#define PY_SSIZE_T_MAX  ((Py_ssize_t)(((size_t)-1)>>1))
#define PY_SSIZE_T_MIN  (-PY_SSIZE_T_MAX - 1)

/* ── Forward declarations ─────────────────────────────────────────────────── */

typedef struct _object      PyObject;
typedef struct _typeobject  PyTypeObject;
typedef struct _longobject  PyLongObject;

/* ── Ob_refcnt / Ob_type helpers ──────────────────────────────────────────── */

/* Inline refcount — matches CPython 3.12 non-Py_GIL_DISABLED layout. */
#define PyObject_HEAD       \
    Py_ssize_t ob_refcnt;   \
    PyTypeObject *ob_type;

#define PyObject_VAR_HEAD   \
    PyObject_HEAD           \
    Py_ssize_t ob_size;

/* ── PyObject ─────────────────────────────────────────────────────────────── */

struct _object {
    PyObject_HEAD
};

typedef struct {
    PyObject_VAR_HEAD
} PyVarObject;

/* ── Function pointer typedefs ────────────────────────────────────────────── */

typedef PyObject *(*unaryfunc)   (PyObject *);
typedef PyObject *(*binaryfunc)  (PyObject *, PyObject *);
typedef PyObject *(*ternaryfunc) (PyObject *, PyObject *, PyObject *);
typedef int       (*inquiry)     (PyObject *);
typedef Py_ssize_t(*lenfunc)     (PyObject *);
typedef PyObject *(*ssizeargfunc)(PyObject *, Py_ssize_t);
typedef int (*ssizeobjargproc)   (PyObject *, Py_ssize_t, PyObject *);
typedef int (*objobjargproc)     (PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)        (PyObject *, PyObject *);
typedef void(*destructor)        (PyObject *);
typedef PyObject *(*reprfunc)    (PyObject *);
typedef PyObject *(*richcmpfunc) (PyObject *, PyObject *, int);
typedef PyObject *(*getattrofunc)(PyObject *, PyObject *);
typedef int (*setattrofunc)      (PyObject *, PyObject *, PyObject *);
typedef Py_hash_t (*hashfunc)    (PyObject *);
typedef int (*visitproc)         (PyObject *, void *);
typedef int (*traverseproc)      (PyObject *, visitproc, void *);
typedef PyObject *(*newfunc)     (PyTypeObject *, PyObject *, PyObject *);
typedef int (*initproc)          (PyObject *, PyObject *, PyObject *);
typedef void (*freefunc)         (void *);

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

typedef struct {
    int (*bf_getbuffer)   (PyObject *, void *view, int flags);
    void (*bf_releasebuffer)(PyObject *, void *view);
} PyBufferProcs;

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
    void               *tp_descr_get;
    void               *tp_descr_set;
    Py_ssize_t          tp_dictoffset;
    initproc            tp_init;
    void               *tp_alloc;
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
};

/* ── tp_flags ─────────────────────────────────────────────────────────────── */

#define Py_TPFLAGS_DEFAULT      (0)
#define Py_TPFLAGS_BASETYPE     (1UL << 10)
#define Py_TPFLAGS_HAVE_GC      (1UL << 14)
#define Py_TPFLAGS_READY        (1UL << 12)

/* ── PyMethodDef ──────────────────────────────────────────────────────────── */

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);

#define METH_VARARGS    0x0001
#define METH_KEYWORDS   0x0002
#define METH_NOARGS     0x0004
#define METH_O          0x0008
#define METH_CLASS      0x0010
#define METH_STATIC     0x0020
#define METH_COEXIST    0x0040
#define METH_FASTCALL   0x0080

typedef struct PyMethodDef {
    const char  *ml_name;
    PyCFunction  ml_meth;
    int          ml_flags;
    const char  *ml_doc;
} PyMethodDef;

/* Sentinel that terminates a PyMethodDef array. */
#define PY_METHODDEF_SENTINEL   { NULL, NULL, 0, NULL }

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
    void               *m_slots;
    traverseproc        m_traverse;
    inquiry             m_clear;
    freefunc            m_free;
} PyModuleDef;

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

#define Py_None  (&Py_None)
#define Py_True  (&Py_True)
#define Py_False (&Py_False)

/* ── Refcount macros ──────────────────────────────────────────────────────── */

extern void Py_INCREF(PyObject *op);
extern void Py_DECREF(PyObject *op);

#define Py_XINCREF(op) do { if ((op) != NULL) Py_INCREF(op); } while(0)
#define Py_XDECREF(op) do { if ((op) != NULL) Py_DECREF(op); } while(0)

#define Py_CLEAR(op) do {          \
    PyObject *_py_tmp = (PyObject *)(op); \
    if (_py_tmp != NULL) {          \
        (op) = NULL;                \
        Py_DECREF(_py_tmp);         \
    }                               \
} while(0)

/* ── Return helpers ───────────────────────────────────────────────────────── */

#define Py_RETURN_NONE                \
    do { Py_INCREF(Py_None); return Py_None; } while(0)

#define Py_RETURN_TRUE                \
    do { Py_INCREF(Py_True); return Py_True; } while(0)

#define Py_RETURN_FALSE               \
    do { Py_INCREF(Py_False); return Py_False; } while(0)

#define Py_RETURN_NOTIMPLEMENTED      Py_RETURN_NONE

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
extern int          PyObject_TypeCheck  (PyObject *op, PyTypeObject *tp);
extern PyObject    *PyObject_Repr       (PyObject *op);
extern PyObject    *PyObject_Str        (PyObject *op);
extern Py_hash_t    PyObject_Hash       (PyObject *op);
extern int          PyObject_IsInstance (PyObject *inst, PyObject *cls);
extern int          PyCallable_Check    (PyObject *op);
extern PyObject    *PyObject_RichCompare(PyObject *v, PyObject *w, int op);
extern int          PyObject_RichCompareBool(PyObject *v, PyObject *w, int op);

/* Integer */
extern PyObject *PyLong_FromLong         (long v);
extern PyObject *PyLong_FromLongLong     (long long v);
extern PyObject *PyLong_FromSsize_t      (Py_ssize_t v);
extern PyObject *PyLong_FromUnsignedLong (unsigned long v);
extern PyObject *PyLong_FromUnsignedLongLong(unsigned long long v);
extern long      PyLong_AsLong           (PyObject *op);
extern long long PyLong_AsLongLong       (PyObject *op);
extern Py_ssize_t PyLong_AsSsize_t       (PyObject *op);
extern unsigned long PyLong_AsUnsignedLong(PyObject *op);
extern int       PyLong_Check            (PyObject *op);

/* Float */
extern PyObject *PyFloat_FromDouble (double v);
extern double    PyFloat_AsDouble   (PyObject *op);
extern int       PyFloat_Check      (PyObject *op);

/* Bool */
extern PyObject *PyBool_FromLong (long v);
extern int       PyBool_Check    (PyObject *op);

/* Unicode / str */
extern PyObject    *PyUnicode_FromString          (const char *s);
extern PyObject    *PyUnicode_FromStringAndSize   (const char *s, Py_ssize_t size);
extern const char  *PyUnicode_AsUTF8              (PyObject *op);
extern const char  *PyUnicode_AsUTF8AndSize       (PyObject *op, Py_ssize_t *size);
extern Py_ssize_t   PyUnicode_GetLength           (PyObject *op);
extern int          PyUnicode_Check               (PyObject *op);
extern int          PyUnicode_CompareWithASCIIString(PyObject *op, const char *s);

/* Bytes */
extern PyObject *PyBytes_FromString        (const char *s);
extern PyObject *PyBytes_FromStringAndSize (const char *s, Py_ssize_t len);
extern int       PyBytes_AsStringAndSize   (PyObject *op, char **buf, Py_ssize_t *length);
extern Py_ssize_t PyBytes_Size             (PyObject *op);
extern int       PyBytes_Check             (PyObject *op);

/* List */
extern PyObject   *PyList_New     (Py_ssize_t size);
extern int         PyList_Append  (PyObject *list, PyObject *item);
extern PyObject   *PyList_GetItem (PyObject *op, Py_ssize_t i);
extern int         PyList_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern Py_ssize_t  PyList_Size    (PyObject *op);
extern int         PyList_Check   (PyObject *op);

#define PyList_GET_ITEM(op, i)  PyList_GetItem(op, i)
#define PyList_SET_ITEM(op, i, v) PyList_SetItem(op, i, v)
#define PyList_GET_SIZE(op)     PyList_Size(op)

/* Tuple */
extern PyObject   *PyTuple_New     (Py_ssize_t size);
extern PyObject   *PyTuple_GetItem (PyObject *op, Py_ssize_t i);
extern int         PyTuple_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern Py_ssize_t  PyTuple_Size    (PyObject *op);
extern int         PyTuple_Check   (PyObject *op);

#define PyTuple_GET_ITEM(op, i) PyTuple_GetItem(op, i)
#define PyTuple_GET_SIZE(op)    PyTuple_Size(op)

/* Dict */
extern PyObject   *PyDict_New           (void);
extern int         PyDict_SetItem       (PyObject *op, PyObject *key, PyObject *val);
extern int         PyDict_SetItemString (PyObject *op, const char *key, PyObject *val);
extern PyObject   *PyDict_GetItem       (PyObject *op, PyObject *key);
extern PyObject   *PyDict_GetItemString (PyObject *op, const char *key);
extern int         PyDict_DelItemString (PyObject *op, const char *key);
extern Py_ssize_t  PyDict_Size          (PyObject *op);
extern int         PyDict_Check         (PyObject *op);
extern PyObject   *PyDict_Copy          (PyObject *op);
extern PyObject   *PyDict_Keys          (PyObject *op);
extern PyObject   *PyDict_Values        (PyObject *op);

/* Module */
extern PyObject *PyModule_New          (const char *name);
extern PyObject *PyModule_GetDict      (PyObject *module);
extern int       PyModule_AddObject    (PyObject *module, const char *name, PyObject *value);
extern int       PyModule_AddIntConstant   (PyObject *module, const char *name, long value);
extern int       PyModule_AddStringConstant(PyObject *module, const char *name, const char *value);
extern PyObject *PyModuleDef_Init      (PyModuleDef *def);
extern PyObject *PyModule_Create2      (PyModuleDef *def, int module_api_version);

#define PyModule_Create(def) PyModule_Create2(def, 1013)

/* Errors */
extern void      PyErr_SetString  (PyObject *exc_type, const char *message);
extern void      PyErr_SetNone    (PyObject *exc_type);
extern PyObject *PyErr_Occurred   (void);
extern void      PyErr_Clear      (void);
extern void      PyErr_Print      (void);
extern PyObject *PyErr_Format     (PyObject *exc_type, const char *format, ...);

/* Argument parsing (variadic — implemented in C shim) */
extern int PyArg_ParseTuple             (PyObject *args, const char *format, ...);
extern int PyArg_ParseTupleAndKeywords  (PyObject *args, PyObject *kwds,
                                         const char *format, char **kwlist, ...);
extern int PyArg_UnpackTuple            (PyObject *args, const char *name,
                                         Py_ssize_t min, Py_ssize_t max, ...);

/* Standard exception singletons (non-null sentinels; exact type unimportant). */
extern PyObject *PyExc_BaseException;
extern PyObject *PyExc_Exception;
extern PyObject *PyExc_ValueError;
extern PyObject *PyExc_TypeError;
extern PyObject *PyExc_RuntimeError;
extern PyObject *PyExc_MemoryError;
extern PyObject *PyExc_IndexError;
extern PyObject *PyExc_KeyError;
extern PyObject *PyExc_AttributeError;
extern PyObject *PyExc_OverflowError;
extern PyObject *PyExc_ZeroDivisionError;
extern PyObject *PyExc_ImportError;
extern PyObject *PyExc_StopIteration;
extern PyObject *PyExc_NotImplementedError;
extern PyObject *PyExc_OSError;

/* ── Convenience macros ───────────────────────────────────────────────────── */

#define Py_TYPE(ob)     (((PyObject *)(ob))->ob_type)
#define Py_REFCNT(ob)   (((PyObject *)(ob))->ob_refcnt)

/* Number check shorthands */
#define PyNumber_Check(op)  (PyLong_Check(op) || PyFloat_Check(op) || PyBool_Check(op))

/* ── Module API version used by PyModule_Create macro ────────────────────── */

#define PYTHON_API_VERSION 1013

#ifdef __cplusplus
}
#endif

#endif /* Py_PYTHON_H */
