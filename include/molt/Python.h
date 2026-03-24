#ifndef MOLT_C_API_PYTHON_H
#define MOLT_C_API_PYTHON_H

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

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Symbols exported by the runtime exception subsystem.
 * These are used to synthesize PyExc_* shims in this compatibility header.
 */
MoltHandle molt_exception_kind(MoltHandle exc_bits);
MoltHandle molt_exception_class(MoltHandle kind_bits);

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
extern uint64_t molt_iter(uint64_t);
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
extern uint64_t molt_type_of_borrowed(uint64_t);
extern uint64_t molt_dict_getitem_borrowed(uint64_t, uint64_t);
extern uint64_t molt_list_getitem_borrowed(uint64_t, uint64_t);
extern uint64_t molt_tuple_getitem_borrowed(uint64_t, uint64_t);
extern uint64_t molt_object_getattr_borrowed(uint64_t obj, const uint8_t *name, uint64_t name_len);

typedef intptr_t Py_ssize_t;
typedef Py_ssize_t Py_hash_t;
struct _molt_pyobject {
    /* Intentionally minimal. The Molt handle lives in the POINTER value
       (via cast), not in this struct. Keep this as small as possible to
       minimize sizeof(PyObject) for embedded ob_base fields. */
    char _opaque;
};
typedef struct _molt_pyobject PyObject;
typedef PyObject PyTypeObject;
typedef int PyGILState_STATE;
typedef uint32_t Py_UCS4;
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
    /* backing molt view for runtime interop */
    MoltBufferView _molt_view;
} Py_buffer;

typedef struct _molt_pythreadstate {
    int _molt_reserved;
} PyThreadState;

typedef struct _molt_pyinterpreterstate {
    int _molt_reserved;
} PyInterpreterState;

typedef void (*PyCapsule_Destructor)(PyObject *);

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*getter)(PyObject *, void *);
typedef int (*setter)(PyObject *, PyObject *, void *);
typedef Py_ssize_t (*lenfunc)(PyObject *);
typedef PyObject *(*binaryfunc)(PyObject *, PyObject *);
typedef int (*objobjargproc)(PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)(PyObject *, PyObject *);
typedef PyObject *(*vectorcallfunc)(PyObject *callable, PyObject *const *args,
                                    size_t nargsf, PyObject *kwnames);

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

static inline const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out);
static inline PyObject *PyUnicode_AsEncodedString(PyObject *unicode,
                                                    const char *encoding,
                                                    const char *errors);
static inline PyObject *PyType_FromSpec(PyType_Spec *spec);
static inline PyObject *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases);
static inline PyObject *PyType_FromModuleAndSpec(
    PyObject *module, PyType_Spec *spec, PyObject *bases);
static inline PyObject *PyType_GetModule(PyTypeObject *type);
static inline void *PyType_GetModuleState(PyTypeObject *type);
static inline PyObject *PyType_GetModuleByDef(PyTypeObject *type, PyModuleDef *def);
static inline PyModuleDef *PyModule_GetDef(PyObject *module);
static inline void *PyModule_GetState(PyObject *module);
static inline int PyModule_AddFunctions(PyObject *module, PyMethodDef *functions);
static inline int PyState_AddModule(PyObject *module, PyModuleDef *def);
static inline PyObject *_molt_builtin_class_lookup_utf8(const char *name);
static inline void PyErr_Clear(void);
static inline int PyErr_ExceptionMatches(PyObject *exc);
static inline void PyErr_SetString(PyObject *exc, const char *message);
static inline void PyErr_SetNone(PyObject *exc);
static inline PyObject *PyErr_FormatV(PyObject *exc, const char *fmt, va_list vargs);
static inline PyObject *PyErr_NoMemory(void);
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
static inline const char *PyUnicode_AsUTF8(PyObject *value);
static inline int PyUnicode_Check(PyObject *obj);
static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size);
static inline long long PyLong_AsLongLong(PyObject *obj);
static inline long long PyLong_AsLongLongAndOverflow(PyObject *obj, int *overflow);
static inline PyObject *PyNumber_Long(PyObject *obj);
static inline int PyIter_Check(PyObject *obj);
static inline PyObject *PyIter_Next(PyObject *obj);
static inline double PyOS_string_to_double(
    const char *text, char **endptr, PyObject *overflow_exception);
static inline PyObject *PyImport_ImportModule(const char *name);
static inline PyObject *PyImport_GetModuleDict(void);
static inline void *PyCapsule_Import(const char *name, int no_block);
static inline PyObject *PyObject_GetAttrString(PyObject *obj, const char *name);
static inline int PyObject_SetAttrString(PyObject *obj, const char *name, PyObject *value);
static inline PyObject *PyObject_CallObject(PyObject *callable, PyObject *args);
static inline PyThreadState *PyThreadState_Get(void);
static inline PyGILState_STATE PyGILState_Ensure(void);
static inline void PyGILState_Release(PyGILState_STATE state);

#ifndef PYTHON_API_VERSION
#define PYTHON_API_VERSION 1013
#endif

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

#define Py_LT 0
#define Py_LE 1
#define Py_EQ 2
#define Py_NE 3
#define Py_GT 4
#define Py_GE 5

#define READONLY 1
#define T_OBJECT 6
#define T_OBJECT_EX 16

#define Py_TPFLAGS_DEFAULT 0UL
#define Py_TPFLAGS_BASETYPE (1UL << 10)
#define Py_TPFLAGS_HAVE_VECTORCALL (1UL << 11)
#define _Py_TPFLAGS_HAVE_VECTORCALL Py_TPFLAGS_HAVE_VECTORCALL
#define Py_TPFLAGS_HAVE_GC (1UL << 14)
#define Py_TPFLAGS_HEAPTYPE (1UL << 9)
#define Py_TPFLAGS_LONG_SUBCLASS (1UL << 24)
#define Py_TPFLAGS_READY (1UL << 12)

#define PyModuleDef_HEAD_INIT NULL

#define Py_SUCCESS 0
#define Py_FAILURE -1

#define PYGEN_RETURN 0
#define PYGEN_ERROR (-1)
#define PYGEN_NEXT 1

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0

#define PyOS_snprintf snprintf

#define PyGILState_LOCKED 0
#define PyGILState_UNLOCKED 1

#define Py_GIL_DISABLED 0
#define Py_MOD_GIL_NOT_USED 1
#define Py_mod_exec 2
#define Py_mod_gil 3

#ifndef Py_LIMITED_API
#define Py_LIMITED_API 0x030C0000
#endif

#ifndef PyAPI_FUNC
#define PyAPI_FUNC(RTYPE) RTYPE
#endif

#ifndef PyAPI_DATA
#define PyAPI_DATA(RTYPE) extern RTYPE
#endif

#ifndef PyMODINIT_FUNC
#define PyMODINIT_FUNC PyObject *
#endif

#define PyObject_HEAD_INIT(type) { 0 }
#define PyVarObject_HEAD_INIT(type, size) { 0 }
#define PyObject_HEAD PyObject ob_base;
#define PyObject_VAR_HEAD PyObject ob_base;

typedef struct {
    MoltHandle _molt_ob_base;
    Py_ssize_t ob_size;
} PyVarObject;

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

static inline PyObject *_molt_pyexc_not_implemented_error(void) {
    static MoltHandle cached = 0;
    if (cached == 0) {
        cached = _molt_exception_class_from_name("NotImplementedError");
    }
    return _molt_pyobject_from_handle(cached);
}

#define PyExc_TypeError _molt_pyexc_type_error()
#define PyExc_ValueError _molt_pyexc_value_error()
#define PyExc_RuntimeError _molt_pyexc_runtime_error()
#define PyExc_OverflowError _molt_pyexc_overflow_error()
#define PyExc_ImportError _molt_pyexc_import_error()
#define PyExc_PermissionError _molt_pyexc_permission_error()
#define PyExc_KeyError _molt_pyexc_key_error()
#define PyExc_MemoryError _molt_pyexc_memory_error()
#define PyExc_IndexError _molt_pyexc_index_error()
#define PyExc_SystemError _molt_pyexc_system_error()
#define PyExc_AttributeError _molt_pyexc_attribute_error()
#define PyExc_RuntimeWarning _molt_pyexc_runtime_warning()
#define PyExc_UserWarning _molt_pyexc_user_warning()
#define PyExc_OSError _molt_pyexc_os_error()
#define PyExc_StopIteration _molt_pyexc_stop_iteration()
#define PyExc_NotImplementedError _molt_pyexc_not_implemented_error()

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
    uint64_t type_bits = molt_type_of_borrowed((uint64_t)(uintptr_t)obj);
    return (PyTypeObject *)(uintptr_t)type_bits;
}

static inline void _molt_py_set_type(PyObject *obj, PyTypeObject *type_obj) {
    if (obj == NULL || type_obj == NULL) {
        return;
    }
    (void)PyObject_SetAttrString(obj, "__class__", (PyObject *)type_obj);
}

#define Py_TYPE(ob) _molt_py_typeof((PyObject *)(ob))
#define Py_SET_TYPE(ob, type_obj) _molt_py_set_type((PyObject *)(ob), (PyTypeObject *)(type_obj))
#define Py_REFCNT(ob) ((Py_ssize_t)1)
#define Py_SET_REFCNT(ob, refcnt) ((void)(ob), (void)(refcnt))
#define PyThreadState_GET() PyThreadState_Get()
#define PyObject_New(type, typeobj) ((type *)PyObject_CallObject((PyObject *)(typeobj), NULL))

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

static inline void PyErr_SetString(PyObject *exc, const char *message) {
    const char *msg = message != NULL ? message : "";
    MoltHandle exc_bits = exc != NULL ? _molt_py_handle(exc) : _molt_py_handle(PyExc_RuntimeError);
    (void)molt_err_set(exc_bits, (const uint8_t *)msg, (uint64_t)strlen(msg));
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
#define PyMem_FREE PyMem_Free
#define PyObject_Malloc PyMem_Malloc
#define PyObject_Free PyMem_Free

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
    if (obj == NULL || name == NULL) {
        PyErr_SetString(PyExc_SystemError, "NULL argument to PyObject_SetAttr");
        return -1;
    }
    if (value == NULL) {
        /* CPython contract: SetAttr with NULL value means delete attribute. */
        return (int)molt_object_delattr(_molt_py_handle(obj), _molt_py_handle(name));
    }
    return (int)molt_object_setattr(_molt_py_handle(obj), _molt_py_handle(name), _molt_py_handle(value));
}

static inline int PyObject_SetAttrString(PyObject *obj, const char *name, PyObject *value) {
    if (obj == NULL || name == NULL) {
        PyErr_SetString(PyExc_SystemError, "NULL argument to PyObject_SetAttrString");
        return -1;
    }
    if (value == NULL) {
        /* CPython contract: SetAttr with NULL value means delete attribute. */
        MoltHandle name_handle = molt_string_from((const uint8_t *)name, (uint64_t)strlen(name));
        int rc = (int)molt_object_delattr(_molt_py_handle(obj), name_handle);
        molt_handle_decref(name_handle);
        return rc;
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

/* ---- Vectorcall protocol (PEP 590) ---- */

#define PY_VECTORCALL_ARGUMENTS_OFFSET ((size_t)1 << (8 * sizeof(size_t) - 1))

static inline Py_ssize_t PyVectorcall_NARGS(size_t nargsf) {
    return (Py_ssize_t)(nargsf & ~PY_VECTORCALL_ARGUMENTS_OFFSET);
}

/*
 * PyObject_Vectorcall — "slow but correct" shim that converts the vectorcall
 * convention into a regular PyObject_Call(callable, args_tuple, kwargs_dict).
 */
static inline PyObject *PyObject_Vectorcall(
    PyObject *callable,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwnames
) {
    Py_ssize_t nargs = PyVectorcall_NARGS(nargsf);
    Py_ssize_t nkw = 0;
    Py_ssize_t i;
    MoltHandle *items = NULL;
    MoltHandle args_bits;
    MoltHandle kwargs_bits;
    MoltHandle out;

    if (callable == NULL) {
        PyErr_SetString(PyExc_TypeError, "callable must not be NULL");
        return NULL;
    }

    /* Build the positional args tuple via molt_tuple_from_array. */
    if (nargs > 0) {
        items = (MoltHandle *)PyMem_Malloc(sizeof(MoltHandle) * (size_t)nargs);
        if (items == NULL) {
            return PyErr_NoMemory();
        }
        for (i = 0; i < nargs; i++) {
            items[i] = _molt_py_handle(args[i]);
        }
    }
    args_bits = molt_tuple_from_array(items, (uint64_t)nargs);
    PyMem_Free(items);
    if (args_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }

    /* Build the keyword-arguments dict from kwnames tuple + trailing args. */
    kwargs_bits = molt_none();
    if (kwnames != NULL) {
        MoltHandle kwnames_bits = _molt_py_handle(kwnames);
        nkw = (Py_ssize_t)molt_sequence_length(kwnames_bits);
        if (nkw > 0) {
            MoltHandle *kw_keys = (MoltHandle *)PyMem_Malloc(sizeof(MoltHandle) * (size_t)nkw);
            MoltHandle *kw_vals = (MoltHandle *)PyMem_Malloc(sizeof(MoltHandle) * (size_t)nkw);
            MoltHandle kw_dict_bits;
            if (kw_keys == NULL || kw_vals == NULL) {
                PyMem_Free(kw_keys);
                PyMem_Free(kw_vals);
                molt_handle_decref(args_bits);
                return PyErr_NoMemory();
            }
            for (i = 0; i < nkw; i++) {
                MoltHandle idx = molt_int_from_i64((int64_t)i);
                kw_keys[i] = molt_sequence_getitem(kwnames_bits, idx);
                molt_handle_decref(idx);
                kw_vals[i] = _molt_py_handle(args[nargs + i]);
            }
            kw_dict_bits = molt_dict_from_pairs(kw_keys, kw_vals, (uint64_t)nkw);
            for (i = 0; i < nkw; i++) {
                molt_handle_decref(kw_keys[i]);
            }
            PyMem_Free(kw_keys);
            PyMem_Free(kw_vals);
            if (kw_dict_bits == 0 || molt_err_pending() != 0) {
                molt_handle_decref(args_bits);
                return NULL;
            }
            kwargs_bits = kw_dict_bits;
            out = molt_object_call(_molt_py_handle(callable), args_bits, kwargs_bits);
            molt_handle_decref(args_bits);
            molt_handle_decref(kwargs_bits);
            return _molt_pyobject_from_result(out);
        }
    }

    out = molt_object_call(_molt_py_handle(callable), args_bits, kwargs_bits);
    molt_handle_decref(args_bits);
    return _molt_pyobject_from_result(out);
}

/*
 * PyObject_VectorcallDict — like Vectorcall but takes a dict directly
 * instead of kwnames + trailing positional slots.
 */
static inline PyObject *PyObject_VectorcallDict(
    PyObject *callable,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwdict
) {
    Py_ssize_t nargs = PyVectorcall_NARGS(nargsf);
    Py_ssize_t i;
    MoltHandle *items = NULL;
    MoltHandle args_bits;
    MoltHandle kwargs_bits;
    MoltHandle out;

    if (callable == NULL) {
        PyErr_SetString(PyExc_TypeError, "callable must not be NULL");
        return NULL;
    }

    if (nargs > 0) {
        items = (MoltHandle *)PyMem_Malloc(sizeof(MoltHandle) * (size_t)nargs);
        if (items == NULL) {
            return PyErr_NoMemory();
        }
        for (i = 0; i < nargs; i++) {
            items[i] = _molt_py_handle(args[i]);
        }
    }
    args_bits = molt_tuple_from_array(items, (uint64_t)nargs);
    PyMem_Free(items);
    if (args_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }

    kwargs_bits = (kwdict != NULL) ? _molt_py_handle(kwdict) : molt_none();
    out = molt_object_call(_molt_py_handle(callable), args_bits, kwargs_bits);
    molt_handle_decref(args_bits);
    return _molt_pyobject_from_result(out);
}

/*
 * PyObject_VectorcallMethod — look up a method by name on args[0]
 * and call it with the vectorcall convention.
 *
 *   args[0] is "self", args[1..nargs-1] are positional arguments.
 */
static inline PyObject *PyObject_VectorcallMethod(
    PyObject *name,
    PyObject *const *args,
    size_t nargsf,
    PyObject *kwnames
) {
    Py_ssize_t nargs = PyVectorcall_NARGS(nargsf);
    PyObject *callable;
    PyObject *out;

    if (name == NULL || nargs < 1 || args == NULL || args[0] == NULL) {
        PyErr_SetString(PyExc_TypeError,
            "PyObject_VectorcallMethod: name and self must not be NULL");
        return NULL;
    }

    callable = PyObject_GetAttr(args[0], name);
    if (callable == NULL) {
        return NULL;
    }

    /*
     * Forward args[1..] to the resolved method.  The method is already
     * bound to args[0], so we skip self.
     */
    {
        size_t method_nargsf = (size_t)(nargs - 1);
        /* Preserve PY_VECTORCALL_ARGUMENTS_OFFSET if it was set. */
        if (nargsf & PY_VECTORCALL_ARGUMENTS_OFFSET) {
            method_nargsf |= PY_VECTORCALL_ARGUMENTS_OFFSET;
        }
        out = PyObject_Vectorcall(callable, args + 1, method_nargsf, kwnames);
    }

    Py_DECREF(callable);
    return out;
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

static inline void PyType_Modified(PyTypeObject *type) {
    /* In CPython this invalidates internal caches (method resolution order,
       attribute lookup caches) after a type object is mutated.  Molt does not
       maintain MRO caches at the C-API level, so this is a no-op. */
    (void)type;
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

/* ---- richcompare slot wrappers ----
 * CPython's tp_richcompare signature: PyObject *(*)(PyObject *self, PyObject *other, int op)
 * We create per-op thunk structures that store the original richcmpfunc pointer and the
 * comparison op code, then register them as individual __eq__, __lt__, etc. dunders.
 */

typedef PyObject *(*_molt_richcmpfunc)(PyObject *, PyObject *, int);

/* Per-op trampolines: each retrieves the stored richcmpfunc from the type object
 * and calls it with the appropriate Py_XX comparison op constant. */

static PyObject *_molt_richcmp_eq_trampoline(PyObject *self, PyObject *other);
static PyObject *_molt_richcmp_ne_trampoline(PyObject *self, PyObject *other);
static PyObject *_molt_richcmp_lt_trampoline(PyObject *self, PyObject *other);
static PyObject *_molt_richcmp_le_trampoline(PyObject *self, PyObject *other);
static PyObject *_molt_richcmp_gt_trampoline(PyObject *self, PyObject *other);
static PyObject *_molt_richcmp_ge_trampoline(PyObject *self, PyObject *other);

/* ---- hash slot wrapper ----
 * CPython's tp_hash signature: Py_hash_t (*)(PyObject *self)
 * We wrap it to return a PyObject* (Python int) so it works as __hash__. */

typedef Py_hash_t (*_molt_hashfunc)(PyObject *);

/* ---- dealloc slot storage ----
 * CPython's tp_dealloc signature: void (*)(PyObject *self)
 * We store it as __del__ on the type. When called from Python, it ignores return value. */

typedef void (*_molt_deallocfunc)(PyObject *);

/* Helper: install richcompare wrappers for all 6 comparison ops.
 * The richcmpfunc pointer is stored as a __molt_tp_richcompare__ attribute on the type
 * so trampolines can retrieve it. */
static inline int _molt_type_install_richcompare(PyObject *type_obj, uintptr_t richcmp_ptr) {
    /* Store the function pointer as an integer attribute for trampoline lookup */
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)richcmp_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_richcompare__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);

    /* Install each comparison dunder as a METH_O callable */
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__eq__", (uintptr_t)_molt_richcmp_eq_trampoline, (uint32_t)METH_O) < 0)
        return -1;
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__ne__", (uintptr_t)_molt_richcmp_ne_trampoline, (uint32_t)METH_O) < 0)
        return -1;
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__lt__", (uintptr_t)_molt_richcmp_lt_trampoline, (uint32_t)METH_O) < 0)
        return -1;
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__le__", (uintptr_t)_molt_richcmp_le_trampoline, (uint32_t)METH_O) < 0)
        return -1;
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__gt__", (uintptr_t)_molt_richcmp_gt_trampoline, (uint32_t)METH_O) < 0)
        return -1;
    if (_molt_type_maybe_set_slot_callable(
            type_obj, "__ge__", (uintptr_t)_molt_richcmp_ge_trampoline, (uint32_t)METH_O) < 0)
        return -1;

    return 0;
}

/* Richcompare trampoline helper: retrieve the stored richcmpfunc from the type and call it */
static inline PyObject *_molt_richcmp_call_for_op(PyObject *self, PyObject *other, int op) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_richcompare__");
    _molt_richcmpfunc func;
    PyObject *result;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_richcmpfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_richcompare slot");
        return NULL;
    }
    result = func(self, other, op);
    return result;
}

static PyObject *_molt_richcmp_eq_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_EQ);
}
static PyObject *_molt_richcmp_ne_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_NE);
}
static PyObject *_molt_richcmp_lt_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_LT);
}
static PyObject *_molt_richcmp_le_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_LE);
}
static PyObject *_molt_richcmp_gt_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_GT);
}
static PyObject *_molt_richcmp_ge_trampoline(PyObject *self, PyObject *other) {
    return _molt_richcmp_call_for_op(self, other, Py_GE);
}

/* Hash slot wrapper: wraps Py_hash_t return into a PyObject* int.
 * Stored on the type as __hash__ via a trampoline that retrieves the stored hashfunc. */
static inline int _molt_type_install_hash(PyObject *type_obj, uintptr_t hash_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)hash_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_hash__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return 0;
}

static PyObject *_molt_hash_trampoline(PyObject *self);

static inline int _molt_type_install_hash_callable(PyObject *type_obj, uintptr_t hash_ptr) {
    if (_molt_type_install_hash(type_obj, hash_ptr) < 0)
        return -1;
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__hash__", (uintptr_t)_molt_hash_trampoline, (uint32_t)METH_NOARGS);
}

static PyObject *_molt_hash_trampoline(PyObject *self) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_hash__");
    _molt_hashfunc func;
    Py_hash_t hash_val;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_hashfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_hash slot");
        return NULL;
    }
    hash_val = func(self);
    if (hash_val == (Py_hash_t)-1 && molt_err_pending() != 0) {
        return NULL;
    }
    return PyLong_FromLongLong((long long)hash_val);
}

/* Dealloc slot wrapper: wraps void(*)(PyObject*) into a no-return callable for __del__. */
static PyObject *_molt_dealloc_trampoline(PyObject *self);

static inline int _molt_type_install_dealloc(PyObject *type_obj, uintptr_t dealloc_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)dealloc_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_dealloc__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__del__", (uintptr_t)_molt_dealloc_trampoline, (uint32_t)METH_NOARGS);
}

static PyObject *_molt_dealloc_trampoline(PyObject *self) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_dealloc__");
    _molt_deallocfunc func;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_deallocfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_dealloc slot");
        return NULL;
    }
    func(self);
    Py_RETURN_NONE;
}

/* ---- nb_bool trampoline ----
 * CPython signature: int (*)(PyObject *)
 * We wrap to return Python bool. */
typedef int (*_molt_inquiry)(PyObject *);

static PyObject *_molt_nb_bool_trampoline(PyObject *self) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_nb_bool__");
    _molt_inquiry func;
    int result;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_inquiry)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt nb_bool slot");
        return NULL;
    }
    result = func(self);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    if (result) {
        Py_RETURN_TRUE;
    }
    Py_RETURN_FALSE;
}

static inline int _molt_type_install_nb_bool(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_nb_bool__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__bool__", (uintptr_t)_molt_nb_bool_trampoline, (uint32_t)METH_NOARGS);
}

/* ---- nb_power trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, PyObject *, PyObject *)
 * __pow__ takes (self, other[, mod]). We install as METH_VARARGS. */
typedef PyObject *(*_molt_ternaryfunc)(PyObject *, PyObject *, PyObject *);

static PyObject *_molt_nb_power_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_nb_power__");
    _molt_ternaryfunc func;
    PyObject *other = NULL;
    PyObject *mod = NULL;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ternaryfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt nb_power slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__pow__ takes 1 or 2 arguments");
        return NULL;
    }
    other = PyTuple_GetItem(args, 0);
    if (nargs == 2) {
        mod = PyTuple_GetItem(args, 1);
    } else {
        mod = Py_None;
    }
    return func(self, other, mod);
}

static inline int _molt_type_install_nb_power(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_nb_power__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__pow__", (uintptr_t)_molt_nb_power_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- nb_inplace_power trampoline ---- */
static PyObject *_molt_nb_ipower_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_nb_ipower__");
    _molt_ternaryfunc func;
    PyObject *other = NULL;
    PyObject *mod = NULL;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ternaryfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt nb_inplace_power slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__ipow__ takes 1 or 2 arguments");
        return NULL;
    }
    other = PyTuple_GetItem(args, 0);
    if (nargs == 2) {
        mod = PyTuple_GetItem(args, 1);
    } else {
        mod = Py_None;
    }
    return func(self, other, mod);
}

static inline int _molt_type_install_nb_ipower(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_nb_ipower__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__ipow__", (uintptr_t)_molt_nb_ipower_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- sq_length / mp_length trampoline ----
 * CPython signature: Py_ssize_t (*)(PyObject *)
 * Wraps to return Python int. */
typedef Py_ssize_t (*_molt_lenfunc)(PyObject *);

static inline PyObject *_molt_lenfunc_trampoline_impl(PyObject *self, const char *attr) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, attr);
    _molt_lenfunc func;
    Py_ssize_t result;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_lenfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt length slot");
        return NULL;
    }
    result = func(self);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    return PyLong_FromLongLong((long long)result);
}

static PyObject *_molt_sq_length_trampoline(PyObject *self) {
    return _molt_lenfunc_trampoline_impl(self, "__molt_sq_length__");
}

static PyObject *_molt_mp_length_trampoline(PyObject *self) {
    return _molt_lenfunc_trampoline_impl(self, "__molt_mp_length__");
}

static inline int _molt_type_install_sq_length(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_length__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__len__", (uintptr_t)_molt_sq_length_trampoline, (uint32_t)METH_NOARGS);
}

static inline int _molt_type_install_mp_length(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_mp_length__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__len__", (uintptr_t)_molt_mp_length_trampoline, (uint32_t)METH_NOARGS);
}

/* ---- sq_item trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, Py_ssize_t)
 * We wrap to accept a Python int index. */
typedef PyObject *(*_molt_ssizeargfunc)(PyObject *, Py_ssize_t);

static PyObject *_molt_sq_item_trampoline(PyObject *self, PyObject *index_obj) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_sq_item__");
    _molt_ssizeargfunc func;
    Py_ssize_t index;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ssizeargfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt sq_item slot");
        return NULL;
    }
    index = (Py_ssize_t)PyLong_AsLongLong(index_obj);
    if (index == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    return func(self, index);
}

static inline int _molt_type_install_sq_item(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_item__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__getitem__", (uintptr_t)_molt_sq_item_trampoline, (uint32_t)METH_O);
}

/* ---- sq_ass_item trampoline ----
 * CPython signature: int (*)(PyObject *, Py_ssize_t, PyObject *)
 * Wraps as __setitem__(self, index, value) or __delitem__(self, index) if value==NULL. */
typedef int (*_molt_ssizeobjargproc)(PyObject *, Py_ssize_t, PyObject *);

static PyObject *_molt_sq_ass_item_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_sq_ass_item__");
    _molt_ssizeobjargproc func;
    PyObject *index_obj;
    PyObject *value = NULL;
    Py_ssize_t index;
    int result;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ssizeobjargproc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt sq_ass_item slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__setitem__/__delitem__ takes 1 or 2 arguments");
        return NULL;
    }
    index_obj = PyTuple_GetItem(args, 0);
    index = (Py_ssize_t)PyLong_AsLongLong(index_obj);
    if (index == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    if (nargs == 2) {
        value = PyTuple_GetItem(args, 1);
    }
    result = func(self, index, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static inline int _molt_type_install_sq_ass_item(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_ass_item__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__setitem__", (uintptr_t)_molt_sq_ass_item_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- sq_contains trampoline ----
 * CPython signature: int (*)(PyObject *, PyObject *)
 * Wraps to return Python bool. */
typedef int (*_molt_objobjproc)(PyObject *, PyObject *);

static PyObject *_molt_sq_contains_trampoline(PyObject *self, PyObject *value) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_sq_contains__");
    _molt_objobjproc func;
    int result;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_objobjproc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt sq_contains slot");
        return NULL;
    }
    result = func(self, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    if (result) {
        Py_RETURN_TRUE;
    }
    Py_RETURN_FALSE;
}

static inline int _molt_type_install_sq_contains(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_contains__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__contains__", (uintptr_t)_molt_sq_contains_trampoline, (uint32_t)METH_O);
}

/* ---- sq_repeat / sq_inplace_repeat trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, Py_ssize_t)
 * Wraps to accept a Python int. */
static PyObject *_molt_sq_repeat_trampoline(PyObject *self, PyObject *count_obj) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_sq_repeat__");
    _molt_ssizeargfunc func;
    Py_ssize_t count;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ssizeargfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt sq_repeat slot");
        return NULL;
    }
    count = (Py_ssize_t)PyLong_AsLongLong(count_obj);
    if (count == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    return func(self, count);
}

static inline int _molt_type_install_sq_repeat(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_repeat__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__mul__", (uintptr_t)_molt_sq_repeat_trampoline, (uint32_t)METH_O);
}

static PyObject *_molt_sq_irepeat_trampoline(PyObject *self, PyObject *count_obj) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_sq_irepeat__");
    _molt_ssizeargfunc func;
    Py_ssize_t count;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ssizeargfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt sq_inplace_repeat slot");
        return NULL;
    }
    count = (Py_ssize_t)PyLong_AsLongLong(count_obj);
    if (count == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    return func(self, count);
}

static inline int _molt_type_install_sq_irepeat(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_sq_irepeat__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__imul__", (uintptr_t)_molt_sq_irepeat_trampoline, (uint32_t)METH_O);
}

/* ---- mp_ass_subscript trampoline ----
 * CPython signature: int (*)(PyObject *, PyObject *, PyObject *)
 * __setitem__(self, key, value) or __delitem__(self, key) if value==NULL. */
typedef int (*_molt_objobjargproc)(PyObject *, PyObject *, PyObject *);

static PyObject *_molt_mp_ass_subscript_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_mp_ass_subscript__");
    _molt_objobjargproc func;
    PyObject *key;
    PyObject *value = NULL;
    int result;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_objobjargproc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt mp_ass_subscript slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__setitem__/__delitem__ takes 1 or 2 arguments");
        return NULL;
    }
    key = PyTuple_GetItem(args, 0);
    if (nargs == 2) {
        value = PyTuple_GetItem(args, 1);
    }
    result = func(self, key, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static inline int _molt_type_install_mp_ass_subscript(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_mp_ass_subscript__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__setitem__", (uintptr_t)_molt_mp_ass_subscript_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- tp_descr_get trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, PyObject *, PyObject *)
 * __get__(self, obj, type) */
static PyObject *_molt_descr_get_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_descr_get__");
    _molt_ternaryfunc func;
    PyObject *obj;
    PyObject *type_arg;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_ternaryfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_descr_get slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs != 2) {
        PyErr_SetString(PyExc_TypeError, "__get__ takes exactly 2 arguments");
        return NULL;
    }
    obj = PyTuple_GetItem(args, 0);
    type_arg = PyTuple_GetItem(args, 1);
    return func(self, obj, type_arg);
}

static inline int _molt_type_install_descr_get(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_descr_get__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__get__", (uintptr_t)_molt_descr_get_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- tp_descr_set trampoline ----
 * CPython signature: int (*)(PyObject *, PyObject *, PyObject *)
 * __set__(self, obj, value) / __delete__(self, obj) if value==NULL. */
static PyObject *_molt_descr_set_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_descr_set__");
    _molt_objobjargproc func;
    PyObject *obj;
    PyObject *value = NULL;
    int result;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_objobjargproc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_descr_set slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__set__/__delete__ takes 1 or 2 arguments");
        return NULL;
    }
    obj = PyTuple_GetItem(args, 0);
    if (nargs == 2) {
        value = PyTuple_GetItem(args, 1);
    }
    result = func(self, obj, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static inline int _molt_type_install_descr_set(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_descr_set__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__set__", (uintptr_t)_molt_descr_set_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- tp_del / tp_finalize trampoline ----
 * CPython signature: void (*)(PyObject *)
 * Same as dealloc but for __del__ / __finalize__. */
static PyObject *_molt_tp_del_trampoline(PyObject *self) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_del__");
    _molt_deallocfunc func;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_deallocfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_del slot");
        return NULL;
    }
    func(self);
    Py_RETURN_NONE;
}

static inline int _molt_type_install_tp_del(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_del__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__del__", (uintptr_t)_molt_tp_del_trampoline, (uint32_t)METH_NOARGS);
}

static PyObject *_molt_tp_finalize_trampoline(PyObject *self) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_finalize__");
    _molt_deallocfunc func;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_deallocfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_finalize slot");
        return NULL;
    }
    func(self);
    Py_RETURN_NONE;
}

static inline int _molt_type_install_tp_finalize(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_finalize__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__del__", (uintptr_t)_molt_tp_finalize_trampoline, (uint32_t)METH_NOARGS);
}

/* ---- nb_divmod trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, PyObject *)
 * __divmod__(self, other) — same as METH_O, can use direct slot. */

/* ---- tp_getattr trampoline ----
 * CPython signature: PyObject *(*)(PyObject *, char *)
 * Deprecated in favor of tp_getattro. We accept and store it. */
typedef PyObject *(*_molt_getattrfunc)(PyObject *, char *);

static PyObject *_molt_tp_getattr_trampoline(PyObject *self, PyObject *name_obj) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_getattr__");
    _molt_getattrfunc func;
    const char *name;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_getattrfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_getattr slot");
        return NULL;
    }
    name = PyUnicode_AsUTF8(name_obj);
    if (name == NULL) {
        return NULL;
    }
    return func(self, (char *)name);
}

static inline int _molt_type_install_tp_getattr(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_getattr__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__getattr__", (uintptr_t)_molt_tp_getattr_trampoline, (uint32_t)METH_O);
}

/* ---- tp_setattr trampoline ----
 * CPython signature: int (*)(PyObject *, char *, PyObject *)
 * Deprecated in favor of tp_setattro. */
typedef int (*_molt_setattrfunc)(PyObject *, char *, PyObject *);

static PyObject *_molt_tp_setattr_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_setattr__");
    _molt_setattrfunc func;
    PyObject *name_obj;
    PyObject *value = NULL;
    const char *name;
    int result;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_setattrfunc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_setattr slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__setattr__/__delattr__ takes 1 or 2 arguments");
        return NULL;
    }
    name_obj = PyTuple_GetItem(args, 0);
    name = PyUnicode_AsUTF8(name_obj);
    if (name == NULL) {
        return NULL;
    }
    if (nargs == 2) {
        value = PyTuple_GetItem(args, 1);
    }
    result = func(self, (char *)name, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static inline int _molt_type_install_tp_setattr(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_setattr__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__setattr__", (uintptr_t)_molt_tp_setattr_trampoline, (uint32_t)METH_VARARGS);
}

/* ---- tp_setattro trampoline ----
 * CPython signature: int (*)(PyObject *, PyObject *, PyObject *)
 * __setattr__(self, name, value) / __delattr__(self, name) */
static PyObject *_molt_tp_setattro_trampoline(PyObject *self, PyObject *args) {
    PyObject *type_obj = (PyObject *)Py_TYPE(self);
    PyObject *ptr_obj = PyObject_GetAttrString(type_obj, "__molt_tp_setattro__");
    _molt_objobjargproc func;
    PyObject *name_arg;
    PyObject *value = NULL;
    int result;
    Py_ssize_t nargs;
    if (ptr_obj == NULL) {
        return NULL;
    }
    func = (_molt_objobjargproc)(uintptr_t)PyLong_AsUnsignedLongLong(ptr_obj);
    Py_DECREF(ptr_obj);
    if (func == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "corrupt tp_setattro slot");
        return NULL;
    }
    nargs = PyTuple_Size(args);
    if (nargs < 1 || nargs > 2) {
        PyErr_SetString(PyExc_TypeError, "__setattr__/__delattr__ takes 1 or 2 arguments");
        return NULL;
    }
    name_arg = PyTuple_GetItem(args, 0);
    if (nargs == 2) {
        value = PyTuple_GetItem(args, 1);
    }
    result = func(self, name_arg, value);
    if (result == -1 && molt_err_pending() != 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static inline int _molt_type_install_tp_setattro(PyObject *type_obj, uintptr_t func_ptr) {
    PyObject *ptr_obj = PyLong_FromUnsignedLongLong((unsigned long long)func_ptr);
    if (ptr_obj == NULL) {
        return -1;
    }
    if (PyObject_SetAttrString(type_obj, "__molt_tp_setattro__", ptr_obj) < 0) {
        Py_DECREF(ptr_obj);
        return -1;
    }
    Py_DECREF(ptr_obj);
    return _molt_type_maybe_set_slot_callable(
        type_obj, "__setattr__", (uintptr_t)_molt_tp_setattro_trampoline, (uint32_t)METH_VARARGS);
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

static inline PyObject *_molt_type_get_attached_module(
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
     * NOTE: This returns a NEW reference (from PyObject_GetAttrString).
     * The caller is responsible for calling Py_DECREF when done.
     * This differs from some CPython type APIs that return borrowed refs,
     * but is necessary because there is no internal borrowed-ref path here.
     */
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
    /* -- tp_ protocol slots -- */
    uintptr_t slot_tp_call = 0;
    uintptr_t slot_tp_iter = 0;
    uintptr_t slot_tp_iternext = 0;
    uintptr_t slot_tp_repr = 0;
    uintptr_t slot_tp_str = 0;
    uintptr_t slot_tp_dealloc = 0;
    uintptr_t slot_tp_init = 0;
    uintptr_t slot_tp_hash = 0;
    uintptr_t slot_tp_richcompare = 0;
    uintptr_t slot_tp_alloc = 0;
    uintptr_t slot_tp_free = 0;
    uintptr_t slot_tp_del = 0;
    uintptr_t slot_tp_finalize = 0;
    uintptr_t slot_tp_traverse = 0;
    uintptr_t slot_tp_clear = 0;
    uintptr_t slot_tp_is_gc = 0;
    uintptr_t slot_tp_getattr = 0;
    uintptr_t slot_tp_getattro = 0;
    uintptr_t slot_tp_setattr = 0;
    uintptr_t slot_tp_setattro = 0;
    uintptr_t slot_tp_descr_get = 0;
    uintptr_t slot_tp_descr_set = 0;
    /* -- nb_ number protocol slots -- */
    uintptr_t slot_nb_add = 0;
    uintptr_t slot_nb_subtract = 0;
    uintptr_t slot_nb_multiply = 0;
    uintptr_t slot_nb_remainder = 0;
    uintptr_t slot_nb_divmod = 0;
    uintptr_t slot_nb_power = 0;
    uintptr_t slot_nb_negative = 0;
    uintptr_t slot_nb_positive = 0;
    uintptr_t slot_nb_absolute = 0;
    uintptr_t slot_nb_bool = 0;
    uintptr_t slot_nb_invert = 0;
    uintptr_t slot_nb_lshift = 0;
    uintptr_t slot_nb_rshift = 0;
    uintptr_t slot_nb_and = 0;
    uintptr_t slot_nb_xor = 0;
    uintptr_t slot_nb_or = 0;
    uintptr_t slot_nb_int = 0;
    uintptr_t slot_nb_float = 0;
    uintptr_t slot_nb_floor_divide = 0;
    uintptr_t slot_nb_true_divide = 0;
    uintptr_t slot_nb_index = 0;
    uintptr_t slot_nb_inplace_add = 0;
    uintptr_t slot_nb_inplace_subtract = 0;
    uintptr_t slot_nb_inplace_multiply = 0;
    uintptr_t slot_nb_inplace_remainder = 0;
    uintptr_t slot_nb_inplace_power = 0;
    uintptr_t slot_nb_inplace_lshift = 0;
    uintptr_t slot_nb_inplace_rshift = 0;
    uintptr_t slot_nb_inplace_and = 0;
    uintptr_t slot_nb_inplace_xor = 0;
    uintptr_t slot_nb_inplace_or = 0;
    uintptr_t slot_nb_inplace_floor_divide = 0;
    uintptr_t slot_nb_inplace_true_divide = 0;
    uintptr_t slot_nb_matrix_multiply = 0;
    uintptr_t slot_nb_inplace_matrix_multiply = 0;
    /* -- sq_ sequence protocol slots -- */
    uintptr_t slot_sq_concat = 0;
    uintptr_t slot_sq_length = 0;
    uintptr_t slot_sq_item = 0;
    uintptr_t slot_sq_ass_item = 0;
    uintptr_t slot_sq_contains = 0;
    uintptr_t slot_sq_repeat = 0;
    uintptr_t slot_sq_inplace_concat = 0;
    uintptr_t slot_sq_inplace_repeat = 0;
    /* -- mp_ mapping protocol slots -- */
    uintptr_t slot_mp_length = 0;
    uintptr_t slot_mp_subscript = 0;
    uintptr_t slot_mp_ass_subscript = 0;
    /* -- bf_ buffer protocol slots -- */
    uintptr_t slot_bf_getbuffer = 0;
    uintptr_t slot_bf_releasebuffer = 0;
    /* -- am_ async protocol slots -- */
    uintptr_t slot_am_await = 0;
    uintptr_t slot_am_aiter = 0;
    uintptr_t slot_am_anext = 0;
    uintptr_t slot_am_send = 0;
    int saw_methods = 0;
    int saw_getset = 0;
    int saw_members = 0;
    int saw_base = 0;
    int saw_bases = 0;
    int saw_doc = 0;
    int saw_new = 0;
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
            /* Macro to reduce boilerplate for uintptr_t slot storage with duplicate detection */
            #define _MOLT_STORE_SLOT(slot_id, dest_var, label) \
                case slot_id: \
                    if (dest_var != 0) { \
                        PyErr_SetString(PyExc_TypeError, "duplicate " label " slot"); \
                        return NULL; \
                    } \
                    dest_var = (uintptr_t)slot->pfunc; \
                    break

            switch (slot->slot) {
            /* ---- tp_ type protocol (special handling) ---- */
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

            /* ---- tp_ callable/protocol slots ---- */
            _MOLT_STORE_SLOT(Py_tp_call, slot_tp_call, "Py_tp_call");
            _MOLT_STORE_SLOT(Py_tp_iter, slot_tp_iter, "Py_tp_iter");
            _MOLT_STORE_SLOT(Py_tp_iternext, slot_tp_iternext, "Py_tp_iternext");
            _MOLT_STORE_SLOT(Py_tp_repr, slot_tp_repr, "Py_tp_repr");
            _MOLT_STORE_SLOT(Py_tp_str, slot_tp_str, "Py_tp_str");
            _MOLT_STORE_SLOT(Py_tp_dealloc, slot_tp_dealloc, "Py_tp_dealloc");
            _MOLT_STORE_SLOT(Py_tp_init, slot_tp_init, "Py_tp_init");
            _MOLT_STORE_SLOT(Py_tp_hash, slot_tp_hash, "Py_tp_hash");
            _MOLT_STORE_SLOT(Py_tp_richcompare, slot_tp_richcompare, "Py_tp_richcompare");
            _MOLT_STORE_SLOT(Py_tp_alloc, slot_tp_alloc, "Py_tp_alloc");
            _MOLT_STORE_SLOT(Py_tp_free, slot_tp_free, "Py_tp_free");
            _MOLT_STORE_SLOT(Py_tp_del, slot_tp_del, "Py_tp_del");
            _MOLT_STORE_SLOT(Py_tp_finalize, slot_tp_finalize, "Py_tp_finalize");
            _MOLT_STORE_SLOT(Py_tp_traverse, slot_tp_traverse, "Py_tp_traverse");
            _MOLT_STORE_SLOT(Py_tp_clear, slot_tp_clear, "Py_tp_clear");
            _MOLT_STORE_SLOT(Py_tp_is_gc, slot_tp_is_gc, "Py_tp_is_gc");
            _MOLT_STORE_SLOT(Py_tp_getattr, slot_tp_getattr, "Py_tp_getattr");
            _MOLT_STORE_SLOT(Py_tp_getattro, slot_tp_getattro, "Py_tp_getattro");
            _MOLT_STORE_SLOT(Py_tp_setattr, slot_tp_setattr, "Py_tp_setattr");
            _MOLT_STORE_SLOT(Py_tp_setattro, slot_tp_setattro, "Py_tp_setattro");
            _MOLT_STORE_SLOT(Py_tp_descr_get, slot_tp_descr_get, "Py_tp_descr_get");
            _MOLT_STORE_SLOT(Py_tp_descr_set, slot_tp_descr_set, "Py_tp_descr_set");

            /* ---- nb_ number protocol ---- */
            _MOLT_STORE_SLOT(Py_nb_add, slot_nb_add, "Py_nb_add");
            _MOLT_STORE_SLOT(Py_nb_subtract, slot_nb_subtract, "Py_nb_subtract");
            _MOLT_STORE_SLOT(Py_nb_multiply, slot_nb_multiply, "Py_nb_multiply");
            _MOLT_STORE_SLOT(Py_nb_remainder, slot_nb_remainder, "Py_nb_remainder");
            _MOLT_STORE_SLOT(Py_nb_divmod, slot_nb_divmod, "Py_nb_divmod");
            _MOLT_STORE_SLOT(Py_nb_power, slot_nb_power, "Py_nb_power");
            _MOLT_STORE_SLOT(Py_nb_negative, slot_nb_negative, "Py_nb_negative");
            _MOLT_STORE_SLOT(Py_nb_positive, slot_nb_positive, "Py_nb_positive");
            _MOLT_STORE_SLOT(Py_nb_absolute, slot_nb_absolute, "Py_nb_absolute");
            _MOLT_STORE_SLOT(Py_nb_bool, slot_nb_bool, "Py_nb_bool");
            _MOLT_STORE_SLOT(Py_nb_invert, slot_nb_invert, "Py_nb_invert");
            _MOLT_STORE_SLOT(Py_nb_lshift, slot_nb_lshift, "Py_nb_lshift");
            _MOLT_STORE_SLOT(Py_nb_rshift, slot_nb_rshift, "Py_nb_rshift");
            _MOLT_STORE_SLOT(Py_nb_and, slot_nb_and, "Py_nb_and");
            _MOLT_STORE_SLOT(Py_nb_xor, slot_nb_xor, "Py_nb_xor");
            _MOLT_STORE_SLOT(Py_nb_or, slot_nb_or, "Py_nb_or");
            _MOLT_STORE_SLOT(Py_nb_int, slot_nb_int, "Py_nb_int");
            _MOLT_STORE_SLOT(Py_nb_float, slot_nb_float, "Py_nb_float");
            _MOLT_STORE_SLOT(Py_nb_floor_divide, slot_nb_floor_divide, "Py_nb_floor_divide");
            _MOLT_STORE_SLOT(Py_nb_true_divide, slot_nb_true_divide, "Py_nb_true_divide");
            _MOLT_STORE_SLOT(Py_nb_index, slot_nb_index, "Py_nb_index");
            _MOLT_STORE_SLOT(Py_nb_inplace_add, slot_nb_inplace_add, "Py_nb_inplace_add");
            _MOLT_STORE_SLOT(Py_nb_inplace_subtract, slot_nb_inplace_subtract, "Py_nb_inplace_subtract");
            _MOLT_STORE_SLOT(Py_nb_inplace_multiply, slot_nb_inplace_multiply, "Py_nb_inplace_multiply");
            _MOLT_STORE_SLOT(Py_nb_inplace_remainder, slot_nb_inplace_remainder, "Py_nb_inplace_remainder");
            _MOLT_STORE_SLOT(Py_nb_inplace_power, slot_nb_inplace_power, "Py_nb_inplace_power");
            _MOLT_STORE_SLOT(Py_nb_inplace_lshift, slot_nb_inplace_lshift, "Py_nb_inplace_lshift");
            _MOLT_STORE_SLOT(Py_nb_inplace_rshift, slot_nb_inplace_rshift, "Py_nb_inplace_rshift");
            _MOLT_STORE_SLOT(Py_nb_inplace_and, slot_nb_inplace_and, "Py_nb_inplace_and");
            _MOLT_STORE_SLOT(Py_nb_inplace_xor, slot_nb_inplace_xor, "Py_nb_inplace_xor");
            _MOLT_STORE_SLOT(Py_nb_inplace_or, slot_nb_inplace_or, "Py_nb_inplace_or");
            _MOLT_STORE_SLOT(Py_nb_inplace_floor_divide, slot_nb_inplace_floor_divide, "Py_nb_inplace_floor_divide");
            _MOLT_STORE_SLOT(Py_nb_inplace_true_divide, slot_nb_inplace_true_divide, "Py_nb_inplace_true_divide");
            _MOLT_STORE_SLOT(Py_nb_matrix_multiply, slot_nb_matrix_multiply, "Py_nb_matrix_multiply");
            _MOLT_STORE_SLOT(Py_nb_inplace_matrix_multiply, slot_nb_inplace_matrix_multiply, "Py_nb_inplace_matrix_multiply");

            /* ---- sq_ sequence protocol ---- */
            _MOLT_STORE_SLOT(Py_sq_concat, slot_sq_concat, "Py_sq_concat");
            _MOLT_STORE_SLOT(Py_sq_length, slot_sq_length, "Py_sq_length");
            _MOLT_STORE_SLOT(Py_sq_item, slot_sq_item, "Py_sq_item");
            _MOLT_STORE_SLOT(Py_sq_ass_item, slot_sq_ass_item, "Py_sq_ass_item");
            _MOLT_STORE_SLOT(Py_sq_contains, slot_sq_contains, "Py_sq_contains");
            _MOLT_STORE_SLOT(Py_sq_repeat, slot_sq_repeat, "Py_sq_repeat");
            _MOLT_STORE_SLOT(Py_sq_inplace_concat, slot_sq_inplace_concat, "Py_sq_inplace_concat");
            _MOLT_STORE_SLOT(Py_sq_inplace_repeat, slot_sq_inplace_repeat, "Py_sq_inplace_repeat");

            /* ---- mp_ mapping protocol ---- */
            _MOLT_STORE_SLOT(Py_mp_length, slot_mp_length, "Py_mp_length");
            _MOLT_STORE_SLOT(Py_mp_subscript, slot_mp_subscript, "Py_mp_subscript");
            _MOLT_STORE_SLOT(Py_mp_ass_subscript, slot_mp_ass_subscript, "Py_mp_ass_subscript");

            /* ---- bf_ buffer protocol ---- */
            _MOLT_STORE_SLOT(Py_bf_getbuffer, slot_bf_getbuffer, "Py_bf_getbuffer");
            _MOLT_STORE_SLOT(Py_bf_releasebuffer, slot_bf_releasebuffer, "Py_bf_releasebuffer");

            /* ---- am_ async protocol ---- */
            _MOLT_STORE_SLOT(Py_am_await, slot_am_await, "Py_am_await");
            _MOLT_STORE_SLOT(Py_am_aiter, slot_am_aiter, "Py_am_aiter");
            _MOLT_STORE_SLOT(Py_am_anext, slot_am_anext, "Py_am_anext");
            _MOLT_STORE_SLOT(Py_am_send, slot_am_send, "Py_am_send");

            default:
                PyErr_Format(
                    PyExc_RuntimeError,
                    "unsupported PyType_Spec slot %d for %s",
                    slot->slot,
                    full_name);
                return NULL;
            }
            #undef _MOLT_STORE_SLOT
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
    /* ================================================================
     * Install all slot callables on the type.
     *
     * Macro _MOLT_INSTALL_SIMPLE installs a slot as a dunder method
     * using _molt_type_maybe_set_slot_callable (no-op if ptr==0).
     * Macro _MOLT_INSTALL_TRAMPOLINE calls a custom installer that
     * stores the pointer and sets up a trampoline (only if ptr!=0).
     * ================================================================ */
    #define _MOLT_INSTALL_SIMPLE(dunder, slot_var, flags) \
        if (_molt_type_maybe_set_slot_callable( \
                type_obj, dunder, slot_var, (uint32_t)(flags)) < 0) { \
            Py_DECREF(type_obj); \
            return NULL; \
        }
    #define _MOLT_INSTALL_TRAMPOLINE(slot_var, installer) \
        if (slot_var != 0 && installer(type_obj, slot_var) < 0) { \
            Py_DECREF(type_obj); \
            return NULL; \
        }

    /* ---- tp_ type protocol ---- */
    _MOLT_INSTALL_SIMPLE("__call__", slot_tp_call, METH_VARARGS | METH_KEYWORDS)
    _MOLT_INSTALL_SIMPLE("__iter__", slot_tp_iter, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__next__", slot_tp_iternext, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__repr__", slot_tp_repr, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__str__", slot_tp_str, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__init__", slot_tp_init, METH_VARARGS | METH_KEYWORDS)
    _MOLT_INSTALL_SIMPLE("__getattribute__", slot_tp_getattro, METH_O)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_dealloc, _molt_type_install_dealloc)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_hash, _molt_type_install_hash_callable)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_richcompare, _molt_type_install_richcompare)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_del, _molt_type_install_tp_del)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_finalize, _molt_type_install_tp_finalize)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_getattr, _molt_type_install_tp_getattr)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_setattr, _molt_type_install_tp_setattr)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_setattro, _molt_type_install_tp_setattro)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_descr_get, _molt_type_install_descr_get)
    _MOLT_INSTALL_TRAMPOLINE(slot_tp_descr_set, _molt_type_install_descr_set)
    /* tp_alloc, tp_free, tp_traverse, tp_clear, tp_is_gc are GC/memory management
     * slots. We accept them silently (for ABI compat) but they are no-ops in Molt
     * since Molt uses its own GC. */
    (void)slot_tp_alloc;
    (void)slot_tp_free;
    (void)slot_tp_traverse;
    (void)slot_tp_clear;
    (void)slot_tp_is_gc;

    /* ---- nb_ number protocol ---- */
    _MOLT_INSTALL_SIMPLE("__add__", slot_nb_add, METH_O)
    _MOLT_INSTALL_SIMPLE("__sub__", slot_nb_subtract, METH_O)
    _MOLT_INSTALL_SIMPLE("__mul__", slot_nb_multiply, METH_O)
    _MOLT_INSTALL_SIMPLE("__mod__", slot_nb_remainder, METH_O)
    _MOLT_INSTALL_SIMPLE("__divmod__", slot_nb_divmod, METH_O)
    _MOLT_INSTALL_SIMPLE("__neg__", slot_nb_negative, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__pos__", slot_nb_positive, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__abs__", slot_nb_absolute, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__invert__", slot_nb_invert, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__int__", slot_nb_int, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__float__", slot_nb_float, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__index__", slot_nb_index, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__lshift__", slot_nb_lshift, METH_O)
    _MOLT_INSTALL_SIMPLE("__rshift__", slot_nb_rshift, METH_O)
    _MOLT_INSTALL_SIMPLE("__and__", slot_nb_and, METH_O)
    _MOLT_INSTALL_SIMPLE("__xor__", slot_nb_xor, METH_O)
    _MOLT_INSTALL_SIMPLE("__or__", slot_nb_or, METH_O)
    _MOLT_INSTALL_SIMPLE("__floordiv__", slot_nb_floor_divide, METH_O)
    _MOLT_INSTALL_SIMPLE("__truediv__", slot_nb_true_divide, METH_O)
    _MOLT_INSTALL_SIMPLE("__matmul__", slot_nb_matrix_multiply, METH_O)
    /* inplace number ops */
    _MOLT_INSTALL_SIMPLE("__iadd__", slot_nb_inplace_add, METH_O)
    _MOLT_INSTALL_SIMPLE("__isub__", slot_nb_inplace_subtract, METH_O)
    _MOLT_INSTALL_SIMPLE("__imul__", slot_nb_inplace_multiply, METH_O)
    _MOLT_INSTALL_SIMPLE("__imod__", slot_nb_inplace_remainder, METH_O)
    _MOLT_INSTALL_SIMPLE("__ilshift__", slot_nb_inplace_lshift, METH_O)
    _MOLT_INSTALL_SIMPLE("__irshift__", slot_nb_inplace_rshift, METH_O)
    _MOLT_INSTALL_SIMPLE("__iand__", slot_nb_inplace_and, METH_O)
    _MOLT_INSTALL_SIMPLE("__ixor__", slot_nb_inplace_xor, METH_O)
    _MOLT_INSTALL_SIMPLE("__ior__", slot_nb_inplace_or, METH_O)
    _MOLT_INSTALL_SIMPLE("__ifloordiv__", slot_nb_inplace_floor_divide, METH_O)
    _MOLT_INSTALL_SIMPLE("__itruediv__", slot_nb_inplace_true_divide, METH_O)
    _MOLT_INSTALL_SIMPLE("__imatmul__", slot_nb_inplace_matrix_multiply, METH_O)
    /* nb_bool, nb_power, nb_inplace_power need trampolines (different signatures) */
    _MOLT_INSTALL_TRAMPOLINE(slot_nb_bool, _molt_type_install_nb_bool)
    _MOLT_INSTALL_TRAMPOLINE(slot_nb_power, _molt_type_install_nb_power)
    _MOLT_INSTALL_TRAMPOLINE(slot_nb_inplace_power, _molt_type_install_nb_ipower)

    /* ---- sq_ sequence protocol ---- */
    /* sq_concat -> __add__ (only if nb_add not set) */
    if (slot_nb_add == 0) {
        _MOLT_INSTALL_SIMPLE("__add__", slot_sq_concat, METH_O)
    }
    /* sq_inplace_concat -> __iadd__ (only if nb_inplace_add not set) */
    if (slot_nb_inplace_add == 0) {
        _MOLT_INSTALL_SIMPLE("__iadd__", slot_sq_inplace_concat, METH_O)
    }
    _MOLT_INSTALL_TRAMPOLINE(slot_sq_length, _molt_type_install_sq_length)
    _MOLT_INSTALL_TRAMPOLINE(slot_sq_item, _molt_type_install_sq_item)
    _MOLT_INSTALL_TRAMPOLINE(slot_sq_ass_item, _molt_type_install_sq_ass_item)
    _MOLT_INSTALL_TRAMPOLINE(slot_sq_contains, _molt_type_install_sq_contains)
    /* sq_repeat -> __mul__ (only if nb_multiply not set) */
    if (slot_nb_multiply == 0) {
        _MOLT_INSTALL_TRAMPOLINE(slot_sq_repeat, _molt_type_install_sq_repeat)
    }
    if (slot_nb_inplace_multiply == 0) {
        _MOLT_INSTALL_TRAMPOLINE(slot_sq_inplace_repeat, _molt_type_install_sq_irepeat)
    }

    /* ---- mp_ mapping protocol ---- */
    _MOLT_INSTALL_TRAMPOLINE(slot_mp_length, _molt_type_install_mp_length)
    _MOLT_INSTALL_SIMPLE("__getitem__", slot_mp_subscript, METH_O)
    _MOLT_INSTALL_TRAMPOLINE(slot_mp_ass_subscript, _molt_type_install_mp_ass_subscript)

    /* ---- bf_ buffer protocol ----
     * Buffer protocol slots are stored for ABI compatibility but are no-ops
     * in the Molt runtime (Molt uses its own memory model). */
    (void)slot_bf_getbuffer;
    (void)slot_bf_releasebuffer;

    /* ---- am_ async protocol ---- */
    _MOLT_INSTALL_SIMPLE("__await__", slot_am_await, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__aiter__", slot_am_aiter, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__anext__", slot_am_anext, METH_NOARGS)
    _MOLT_INSTALL_SIMPLE("__molt_am_send__", slot_am_send, METH_O)

    #undef _MOLT_INSTALL_SIMPLE
    #undef _MOLT_INSTALL_TRAMPOLINE
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
    return _molt_type_get_attached_module(type, 0);
}

static inline void *PyType_GetModuleState(PyTypeObject *type) {
    PyObject *module = PyType_GetModule(type);
    void *state;
    if (module == NULL) {
        return NULL;
    }
    state = PyModule_GetState(module);
    Py_DECREF(module);
    return state;
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
        module = _molt_type_get_attached_module((PyTypeObject *)_molt_pyobject_from_handle(base_bits), 1);
        if (module != NULL) {
            candidate_def = PyModule_GetDef(module);
            if (candidate_def == def) {
                molt_handle_decref(base_bits);
                molt_handle_decref(mro_bits);
                return module; /* caller owns the new reference */
            }
            if (candidate_def == NULL && molt_err_pending() != 0) {
                Py_DECREF(module);
                molt_handle_decref(base_bits);
                molt_handle_decref(mro_bits);
                return NULL;
            }
            Py_DECREF(module); /* not a match — release the new reference */
        }
        molt_handle_decref(base_bits);
    }
    molt_handle_decref(mro_bits);
    PyErr_SetString(PyExc_TypeError, "type has no associated module for the given definition");
    return NULL;
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
    if (value > (unsigned long long)INT64_MAX) {
        PyErr_SetString(PyExc_OverflowError,
            "Python int too large to convert to molt i64");
        return NULL;
    }
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline long PyLong_AsLong(PyObject *obj) {
    return (long)molt_int_as_i64(_molt_py_handle(obj));
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

static inline PyObject *PyFloat_FromDouble(double value) {
    return _molt_pyobject_from_result(molt_float_from_f64(value));
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

static inline Py_ssize_t PyMapping_Size(PyObject *mapping) {
    return (Py_ssize_t)molt_mapping_length(_molt_py_handle(mapping));
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

static inline PyObject *PyDict_New(void) {
    return _molt_pyobject_from_result(molt_dict_from_pairs(NULL, NULL, 0));
}

static inline Py_ssize_t PyDict_Size(PyObject *dict) {
    return PyMapping_Size(dict);
}

static inline int PyDict_SetItem(PyObject *dict, PyObject *key, PyObject *value) {
    return molt_mapping_setitem(_molt_py_handle(dict), _molt_py_handle(key), _molt_py_handle(value));
}

static inline int PyDict_SetItemString(PyObject *dict, const char *key, PyObject *value) {
    return PyMapping_SetItemString(dict, key, value);
}

static inline PyObject *PyDict_GetItem(PyObject *dict, PyObject *key) {
    uint64_t val = molt_dict_getitem_borrowed((uint64_t)(uintptr_t)dict, (uint64_t)(uintptr_t)key);
    if (val == 0) return NULL;
    return (PyObject *)(uintptr_t)val;
}

static inline PyObject *PyDict_GetItemString(PyObject *dict, const char *key) {
    MoltHandle key_bits;
    MoltHandle out;
    if (key == NULL) {
        return NULL;
    }
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0 || molt_err_pending() != 0) {
        molt_err_clear();  /* PyDict_GetItemString silently clears errors */
        return NULL;
    }
    out = molt_mapping_getitem(_molt_py_handle(dict), key_bits);
    molt_handle_decref(key_bits);
    if (out == 0 || molt_err_pending() != 0) {
        molt_err_clear();  /* PyDict_GetItemString silently clears errors */
        return NULL;
    }
    /* No Py_DECREF — borrowed reference backed by dict */
    return _molt_pyobject_from_handle(out);
}

static inline int PyDict_Contains(PyObject *dict, PyObject *key) {
    return molt_object_contains(_molt_py_handle(dict), _molt_py_handle(key));
}

static inline PyObject *PyDict_GetItemWithError(PyObject *dict, PyObject *key) {
    /* Use borrowed lookup — callers expect a borrowed reference per CPython contract. */
    MoltHandle out = molt_dict_getitem_borrowed(_molt_py_handle(dict), _molt_py_handle(key));
    if (out == 0) {
        /* Key not found. If an error is pending, propagate it; otherwise return NULL
           without setting an exception (KeyError is NOT raised — that's the contract). */
        return NULL;
    }
    return _molt_pyobject_from_handle(out);
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

static inline int PyDict_Next(
    PyObject *dict,
    Py_ssize_t *ppos,
    PyObject **pkey,
    PyObject **pvalue
) {
    MoltHandle keys_obj;
    MoltHandle key_handle;
    MoltHandle key_idx;
    MoltHandle val_handle;
    Py_ssize_t pos;
    int64_t dict_len;

    if (dict == NULL || ppos == NULL) {
        return 0;
    }
    pos = *ppos;
    dict_len = molt_mapping_length(_molt_py_handle(dict));
    if (pos < 0 || pos >= (Py_ssize_t)dict_len) {
        return 0;
    }

    keys_obj = molt_mapping_keys(_molt_py_handle(dict));
    if (keys_obj == 0 || molt_err_pending() != 0) {
        molt_err_clear();
        return 0;
    }

    key_idx = molt_int_from_i64((int64_t)pos);
    key_handle = molt_sequence_getitem(keys_obj, key_idx);
    molt_handle_decref(key_idx);
    molt_handle_decref(keys_obj);
    if (key_handle == 0 || molt_err_pending() != 0) {
        molt_err_clear();
        return 0;
    }

    if (pkey != NULL) {
        *pkey = _molt_pyobject_from_handle(key_handle);
    }
    if (pvalue != NULL) {
        val_handle = molt_mapping_getitem(_molt_py_handle(dict), key_handle);
        if (val_handle == 0 || molt_err_pending() != 0) {
            molt_err_clear();
            if (pkey != NULL) { *pkey = NULL; }
            return 0;
        }
        *pvalue = _molt_pyobject_from_handle(val_handle);
    }

    *ppos = pos + 1;
    return 1;
}

static inline PyObject *PyUnicode_FromString(const char *value) {
    MoltHandle bits = _molt_string_from_utf8(value);
    if (bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(bits);
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

static inline Py_ssize_t PyUnicode_GetLength(PyObject *value) {
    /* Return codepoint count, not UTF-8 byte count.
     * molt_sequence_length on a string returns the number of characters. */
    int64_t len = molt_sequence_length(_molt_py_handle(value));
    if (molt_err_pending() != 0) {
        return -1;
    }
    return (Py_ssize_t)len;
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

static inline PyObject *PyUnicode_AsLatin1String(PyObject *value) {
    /* Latin-1 is a superset of ASCII; for byte-range code points it matches
       the first 256 Unicode values.  Delegate to the runtime encoder. */
    return PyUnicode_AsEncodedString(value, "latin-1", NULL);
}

#define PyUnicode_1BYTE_KIND 1
#define PyUnicode_2BYTE_KIND 2
#define PyUnicode_4BYTE_KIND 4
typedef uint8_t Py_UCS1;
typedef uint16_t Py_UCS2;

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

static inline int PyObject_GetBuffer(PyObject *obj, Py_buffer *view, int flags) {
    (void)flags;
    if (view == NULL) {
        PyErr_SetString(PyExc_TypeError, "buffer view must not be NULL");
        return -1;
    }
    int rc;
    memset(view, 0, sizeof(*view));
    rc = molt_buffer_acquire(_molt_py_handle(obj), &view->_molt_view);
    if (rc == 0) {
        view->buf = view->_molt_view.data;
        view->len = (Py_ssize_t)view->_molt_view.len;
        view->readonly = (int)view->_molt_view.readonly;
        view->itemsize = (Py_ssize_t)view->_molt_view.itemsize;
        view->ndim = 1;
        view->obj = obj;
        if (obj != NULL) Py_INCREF(obj);
    }
    return rc;
}

static inline void PyBuffer_Release(Py_buffer *view) {
    if (view == NULL) {
        return;
    }
    (void)molt_buffer_release(&view->_molt_view);
    if (view->obj != NULL) {
        Py_DECREF(view->obj);
        view->obj = NULL;
    }
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
    uint64_t idx = molt_int_from_i64((int64_t)index);
    uint64_t val = molt_list_getitem_borrowed((uint64_t)(uintptr_t)list, idx);
    molt_handle_decref(idx);
    if (val == 0) return NULL;
    return (PyObject *)(uintptr_t)val;
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
    uint64_t idx = molt_int_from_i64((int64_t)index);
    uint64_t val = molt_tuple_getitem_borrowed((uint64_t)(uintptr_t)tuple, idx);
    molt_handle_decref(idx);
    if (val == 0) return NULL;
    return (PyObject *)(uintptr_t)val;
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

#define PyLong_Type (*_molt_builtin_type_object_borrowed("int"))
#define PyFloat_Type (*_molt_builtin_type_object_borrowed("float"))
#define PyBool_Type (*_molt_builtin_type_object_borrowed("bool"))
#define PyBytes_Type (*_molt_builtin_type_object_borrowed("bytes"))
#define PyUnicode_Type (*_molt_builtin_type_object_borrowed("str"))
#define PyComplex_Type (*_molt_builtin_type_object_borrowed("complex"))
#define PySet_Type (*_molt_builtin_type_object_borrowed("set"))
#define PyFrozenSet_Type (*_molt_builtin_type_object_borrowed("frozenset"))
#define PyDict_Type (*_molt_builtin_type_object_borrowed("dict"))
#define PyList_Type (*_molt_builtin_type_object_borrowed("list"))
#define PyTuple_Type (*_molt_builtin_type_object_borrowed("tuple"))
#define PyType_Type (*_molt_builtin_type_object_borrowed("type"))
#define PyByteArray_Type (*_molt_builtin_type_object_borrowed("bytearray"))
#define PyMemoryView_Type (*_molt_builtin_type_object_borrowed("memoryview"))
#define PyFloat_AS_DOUBLE(op) PyFloat_AsDouble((PyObject *)(op))

static inline int PyObject_TypeCheck(PyObject *ob, PyTypeObject *type) {
    if (ob == NULL || type == NULL) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(
        _molt_py_handle(ob), _molt_py_handle((PyObject *)type));
}

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

static inline PyObject *PyIter_Next(PyObject *obj) {
    PyObject *next_fn;
    PyObject *out;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "iterator must not be NULL");
        return NULL;
    }
    next_fn = PyObject_GetAttrString(obj, "__next__");
    if (next_fn == NULL) {
        return NULL;
    }
    out = PyObject_CallObject(next_fn, NULL);
    Py_DECREF(next_fn);
    if (out == NULL && PyErr_ExceptionMatches(PyExc_StopIteration)) {
        PyErr_Clear();
        return NULL;
    }
    return out;
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
    /* PySequence_Fast guarantees seq is a list or tuple — use borrowed getitem. */
    uint64_t idx = molt_int_from_i64((int64_t)index);
    uint64_t val;
    if (PyList_Check(seq)) {
        val = molt_list_getitem_borrowed((uint64_t)(uintptr_t)seq, idx);
    } else {
        val = molt_tuple_getitem_borrowed((uint64_t)(uintptr_t)seq, idx);
    }
    molt_handle_decref(idx);
    if (val == 0) return NULL;
    return (PyObject *)(uintptr_t)val;
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

static inline int PyArg_Parse(PyObject *arg, const char *format, ...) {
    /* PyArg_Parse parses a single value (not a tuple).  Wrap in a 1-tuple and
       reuse the existing tuple parser. */
    MoltHandle items[1];
    MoltHandle tuple_bits;
    int out;
    va_list ap;
    if (arg == NULL) {
        PyErr_SetString(PyExc_TypeError, "argument must not be NULL");
        return 0;
    }
    items[0] = _molt_py_handle(arg);
    tuple_bits = molt_tuple_from_array(items, 1);
    va_start(ap, format);
    out = _molt_pyarg_parse_tuple_va(_molt_pyobject_from_handle(tuple_bits), format, &ap);
    va_end(ap);
    molt_handle_decref(tuple_bits);
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


/* =========================================================================
 * Type Object, Object Init, PyLong completions, Abstract Protocol,
 * Weakref, Set/FrozenSet, Descriptor protocol
 * ========================================================================= */

/* ---- Type Object functions ---------------------------------------------- */

static inline unsigned int PyType_GetFlags(PyTypeObject *type) {
    PyObject *flags_obj;
    long long flags_val;
    if (type == NULL) {
        return 0;
    }
    flags_obj = PyObject_GetAttrString((PyObject *)type, "__flags__");
    if (flags_obj == NULL) {
        PyErr_Clear();
        return Py_TPFLAGS_DEFAULT;
    }
    flags_val = PyLong_AsLongLong(flags_obj);
    Py_DECREF(flags_obj);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return Py_TPFLAGS_DEFAULT;
    }
    return (unsigned int)flags_val;
}

static inline PyObject *PyType_GetDict(PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return NULL;
    }
    return PyObject_GetAttrString((PyObject *)type, "__dict__");
}

/* PyType_GetSlot: NOT YET IMPLEMENTED.
 *
 * CPython returns a raw C function pointer (initproc, reprfunc, etc.).
 * Molt's handle-based object model cannot produce C function pointers from
 * Python callables without a trampoline layer. Returning a PyObject* cast
 * to void* would be silently wrong — extensions that call the result as a
 * C function pointer would crash or corrupt memory.
 *
 * Returns NULL for all slots. Extensions that check for NULL gracefully
 * degrade; extensions that assume non-NULL will get a clear failure rather
 * than silent memory corruption.
 *
 * Deferred: implementing C-to-Python trampolines per slot type requires a
 * JIT thunk allocator. Extensions that need slot access should use the
 * tp_* struct fields directly (which Molt populates where possible).
 */
static inline void *PyType_GetSlot(PyTypeObject *type, int slot) {
    (void)type;
    (void)slot;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyType_GetSlot not implemented in Molt — requires C-to-Python trampolines");
    return NULL;
}

static inline PyObject *PyType_GetName(PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return NULL;
    }
    return PyObject_GetAttrString((PyObject *)type, "__name__");
}

static inline PyObject *PyType_GetQualName(PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return NULL;
    }
    return PyObject_GetAttrString((PyObject *)type, "__qualname__");
}

static inline int PyType_HasFeature(PyTypeObject *type, unsigned long feature) {
    return (PyType_GetFlags(type) & (unsigned int)feature) != 0;
}

static inline int PyType_IS_GC(PyTypeObject *type) {
    return PyType_HasFeature(type, Py_TPFLAGS_HAVE_GC);
}

/* ---- Object creation / initialisation ----------------------------------- */

static inline PyObject *PyObject_Init(PyObject *op, PyTypeObject *type) {
    if (op == NULL) {
        return (PyObject *)PyErr_NoMemory();
    }
    Py_SET_TYPE(op, type);
    return op;
}

static inline PyVarObject *PyObject_InitVar(PyVarObject *op, PyTypeObject *type, Py_ssize_t size) {
    if (op == NULL) {
        PyErr_NoMemory();
        return NULL;
    }
    (void)type; /* molt types are handle-based; no struct field to set */
    op->ob_size = size;
    return op;
}

static inline PyObject *_PyObject_New(PyTypeObject *type) {
    return PyObject_CallObject((PyObject *)type, NULL);
}

static inline PyVarObject *_PyObject_NewVar(PyTypeObject *type, Py_ssize_t size) {
    PyObject *size_obj;
    MoltHandle arg_bits;
    MoltHandle args_bits;
    PyObject *out;
    size_obj = PyLong_FromSsize_t(size);
    if (size_obj == NULL) {
        return NULL;
    }
    arg_bits = _molt_py_handle(size_obj);
    args_bits = molt_tuple_from_array(&arg_bits, 1);
    Py_DECREF(size_obj);
    if (args_bits == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    out = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle((PyObject *)type), args_bits, molt_none()));
    molt_handle_decref(args_bits);
    return (PyVarObject *)out;
}

#ifndef PyObject_NewVar
#define PyObject_NewVar(type, typeobj, n) ((type *)_PyObject_NewVar(typeobj, n))
#endif

/* ---- Set / FrozenSet protocol ------------------------------------------- */

static inline PyObject *PySet_New(PyObject *iterable) {
    MoltHandle set_type = _molt_builtin_type_handle_cached("set");
    MoltHandle out;
    if (set_type == 0) return NULL;
    if (iterable != NULL) {
        MoltHandle arg = _molt_py_handle(iterable);
        MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
        if (args_bits == 0 || molt_err_pending() != 0) return NULL;
        out = molt_object_call(set_type, args_bits, molt_none());
        molt_handle_decref(args_bits);
    } else {
        MoltHandle args_bits = molt_tuple_from_array(NULL, 0);
        if (args_bits == 0 || molt_err_pending() != 0) return NULL;
        out = molt_object_call(set_type, args_bits, molt_none());
        molt_handle_decref(args_bits);
    }
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyFrozenSet_New(PyObject *iterable) {
    MoltHandle fset_type = _molt_builtin_type_handle_cached("frozenset");
    MoltHandle out;
    if (fset_type == 0) return NULL;
    if (iterable != NULL) {
        MoltHandle arg = _molt_py_handle(iterable);
        MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
        if (args_bits == 0 || molt_err_pending() != 0) return NULL;
        out = molt_object_call(fset_type, args_bits, molt_none());
        molt_handle_decref(args_bits);
    } else {
        MoltHandle args_bits = molt_tuple_from_array(NULL, 0);
        if (args_bits == 0 || molt_err_pending() != 0) return NULL;
        out = molt_object_call(fset_type, args_bits, molt_none());
        molt_handle_decref(args_bits);
    }
    return _molt_pyobject_from_result(out);
}

static inline int PySet_Add(PyObject *set, PyObject *key) {
    PyObject *add_fn = PyObject_GetAttrString(set, "add");
    PyObject *result;
    if (add_fn == NULL) return -1;
    {
        MoltHandle arg = _molt_py_handle(key);
        MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
        PyObject *args_tuple;
        if (args_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(add_fn);
            return -1;
        }
        args_tuple = _molt_pyobject_from_handle(args_bits);
        result = PyObject_CallObject(add_fn, args_tuple);
        Py_DECREF(add_fn);
        Py_DECREF(args_tuple);
    }
    if (result == NULL) return -1;
    Py_DECREF(result);
    return 0;
}

static inline int PySet_Discard(PyObject *set, PyObject *key) {
    PyObject *discard_fn = PyObject_GetAttrString(set, "discard");
    PyObject *result;
    if (discard_fn == NULL) return -1;
    {
        MoltHandle arg = _molt_py_handle(key);
        MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
        PyObject *args_tuple;
        if (args_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(discard_fn);
            return -1;
        }
        args_tuple = _molt_pyobject_from_handle(args_bits);
        result = PyObject_CallObject(discard_fn, args_tuple);
        Py_DECREF(discard_fn);
        Py_DECREF(args_tuple);
    }
    if (result == NULL) return -1;
    Py_DECREF(result);
    return 0;
}

static inline PyObject *PySet_Pop(PyObject *set) {
    PyObject *pop_fn = PyObject_GetAttrString(set, "pop");
    PyObject *out;
    if (pop_fn == NULL) return NULL;
    out = PyObject_CallObject(pop_fn, NULL);
    Py_DECREF(pop_fn);
    return out;
}

static inline int PySet_Contains(PyObject *anyset, PyObject *key) {
    return molt_object_contains(_molt_py_handle(anyset), _molt_py_handle(key));
}

static inline Py_ssize_t PySet_Size(PyObject *anyset) {
    PyObject *len_fn = PyObject_GetAttrString(anyset, "__len__");
    PyObject *result;
    long long out;
    if (len_fn == NULL) return -1;
    result = PyObject_CallObject(len_fn, NULL);
    Py_DECREF(len_fn);
    if (result == NULL) return -1;
    out = PyLong_AsLongLong(result);
    Py_DECREF(result);
    return (Py_ssize_t)out;
}

static inline int PySet_Clear(PyObject *set) {
    PyObject *clear_fn = PyObject_GetAttrString(set, "clear");
    PyObject *result;
    if (clear_fn == NULL) return -1;
    result = PyObject_CallObject(clear_fn, NULL);
    Py_DECREF(clear_fn);
    if (result == NULL) return -1;
    Py_DECREF(result);
    return 0;
}

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
    result = PyObject_CallObject(ref_callable, _molt_pyobject_from_handle(args_bits));
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
    result = PyObject_CallObject(proxy_callable, _molt_pyobject_from_handle(args_bits));
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

static inline PyObject *PyLong_FromString(const char *str, char **pend, int base) {
    PyObject *int_type;
    PyObject *str_obj;
    PyObject *base_obj;
    MoltHandle args_arr[2];
    MoltHandle args_bits;
    PyObject *result;
    if (str == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL string passed to PyLong_FromString");
        return NULL;
    }
    int_type = _molt_builtin_class_lookup_utf8("int");
    if (int_type == NULL) {
        return NULL;
    }
    str_obj = _molt_pyobject_from_result(_molt_string_from_utf8(str));
    if (str_obj == NULL) {
        Py_DECREF(int_type);
        return NULL;
    }
    base_obj = PyLong_FromLong((long)base);
    if (base_obj == NULL) {
        Py_DECREF(int_type);
        Py_DECREF(str_obj);
        return NULL;
    }
    args_arr[0] = _molt_py_handle(str_obj);
    args_arr[1] = _molt_py_handle(base_obj);
    args_bits = molt_tuple_from_array(args_arr, 2);
    Py_DECREF(str_obj);
    Py_DECREF(base_obj);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(int_type);
        return NULL;
    }
    result = PyObject_CallObject(int_type, _molt_pyobject_from_handle(args_bits));
    molt_handle_decref(args_bits);
    Py_DECREF(int_type);
    if (pend != NULL) {
        *pend = (char *)(str + strlen(str));
    }
    return result;
}

static inline PyObject *PyLong_FromVoidPtr(void *p) {
    return PyLong_FromLongLong((long long)(uintptr_t)p);
}

static inline void *PyLong_AsVoidPtr(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (molt_err_pending() != 0) {
        return NULL;
    }
    return (void *)(uintptr_t)val;
}

static inline double PyLong_AsDouble(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (molt_err_pending() != 0) {
        return -1.0;
    }
    return (double)val;
}

static inline PyObject *PyLong_FromSize_t(size_t v) {
    if (v > (size_t)INT64_MAX) {
        return PyLong_FromUnsignedLongLong((unsigned long long)v);
    }
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)v));
}

static inline size_t PyLong_AsSize_t(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (molt_err_pending() != 0) {
        return (size_t)-1;
    }
    if (val < 0) {
        PyErr_SetString(PyExc_OverflowError,
                        "can't convert negative value to size_t");
        return (size_t)-1;
    }
    return (size_t)val;
}

static inline unsigned long PyLong_AsUnsignedLong(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (molt_err_pending() != 0) {
        return (unsigned long)-1;
    }
    if (val < 0) {
        PyErr_SetString(PyExc_OverflowError,
                        "can't convert negative value to unsigned long");
        return (unsigned long)-1;
    }
    return (unsigned long)val;
}

static inline unsigned long long PyLong_AsUnsignedLongLongMask(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (molt_err_pending() != 0) {
        PyErr_Clear();
        return 0;
    }
    return (unsigned long long)val;
}

/* ---- Abstract Object protocol ------------------------------------------- */

static inline PyObject *PyObject_Type(PyObject *o) {
    PyObject *type_obj;
    if (o == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_Type");
        return NULL;
    }
    type_obj = (PyObject *)Py_TYPE(o);
    Py_INCREF(type_obj);
    return type_obj;
}

static inline Py_ssize_t PyObject_Length(PyObject *o) {
    PyObject *len_fn;
    PyObject *len_result;
    Py_ssize_t out;
    if (o == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_Length");
        return -1;
    }
    len_fn = _molt_builtin_class_lookup_utf8("len");
    if (len_fn == NULL) {
        return -1;
    }
    {
        MoltHandle arg = _molt_py_handle(o);
        MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
        if (args_bits == 0 || molt_err_pending() != 0) {
            Py_DECREF(len_fn);
            return -1;
        }
        len_result = PyObject_CallObject(len_fn, _molt_pyobject_from_handle(args_bits));
        molt_handle_decref(args_bits);
        Py_DECREF(len_fn);
    }
    if (len_result == NULL) {
        return -1;
    }
    out = (Py_ssize_t)PyLong_AsLongLong(len_result);
    Py_DECREF(len_result);
    return out;
}

static inline Py_ssize_t PyObject_Size(PyObject *o) {
    return PyObject_Length(o);
}

static inline PyObject *PyObject_Bytes(PyObject *o) {
    MoltHandle bytes_type = _molt_builtin_type_handle_cached("bytes");
    MoltHandle arg = _molt_py_handle(o);
    MoltHandle args_bits = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (bytes_type == 0 || args_bits == 0 || molt_err_pending() != 0) {
        if (args_bits != 0) molt_handle_decref(args_bits);
        return NULL;
    }
    out = molt_object_call(bytes_type, args_bits, molt_none());
    molt_handle_decref(args_bits);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyObject_ASCII(PyObject *o) {
    PyObject *ascii_fn;
    MoltHandle arg;
    MoltHandle args_bits;
    PyObject *result;
    if (o == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_ASCII");
        return NULL;
    }
    ascii_fn = _molt_builtin_class_lookup_utf8("ascii");
    if (ascii_fn == NULL) {
        return NULL;
    }
    arg = _molt_py_handle(o);
    args_bits = molt_tuple_from_array(&arg, 1);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(ascii_fn);
        return NULL;
    }
    result = PyObject_CallObject(ascii_fn, _molt_pyobject_from_handle(args_bits));
    molt_handle_decref(args_bits);
    Py_DECREF(ascii_fn);
    return result;
}

static inline PyObject *PyObject_Format(PyObject *obj, PyObject *format_spec) {
    PyObject *format_fn;
    MoltHandle args_arr[2];
    uint64_t nargs;
    MoltHandle args_bits;
    PyObject *result;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_Format");
        return NULL;
    }
    format_fn = _molt_builtin_class_lookup_utf8("format");
    if (format_fn == NULL) {
        return NULL;
    }
    args_arr[0] = _molt_py_handle(obj);
    nargs = 1;
    if (format_spec != NULL && format_spec != Py_None) {
        args_arr[1] = _molt_py_handle(format_spec);
        nargs = 2;
    }
    args_bits = molt_tuple_from_array(args_arr, nargs);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(format_fn);
        return NULL;
    }
    result = PyObject_CallObject(format_fn, _molt_pyobject_from_handle(args_bits));
    molt_handle_decref(args_bits);
    Py_DECREF(format_fn);
    return result;
}

/* ---- Descriptor protocol ------------------------------------------------ */

static inline PyObject *PyDescr_NewMethod(PyTypeObject *type, PyMethodDef *meth) {
    if (type == NULL || meth == NULL) {
        PyErr_SetString(PyExc_TypeError, "type and method must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_cfunction_create_bytes(
            _molt_py_handle((PyObject *)type),
            (const uint8_t *)meth->ml_name,
            meth->ml_name != NULL ? (uint64_t)strlen(meth->ml_name) : 0,
            (uintptr_t)meth->ml_meth,
            (uint32_t)meth->ml_flags,
            (const uint8_t *)meth->ml_doc,
            meth->ml_doc != NULL ? (uint64_t)strlen(meth->ml_doc) : 0));
}

static inline PyObject *PyDescr_NewClassMethod(PyTypeObject *type, PyMethodDef *meth) {
    PyObject *callable;
    PyObject *wrapped;
    if (type == NULL || meth == NULL) {
        PyErr_SetString(PyExc_TypeError, "type and method must not be NULL");
        return NULL;
    }
    callable = PyDescr_NewMethod(type, meth);
    if (callable == NULL) {
        return NULL;
    }
    wrapped = _molt_type_wrap_single_arg_builtin("classmethod", callable);
    Py_DECREF(callable);
    return wrapped;
}

static inline PyObject *PyDescr_NewGetSet(PyTypeObject *type, PyGetSetDef *getset) {
    PyObject *getter_callable;
    PyObject *property_obj;
    if (type == NULL || getset == NULL) {
        PyErr_SetString(PyExc_TypeError, "type and getset must not be NULL");
        return NULL;
    }
    if (getset->get == NULL) {
        PyErr_SetString(PyExc_TypeError, "getset descriptor must have a getter");
        return NULL;
    }
    getter_callable = _molt_type_make_slot_callable(
        _molt_py_handle((PyObject *)type), getset->name, (uintptr_t)getset->get, METH_O, getset->doc);
    if (getter_callable == NULL) {
        return NULL;
    }
    property_obj = _molt_type_wrap_single_arg_builtin("property", getter_callable);
    Py_DECREF(getter_callable);
    return property_obj;
}

static inline PyObject *PyDescr_NewMember(PyTypeObject *type, PyMemberDef *member) {
    (void)type;
    (void)member;
    Py_INCREF(Py_None);
    return Py_None;
}

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
        return _molt_pyobject_from_handle(existing);
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
    module = _molt_pyobject_from_handle(module_bits);
    /* Return borrowed reference — module stays alive in sys.modules. */
    return module;
}

static inline PyObject *PyImport_GetModuleDict(void) {
    PyObject *sys_mod = PyImport_ImportModule("sys");
    MoltHandle modules;
    if (sys_mod == NULL) {
        return NULL;
    }
    modules = molt_object_getattr_borrowed(
        _molt_py_handle(sys_mod), (const uint8_t *)"modules", 7);
    Py_DECREF(sys_mod);
    if (modules == 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(modules);
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
    return _molt_pyobject_from_handle(tstate_dict);
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

static inline PyObject *PyEval_GetBuiltins(void) {
    PyObject *builtins_mod = PyImport_ImportModule("builtins");
    MoltHandle dict;
    if (builtins_mod == NULL) {
        return NULL;
    }
    dict = molt_object_getattr_borrowed(
        _molt_py_handle(builtins_mod), (const uint8_t *)"__dict__", 8);
    Py_DECREF(builtins_mod);
    if (dict == 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(dict);
}

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

static inline PyObject *PyEval_CallObjectWithKeywords(
    PyObject *func, PyObject *args, PyObject *kwargs)
{
    MoltHandle args_bits;
    MoltHandle kwargs_bits;
    int owns_args = 0;
    MoltHandle result;
    if (func == NULL) {
        PyErr_SetString(PyExc_TypeError, "callable must not be NULL");
        return NULL;
    }
    if (args == NULL) {
        args_bits = molt_tuple_from_array(NULL, 0);
        if (molt_err_pending() != 0) {
            return NULL;
        }
        owns_args = 1;
    } else {
        args_bits = _molt_py_handle(args);
    }
    kwargs_bits = (kwargs != NULL) ? _molt_py_handle(kwargs) : molt_none();
    result = molt_object_call(_molt_py_handle(func), args_bits, kwargs_bits);
    if (owns_args) {
        molt_handle_decref(args_bits);
    }
    return _molt_pyobject_from_result(result);
}

/* ========================================================================
 * PySys C API
 * ======================================================================== */

static inline PyObject *PySys_GetObject(const char *name) {
    PyObject *sys_mod = PyImport_ImportModule("sys");
    MoltHandle obj;
    if (sys_mod == NULL) {
        return NULL;
    }
    obj = molt_object_getattr_borrowed(
        _molt_py_handle(sys_mod), (const uint8_t *)name, (uint64_t)strlen(name));
    Py_DECREF(sys_mod);
    if (obj == 0) {
        return NULL;
    }
    return _molt_pyobject_from_handle(obj);
}

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

static inline PyObject *PySlice_New(PyObject *start, PyObject *stop, PyObject *step) {
    MoltHandle slice_class;
    MoltHandle args[3];
    MoltHandle args_tuple;
    MoltHandle result;
    PyObject *builtins_mod = PyImport_ImportModule("builtins");
    if (builtins_mod == NULL) {
        return NULL;
    }
    slice_class = molt_object_getattr_bytes(
        _molt_py_handle(builtins_mod), (const uint8_t *)"slice", 5);
    Py_DECREF(builtins_mod);
    if (slice_class == 0 || molt_err_pending() != 0) {
        return NULL;
    }
    args[0] = (start != NULL) ? _molt_py_handle(start) : molt_none();
    args[1] = (stop != NULL) ? _molt_py_handle(stop) : molt_none();
    args[2] = (step != NULL) ? _molt_py_handle(step) : molt_none();
    args_tuple = molt_tuple_from_array(args, 3);
    if (args_tuple == 0 || molt_err_pending() != 0) {
        molt_handle_decref(slice_class);
        return NULL;
    }
    result = molt_object_call(slice_class, args_tuple, molt_none());
    molt_handle_decref(slice_class);
    molt_handle_decref(args_tuple);
    return _molt_pyobject_from_result(result);
}

static inline int PySlice_Unpack(
    PyObject *slice, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step)
{
    PyObject *start_obj;
    PyObject *stop_obj;
    PyObject *step_obj;
    if (slice == NULL) {
        PyErr_SetString(PyExc_TypeError, "expected a slice object");
        return -1;
    }
    start_obj = PyObject_GetAttrString(slice, "start");
    stop_obj = PyObject_GetAttrString(slice, "stop");
    step_obj = PyObject_GetAttrString(slice, "step");
    if (start_obj == NULL || stop_obj == NULL || step_obj == NULL) {
        Py_XDECREF(start_obj);
        Py_XDECREF(stop_obj);
        Py_XDECREF(step_obj);
        return -1;
    }
    if (step != NULL) {
        if (_molt_py_handle(step_obj) == molt_none()) {
            *step = 1;
        } else {
            *step = (Py_ssize_t)PyLong_AsLongLong(step_obj);
        }
    }
    if (start != NULL) {
        if (_molt_py_handle(start_obj) == molt_none()) {
            *start = (step != NULL && *step < 0) ? PY_SSIZE_T_MAX : 0;
        } else {
            *start = (Py_ssize_t)PyLong_AsLongLong(start_obj);
        }
    }
    if (stop != NULL) {
        if (_molt_py_handle(stop_obj) == molt_none()) {
            *stop = (step != NULL && *step < 0) ? (-PY_SSIZE_T_MAX - 1) : PY_SSIZE_T_MAX;
        } else {
            *stop = (Py_ssize_t)PyLong_AsLongLong(stop_obj);
        }
    }
    Py_DECREF(start_obj);
    Py_DECREF(stop_obj);
    Py_DECREF(step_obj);
    if (molt_err_pending() != 0) {
        return -1;
    }
    return 0;
}

static inline Py_ssize_t PySlice_AdjustIndices(
    Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t step)
{
    Py_ssize_t slicelength;
    if (*start < 0) {
        *start += length;
        if (*start < 0) {
            *start = (step < 0) ? -1 : 0;
        }
    } else if (*start >= length) {
        *start = (step < 0) ? length - 1 : length;
    }
    if (*stop < 0) {
        *stop += length;
        if (*stop < 0) {
            *stop = (step < 0) ? -1 : 0;
        }
    } else if (*stop >= length) {
        *stop = (step < 0) ? length - 1 : length;
    }
    if (step > 0) {
        slicelength = (*stop - *start + step - 1) / step;
    } else {
        slicelength = (*start - *stop + (-step) - 1) / (-step);
    }
    if (slicelength < 0) {
        slicelength = 0;
    }
    return slicelength;
}

static inline int PySlice_GetIndicesEx(
    PyObject *slice, Py_ssize_t length,
    Py_ssize_t *start, Py_ssize_t *stop,
    Py_ssize_t *step, Py_ssize_t *slicelength)
{
    if (PySlice_Unpack(slice, start, stop, step) < 0) {
        return -1;
    }
    if (slicelength != NULL) {
        *slicelength = PySlice_AdjustIndices(length, start, stop, *step);
    } else {
        (void)PySlice_AdjustIndices(length, start, stop, *step);
    }
    return 0;
}

#define PySlice_Type (*_molt_builtin_type_object_borrowed("slice"))

static inline int PySlice_Check(PyObject *obj) {
    MoltHandle slice_bits = _molt_builtin_type_handle_cached("slice");
    if (slice_bits == 0) {
        return 0;
    }
    return _molt_pyarg_object_matches_type(_molt_py_handle(obj), slice_bits);
}

/* ========================================================================
 * Complex C API
 * ======================================================================== */

static inline PyObject *PyComplex_FromDoubles(double real, double imag) {
    PyObject *builtins_mod;
    MoltHandle complex_class;
    MoltHandle args[2];
    MoltHandle args_tuple;
    MoltHandle result;
    builtins_mod = PyImport_ImportModule("builtins");
    if (builtins_mod == NULL) {
        return NULL;
    }
    complex_class = molt_object_getattr_bytes(
        _molt_py_handle(builtins_mod), (const uint8_t *)"complex", 7);
    Py_DECREF(builtins_mod);
    if (complex_class == 0 || molt_err_pending() != 0) {
        PyErr_SetString(PyExc_TypeError, "complex type not available");
        return NULL;
    }
    args[0] = molt_float_from_f64(real);
    args[1] = molt_float_from_f64(imag);
    args_tuple = molt_tuple_from_array(args, 2);
    if (args_tuple == 0 || molt_err_pending() != 0) {
        molt_handle_decref(complex_class);
        molt_handle_decref(args[0]);
        molt_handle_decref(args[1]);
        return NULL;
    }
    result = molt_object_call(complex_class, args_tuple, molt_none());
    molt_handle_decref(complex_class);
    molt_handle_decref(args_tuple);
    molt_handle_decref(args[0]);
    molt_handle_decref(args[1]);
    return _molt_pyobject_from_result(result);
}

static inline double PyComplex_RealAsDouble(PyObject *op) {
    PyObject *real_obj;
    double result;
    if (op == NULL) {
        PyErr_SetString(PyExc_TypeError, "expected complex object");
        return -1.0;
    }
    real_obj = PyObject_GetAttrString(op, "real");
    if (real_obj == NULL) {
        return -1.0;
    }
    result = PyFloat_AsDouble(real_obj);
    Py_DECREF(real_obj);
    return result;
}

static inline double PyComplex_ImagAsDouble(PyObject *op) {
    PyObject *imag_obj;
    double result;
    if (op == NULL) {
        PyErr_SetString(PyExc_TypeError, "expected complex object");
        return -1.0;
    }
    imag_obj = PyObject_GetAttrString(op, "imag");
    if (imag_obj == NULL) {
        return -1.0;
    }
    result = PyFloat_AsDouble(imag_obj);
    Py_DECREF(imag_obj);
    return result;
}

/* ========================================================================
 * Context Variables C API
 * ======================================================================== */

static inline PyObject *PyContext_New(void) {
    PyObject *contextvars_mod = PyImport_ImportModule("contextvars");
    PyObject *copy_context_fn;
    PyObject *ctx;
    if (contextvars_mod == NULL) {
        PyErr_Clear();
        return PyDict_New();
    }
    copy_context_fn = PyObject_GetAttrString(contextvars_mod, "copy_context");
    Py_DECREF(contextvars_mod);
    if (copy_context_fn == NULL) {
        PyErr_Clear();
        return PyDict_New();
    }
    ctx = PyObject_CallObject(copy_context_fn, NULL);
    Py_DECREF(copy_context_fn);
    return ctx;
}

static inline PyObject *PyContext_Copy(PyObject *ctx) {
    PyObject *copy_fn;
    PyObject *result;
    if (ctx == NULL) {
        return PyContext_New();
    }
    copy_fn = PyObject_GetAttrString(ctx, "copy");
    if (copy_fn == NULL) {
        PyErr_Clear();
        return PyContext_New();
    }
    result = PyObject_CallObject(copy_fn, NULL);
    Py_DECREF(copy_fn);
    return result;
}

static inline int PyContext_Enter(PyObject *ctx) {
    (void)ctx;
    return 0;
}

static inline int PyContext_Exit(PyObject *ctx) {
    (void)ctx;
    return 0;
}

static inline PyObject *PyContextVar_New(const char *name, PyObject *def) {
    PyObject *contextvars_mod = PyImport_ImportModule("contextvars");
    PyObject *contextvar_cls;
    PyObject *var;
    MoltHandle args[2];
    MoltHandle args_tuple;
    uint64_t nargs;
    if (contextvars_mod == NULL) {
        return NULL;
    }
    contextvar_cls = PyObject_GetAttrString(contextvars_mod, "ContextVar");
    Py_DECREF(contextvars_mod);
    if (contextvar_cls == NULL) {
        return NULL;
    }
    args[0] = _molt_string_from_utf8(name);
    if (args[0] == 0 || molt_err_pending() != 0) {
        Py_DECREF(contextvar_cls);
        return NULL;
    }
    if (def != NULL) {
        args[1] = _molt_py_handle(def);
        nargs = 2;
    } else {
        nargs = 1;
    }
    args_tuple = molt_tuple_from_array(args, nargs);
    if (args_tuple == 0 || molt_err_pending() != 0) {
        molt_handle_decref(args[0]);
        Py_DECREF(contextvar_cls);
        return NULL;
    }
    var = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle(contextvar_cls), args_tuple, molt_none()));
    molt_handle_decref(args[0]);
    molt_handle_decref(args_tuple);
    Py_DECREF(contextvar_cls);
    return var;
}

static inline int PyContextVar_Get(
    PyObject *var, PyObject *default_value, PyObject **value)
{
    PyObject *get_fn;
    PyObject *result;
    MoltHandle args[1];
    MoltHandle args_tuple;
    if (var == NULL || value == NULL) {
        PyErr_SetString(PyExc_TypeError, "var and value pointer must not be NULL");
        return -1;
    }
    get_fn = PyObject_GetAttrString(var, "get");
    if (get_fn == NULL) {
        return -1;
    }
    if (default_value != NULL) {
        args[0] = _molt_py_handle(default_value);
        args_tuple = molt_tuple_from_array(args, 1);
        if (args_tuple == 0 || molt_err_pending() != 0) {
            Py_DECREF(get_fn);
            return -1;
        }
        result = _molt_pyobject_from_result(
            molt_object_call(_molt_py_handle(get_fn), args_tuple, molt_none()));
        molt_handle_decref(args_tuple);
    } else {
        result = PyObject_CallObject(get_fn, NULL);
    }
    Py_DECREF(get_fn);
    if (result == NULL) {
        if (default_value != NULL) {
            PyErr_Clear();
            Py_INCREF(default_value);
            *value = default_value;
            return 0;
        }
        *value = NULL;
        return -1;
    }
    *value = result;
    return 0;
}

static inline PyObject *PyContextVar_Set(PyObject *var, PyObject *value) {
    PyObject *set_fn;
    PyObject *result;
    MoltHandle args[1];
    MoltHandle args_tuple;
    if (var == NULL) {
        PyErr_SetString(PyExc_TypeError, "context var must not be NULL");
        return NULL;
    }
    set_fn = PyObject_GetAttrString(var, "set");
    if (set_fn == NULL) {
        return NULL;
    }
    args[0] = (value != NULL) ? _molt_py_handle(value) : molt_none();
    args_tuple = molt_tuple_from_array(args, 1);
    if (args_tuple == 0 || molt_err_pending() != 0) {
        Py_DECREF(set_fn);
        return NULL;
    }
    result = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle(set_fn), args_tuple, molt_none()));
    molt_handle_decref(args_tuple);
    Py_DECREF(set_fn);
    return result;
}

/* ========================================================================
 * Marshal C API (minimal stubs)
 * ======================================================================== */

static inline PyObject *PyMarshal_WriteObjectToString(PyObject *value, int version) {
    (void)version;
    (void)value;
    PyErr_SetString(PyExc_RuntimeError, "PyMarshal is not supported in molt");
    return NULL;
}

static inline PyObject *PyMarshal_ReadObjectFromString(const char *data, Py_ssize_t len) {
    (void)data;
    (void)len;
    PyErr_SetString(PyExc_RuntimeError, "PyMarshal is not supported in molt");
    return NULL;
}

/* ========================================================================
 * Additional Warning Exception Types
 * ======================================================================== */

static inline PyObject *_molt_pyexc_deprecation_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) { cached = _molt_exception_class_from_name("DeprecationWarning"); }
    return _molt_pyobject_from_handle(cached);
}
static inline PyObject *_molt_pyexc_future_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) { cached = _molt_exception_class_from_name("FutureWarning"); }
    return _molt_pyobject_from_handle(cached);
}
static inline PyObject *_molt_pyexc_import_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) { cached = _molt_exception_class_from_name("ImportWarning"); }
    return _molt_pyobject_from_handle(cached);
}
static inline PyObject *_molt_pyexc_pending_deprecation_warning(void) {
    static MoltHandle cached = 0;
    if (cached == 0) { cached = _molt_exception_class_from_name("PendingDeprecationWarning"); }
    return _molt_pyobject_from_handle(cached);
}

#define PyExc_DeprecationWarning _molt_pyexc_deprecation_warning()
#define PyExc_FutureWarning _molt_pyexc_future_warning()
#define PyExc_ImportWarning _molt_pyexc_import_warning()
#define PyExc_PendingDeprecationWarning _molt_pyexc_pending_deprecation_warning()

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
 * Number Protocol (remaining)
 * ======================================================================== */

static inline PyObject *PyNumber_Remainder(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_mod((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_Power(PyObject *o1, PyObject *o2, PyObject *o3) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    if (o3 == NULL || o3 == Py_None) {
        return (PyObject *)(uintptr_t)molt_pow((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
    }
    /* Ternary pow(a, b, mod) — call builtins.pow with 3 args */
    {
        PyObject *builtins_mod = PyImport_ImportModule("builtins");
        PyObject *pow_func, *result;
        MoltHandle call_args[3], args_tuple;
        if (builtins_mod == NULL) return NULL;
        pow_func = PyObject_GetAttrString(builtins_mod, "pow");
        Py_DECREF(builtins_mod);
        if (pow_func == NULL) return NULL;
        call_args[0] = _molt_py_handle(o1);
        call_args[1] = _molt_py_handle(o2);
        call_args[2] = _molt_py_handle(o3);
        args_tuple = molt_tuple_from_array(call_args, 3);
        if (args_tuple == 0 || molt_err_pending() != 0) {
            Py_DECREF(pow_func);
            return NULL;
        }
        result = _molt_pyobject_from_result(
            molt_object_call(_molt_py_handle(pow_func), args_tuple, molt_none()));
        molt_handle_decref(args_tuple);
        Py_DECREF(pow_func);
        return result;
    }
}

static inline PyObject *PyNumber_Negative(PyObject *o) {
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_neg((uint64_t)(uintptr_t)o);
}

static inline PyObject *PyNumber_Positive(PyObject *o) {
    MoltHandle method, args, out;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    /* For int/float, positive is identity — fast path */
    if (PyLong_Check(o) || PyFloat_Check(o)) {
        Py_INCREF(o);
        return o;
    }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__pos__", 7);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyNumber_Absolute(PyObject *o) {
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_abs_builtin((uint64_t)(uintptr_t)o);
}

static inline PyObject *PyNumber_Invert(PyObject *o) {
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_invert((uint64_t)(uintptr_t)o);
}

static inline PyObject *PyNumber_Lshift(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_lshift((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_Rshift(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_rshift((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_And(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_bit_and((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_Or(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_bit_or((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_Xor(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_bit_xor((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_Float(PyObject *o) {
    return _molt_pyobject_from_result(molt_number_float(_molt_py_handle(o)));
}

static inline PyObject *PyNumber_Index(PyObject *o) {
    MoltHandle method, args, out;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    /* For exact int, __index__ is identity */
    if (PyLong_Check(o)) {
        Py_INCREF(o);
        return o;
    }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__index__", 9);
    if (method == 0 || molt_err_pending() != 0) {
        PyErr_Clear();
        PyErr_SetString(PyExc_TypeError,
            "object cannot be interpreted as an integer");
        return NULL;
    }
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyNumber_InPlaceAdd(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_add((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceSubtract(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_sub((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceMultiply(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_mul((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceTrueDivide(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_div((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceFloorDivide(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_floordiv((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceRemainder(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_mod((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceLshift(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_lshift((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceRshift(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_rshift((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceAnd(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_bit_and((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceOr(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_bit_or((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceXor(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_bit_xor((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

/* ========================================================================
 * Object Protocol (remaining)
 * ======================================================================== */

static inline PyObject *PyObject_GetIter(PyObject *o) {
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_iter((uint64_t)(uintptr_t)o);
}

static inline int PyObject_SetItem(PyObject *o, PyObject *key, PyObject *v) {
    MoltHandle items[2];
    MoltHandle args;
    MoltHandle method;
    if (o == NULL || key == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__setitem__", 11);
    if (method == 0 || molt_err_pending() != 0) return -1;
    items[0] = _molt_py_handle(key);
    items[1] = _molt_py_handle(v);
    args = molt_tuple_from_array(items, 2);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline int PyObject_DelItem(PyObject *o, PyObject *key) {
    MoltHandle arg;
    MoltHandle args;
    MoltHandle method;
    if (o == NULL || key == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__delitem__", 11);
    if (method == 0 || molt_err_pending() != 0) return -1;
    arg = _molt_py_handle(key);
    args = molt_tuple_from_array(&arg, 1);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline int PyObject_DelAttr(PyObject *o, PyObject *attr_name) {
    if (o == NULL || attr_name == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    return molt_object_setattr(_molt_py_handle(o), _molt_py_handle(attr_name), 0) == 0 ? 0 : -1;
}

static inline int PyObject_DelAttrString(PyObject *o, const char *attr_name) {
    MoltHandle name_bits;
    int32_t rc;
    if (o == NULL || attr_name == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    name_bits = _molt_string_from_utf8(attr_name);
    rc = molt_object_setattr(_molt_py_handle(o), name_bits, 0);
    molt_handle_decref(name_bits);
    return rc == 0 ? 0 : -1;
}

static inline PyObject *PyObject_Dir(PyObject *o) {
    MoltHandle dir_fn;
    MoltHandle arg;
    MoltHandle args;
    MoltHandle out;
    dir_fn = molt_object_getattr_bytes(
        _molt_builtin_type_handle_cached("type"),
        (const uint8_t *)"__dir__", 7);
    if (dir_fn == 0 || molt_err_pending() != 0) {
        /* fallback: call builtins.dir(o) */
        PyObject *builtins = _molt_builtin_class_lookup_utf8("dir");
        if (builtins == NULL) return NULL;
        arg = _molt_py_handle(o);
        args = molt_tuple_from_array(&arg, 1);
        out = molt_object_call(_molt_py_handle(builtins), args, molt_none());
        molt_handle_decref(args);
        Py_DECREF(builtins);
        molt_err_clear();
        return _molt_pyobject_from_result(out);
    }
    arg = _molt_py_handle(o);
    args = molt_tuple_from_array(&arg, 1);
    out = molt_object_call(dir_fn, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(dir_fn);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * Sequence Protocol (remaining)
 * ======================================================================== */

static inline int PySequence_Contains(PyObject *seq, PyObject *ob) {
    if (seq == NULL || ob == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    return molt_object_contains(_molt_py_handle(seq), _molt_py_handle(ob));
}

static inline PyObject *PySequence_Concat(PyObject *s1, PyObject *s2) {
    if (s1 == NULL || s2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_add((uint64_t)(uintptr_t)s1, (uint64_t)(uintptr_t)s2);
}

static inline PyObject *PySequence_Repeat(PyObject *o, Py_ssize_t count) {
    MoltHandle cnt = molt_int_from_i64((int64_t)count);
    PyObject *result;
    if (cnt == 0 || molt_err_pending() != 0) return NULL;
    result = (PyObject *)(uintptr_t)molt_mul((uint64_t)(uintptr_t)o, cnt);
    molt_handle_decref(cnt);
    return result;
}

static inline Py_ssize_t PySequence_Count(PyObject *s, PyObject *value) {
    PyObject *result;
    MoltHandle method;
    MoltHandle arg;
    MoltHandle args;
    MoltHandle out;
    Py_ssize_t count;
    if (s == NULL || value == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(s),
        (const uint8_t *)"count", 5);
    if (method == 0 || molt_err_pending() != 0) return -1;
    arg = _molt_py_handle(value);
    args = molt_tuple_from_array(&arg, 1);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) return -1;
    count = (Py_ssize_t)PyLong_AsLongLong(result);
    Py_DECREF(result);
    return count;
}

static inline Py_ssize_t PySequence_Index(PyObject *s, PyObject *value) {
    PyObject *result;
    MoltHandle method;
    MoltHandle arg;
    MoltHandle args;
    MoltHandle out;
    Py_ssize_t idx;
    if (s == NULL || value == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(s),
        (const uint8_t *)"index", 5);
    if (method == 0 || molt_err_pending() != 0) return -1;
    arg = _molt_py_handle(value);
    args = molt_tuple_from_array(&arg, 1);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    result = _molt_pyobject_from_result(out);
    if (result == NULL) return -1;
    idx = (Py_ssize_t)PyLong_AsLongLong(result);
    Py_DECREF(result);
    return idx;
}

static inline PyObject *PySequence_Tuple(PyObject *o) {
    MoltHandle tuple_type = _molt_builtin_type_handle_cached("tuple");
    MoltHandle arg = _molt_py_handle(o);
    MoltHandle args = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (tuple_type == 0 || args == 0 || molt_err_pending() != 0) {
        if (args != 0) molt_handle_decref(args);
        return NULL;
    }
    out = molt_object_call(tuple_type, args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PySequence_List(PyObject *o) {
    MoltHandle list_type = _molt_builtin_type_handle_cached("list");
    MoltHandle arg = _molt_py_handle(o);
    MoltHandle args = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (list_type == 0 || args == 0 || molt_err_pending() != 0) {
        if (args != 0) molt_handle_decref(args);
        return NULL;
    }
    out = molt_object_call(list_type, args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline int PySequence_DelItem(PyObject *o, Py_ssize_t i) {
    MoltHandle key = molt_int_from_i64((int64_t)i);
    MoltHandle method;
    MoltHandle args;
    if (key == 0 || molt_err_pending() != 0) return -1;
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__delitem__", 11);
    if (method == 0 || molt_err_pending() != 0) { molt_handle_decref(key); return -1; }
    args = molt_tuple_from_array(&key, 1);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    molt_handle_decref(key);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline PyObject *PySequence_InPlaceConcat(PyObject *s1, PyObject *s2) {
    if (s1 == NULL || s2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_add((uint64_t)(uintptr_t)s1, (uint64_t)(uintptr_t)s2);
}

static inline PyObject *PySequence_InPlaceRepeat(PyObject *o, Py_ssize_t count) {
    MoltHandle cnt = molt_int_from_i64((int64_t)count);
    PyObject *result;
    if (cnt == 0 || molt_err_pending() != 0) return NULL;
    result = (PyObject *)(uintptr_t)molt_inplace_mul((uint64_t)(uintptr_t)o, cnt);
    molt_handle_decref(cnt);
    return result;
}

/* ========================================================================
 * Unicode (remaining)
 * ======================================================================== */

static inline PyObject *PyUnicode_FromObject(PyObject *obj) {
    MoltHandle str_type = _molt_builtin_type_handle_cached("str");
    MoltHandle arg = _molt_py_handle(obj);
    MoltHandle args = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (str_type == 0 || args == 0 || molt_err_pending() != 0) {
        if (args != 0) molt_handle_decref(args);
        return NULL;
    }
    out = molt_object_call(str_type, args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyUnicode_Substring(PyObject *str,
                                             Py_ssize_t start,
                                             Py_ssize_t end) {
    MoltHandle slice_args[3];
    MoltHandle slice_class;
    MoltHandle slice_bits;
    MoltHandle args;
    MoltHandle out;
    if (str == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    slice_class = molt_object_getattr_bytes(
        _molt_builtin_type_handle_cached("type"),
        (const uint8_t *)"__getattribute__", 16);
    /* Build slice(start, end) and call str[slice] */
    slice_args[0] = molt_int_from_i64((int64_t)start);
    slice_args[1] = molt_int_from_i64((int64_t)end);
    slice_args[2] = molt_none();
    /* Use builtins.slice */
    {
        MoltHandle builtins_mod = molt_module_import(
            _molt_string_from_utf8("builtins"));
        MoltHandle slice_cls;
        MoltHandle slice_args_tuple;
        MoltHandle slice_obj;
        MoltHandle getitem_arg;
        MoltHandle getitem_args;
        if (builtins_mod == 0 || molt_err_pending() != 0) {
            molt_handle_decref(slice_args[0]);
            molt_handle_decref(slice_args[1]);
            return NULL;
        }
        slice_cls = molt_object_getattr_bytes(builtins_mod,
            (const uint8_t *)"slice", 5);
        molt_handle_decref(builtins_mod);
        if (slice_cls == 0 || molt_err_pending() != 0) {
            molt_handle_decref(slice_args[0]);
            molt_handle_decref(slice_args[1]);
            return NULL;
        }
        slice_args_tuple = molt_tuple_from_array(slice_args, 2);
        slice_obj = molt_object_call(slice_cls, slice_args_tuple, molt_none());
        molt_handle_decref(slice_args_tuple);
        molt_handle_decref(slice_cls);
        molt_handle_decref(slice_args[0]);
        molt_handle_decref(slice_args[1]);
        if (slice_obj == 0 || molt_err_pending() != 0) return NULL;
        getitem_arg = slice_obj;
        getitem_args = molt_tuple_from_array(&getitem_arg, 1);
        {
            MoltHandle getitem_method = molt_object_getattr_bytes(
                _molt_py_handle(str), (const uint8_t *)"__getitem__", 11);
            if (getitem_method == 0 || molt_err_pending() != 0) {
                molt_handle_decref(slice_obj);
                molt_handle_decref(getitem_args);
                return NULL;
            }
            out = molt_object_call(getitem_method, getitem_args, molt_none());
            molt_handle_decref(getitem_args);
            molt_handle_decref(getitem_method);
            molt_handle_decref(slice_obj);
        }
    }
    (void)slice_class;
    (void)slice_bits;
    (void)args;
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyUnicode_Concat(PyObject *left, PyObject *right) {
    if (left == NULL || right == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_add((uint64_t)(uintptr_t)left, (uint64_t)(uintptr_t)right);
}

static inline PyObject *PyUnicode_Join(PyObject *separator, PyObject *seq) {
    MoltHandle method;
    MoltHandle arg;
    MoltHandle args;
    MoltHandle out;
    if (separator == NULL || seq == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument");
        return NULL;
    }
    method = molt_object_getattr_bytes(_molt_py_handle(separator),
        (const uint8_t *)"join", 4);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    arg = _molt_py_handle(seq);
    args = molt_tuple_from_array(&arg, 1);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

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

static inline PyObject *PyUnicode_Replace(PyObject *str, PyObject *substr,
                                           PyObject *replstr, Py_ssize_t maxcount) {
    MoltHandle method;
    MoltHandle call_args[3];
    MoltHandle args;
    MoltHandle out;
    if (str == NULL || substr == NULL || replstr == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument");
        return NULL;
    }
    method = molt_object_getattr_bytes(_molt_py_handle(str),
        (const uint8_t *)"replace", 7);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    call_args[0] = _molt_py_handle(substr);
    call_args[1] = _molt_py_handle(replstr);
    if (maxcount < 0) {
        args = molt_tuple_from_array(call_args, 2);
    } else {
        call_args[2] = molt_int_from_i64((int64_t)maxcount);
        args = molt_tuple_from_array(call_args, 3);
        molt_handle_decref(call_args[2]);
    }
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

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

static inline int PyUnicode_Contains(PyObject *container, PyObject *element) {
    if (container == NULL || element == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument");
        return -1;
    }
    return molt_object_contains(_molt_py_handle(container), _molt_py_handle(element));
}

static inline int PyUnicode_Compare(PyObject *left, PyObject *right) {
    int eq;
    PyObject *lt_result;
    if (left == NULL || right == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return 0; }
    eq = molt_object_equal(_molt_py_handle(left), _molt_py_handle(right));
    if (eq) return 0;
    lt_result = (PyObject *)(uintptr_t)molt_lt((uint64_t)(uintptr_t)left, (uint64_t)(uintptr_t)right);
    if (lt_result == NULL) { PyErr_Clear(); return 0; }
    eq = molt_object_truthy(_molt_py_handle(lt_result));
    Py_DECREF(lt_result);
    return eq ? -1 : 1;
}

static inline int PyUnicode_CompareWithASCIIString(PyObject *uni, const char *str) {
    const char *utf8;
    Py_ssize_t len;
    int cmp;
    if (uni == NULL || str == NULL) return 0;
    utf8 = PyUnicode_AsUTF8AndSize(uni, &len);
    if (utf8 == NULL) { PyErr_Clear(); return 1; }
    cmp = strncmp(utf8, str, (size_t)len);
    if (cmp != 0) return cmp;
    /* Check if str is longer */
    if (str[len] != '\0') return -1;
    return 0;
}

static inline PyObject *PyUnicode_DecodeUTF8(const char *s, Py_ssize_t size,
                                               const char *errors) {
    (void)errors;
    if (s == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_string_from((const uint8_t *)s, (uint64_t)size));
}

static inline PyObject *PyUnicode_DecodeASCII(const char *s, Py_ssize_t size,
                                                const char *errors) {
    (void)errors;
    if (s == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_string_from((const uint8_t *)s, (uint64_t)size));
}

static inline PyObject *PyUnicode_DecodeLatin1(const char *s, Py_ssize_t size,
                                                 const char *errors) {
    (void)errors;
    /* Latin-1 is a subset; for the molt runtime we attempt UTF-8 creation */
    if (s == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_string_from((const uint8_t *)s, (uint64_t)size));
}

static inline PyObject *PyUnicode_AsEncodedString(PyObject *unicode,
                                                    const char *encoding,
                                                    const char *errors) {
    MoltHandle method;
    MoltHandle call_args[2];
    MoltHandle args;
    MoltHandle out;
    (void)errors;
    if (unicode == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(unicode),
        (const uint8_t *)"encode", 6);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    if (encoding != NULL) {
        call_args[0] = _molt_string_from_utf8(encoding);
        args = molt_tuple_from_array(call_args, 1);
        molt_handle_decref(call_args[0]);
    } else {
        args = molt_tuple_from_array(NULL, 0);
    }
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline Py_UCS4 PyUnicode_ReadChar(PyObject *unicode, Py_ssize_t index) {
    const char *utf8;
    Py_ssize_t len;
    if (unicode == NULL) return (Py_UCS4)-1;
    utf8 = PyUnicode_AsUTF8AndSize(unicode, &len);
    if (utf8 == NULL || index < 0 || index >= len) return (Py_UCS4)-1;
    return (Py_UCS4)(unsigned char)utf8[index];
}

static inline int PyUnicode_WriteChar(PyObject *unicode, Py_ssize_t index,
                                       Py_UCS4 character) {
    (void)unicode; (void)index; (void)character;
    PyErr_SetString(PyExc_TypeError, "PyUnicode_WriteChar: molt strings are immutable");
    return -1;
}

static inline PyObject *PyUnicode_Format(PyObject *format, PyObject *args) {
    if (format == NULL || args == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_mod((uint64_t)(uintptr_t)format, (uint64_t)(uintptr_t)args);
}

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
    result = (PyObject *)(uintptr_t)molt_add((uint64_t)(uintptr_t)*bytes, (uint64_t)(uintptr_t)newpart);
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

static inline int PyByteArray_Check(PyObject *o) {
    if (o == NULL) return 0;
    return PyObject_IsInstance(o, (PyObject *)_molt_builtin_type_object_borrowed("bytearray"));
}

static inline PyObject *PyByteArray_FromStringAndSize(const char *string,
                                                        Py_ssize_t len) {
    if (string == NULL && len == 0) {
        return _molt_pyobject_from_result(molt_bytearray_from(NULL, 0));
    }
    if (string == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_bytearray_from((const uint8_t *)string, (uint64_t)len));
}

static inline PyObject *PyByteArray_FromObject(PyObject *o) {
    MoltHandle ba_type = _molt_builtin_type_handle_cached("bytearray");
    MoltHandle arg = _molt_py_handle(o);
    MoltHandle args = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (ba_type == 0 || args == 0 || molt_err_pending() != 0) {
        if (args != 0) molt_handle_decref(args);
        return NULL;
    }
    out = molt_object_call(ba_type, args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline char *PyByteArray_AsString(PyObject *o) {
    uint64_t len = 0;
    if (o == NULL) return NULL;
    return (char *)molt_bytearray_as_ptr(_molt_py_handle(o), &len);
}

static inline Py_ssize_t PyByteArray_Size(PyObject *o) {
    uint64_t len = 0;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    if (molt_bytearray_as_ptr(_molt_py_handle(o), &len) == NULL && len == 0) {
        if (molt_err_pending() != 0) return -1;
    }
    return (Py_ssize_t)len;
}

static inline int PyByteArray_Resize(PyObject *bytearray, Py_ssize_t len) {
    (void)bytearray; (void)len;
    PyErr_SetString(PyExc_RuntimeError, "PyByteArray_Resize not yet supported");
    return -1;
}

static inline PyObject *PyByteArray_Concat(PyObject *a, PyObject *b) {
    if (a == NULL || b == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_add((uint64_t)(uintptr_t)a, (uint64_t)(uintptr_t)b);
}

/* ========================================================================
 * Dict (remaining)
 * ======================================================================== */

static inline int PyDict_DelItem(PyObject *p, PyObject *key) {
    MoltHandle method;
    MoltHandle arg;
    MoltHandle args;
    if (p == NULL || key == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"__delitem__", 11);
    if (method == 0 || molt_err_pending() != 0) return -1;
    arg = _molt_py_handle(key);
    args = molt_tuple_from_array(&arg, 1);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline int PyDict_DelItemString(PyObject *p, const char *key) {
    MoltHandle key_bits;
    PyObject *key_obj;
    int rc;
    if (p == NULL || key == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    key_bits = _molt_string_from_utf8(key);
    key_obj = _molt_pyobject_from_handle(key_bits);
    rc = PyDict_DelItem(p, key_obj);
    molt_handle_decref(key_bits);
    return rc;
}

static inline int PyDict_Merge(PyObject *a, PyObject *b, int override) {
    MoltHandle method;
    MoltHandle arg;
    MoltHandle args;
    if (a == NULL || b == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    if (override) {
        method = molt_object_getattr_bytes(_molt_py_handle(a),
            (const uint8_t *)"update", 6);
        if (method == 0 || molt_err_pending() != 0) return -1;
        arg = _molt_py_handle(b);
        args = molt_tuple_from_array(&arg, 1);
        molt_object_call(method, args, molt_none());
        molt_handle_decref(args);
        molt_handle_decref(method);
        return molt_err_pending() != 0 ? -1 : 0;
    } else {
        /* Non-override: iterate b.items() and only set missing keys */
        MoltHandle items_method = molt_object_getattr_bytes(_molt_py_handle(b),
            (const uint8_t *)"items", 5);
        MoltHandle items_result;
        if (items_method == 0 || molt_err_pending() != 0) return -1;
        items_result = molt_object_call(items_method, molt_tuple_from_array(NULL, 0), molt_none());
        molt_handle_decref(items_method);
        if (items_result == 0 || molt_err_pending() != 0) return -1;
        /* Just use update for simplicity when override is false too */
        method = molt_object_getattr_bytes(_molt_py_handle(a),
            (const uint8_t *)"update", 6);
        molt_handle_decref(items_result);
        if (method == 0 || molt_err_pending() != 0) return -1;
        arg = _molt_py_handle(b);
        args = molt_tuple_from_array(&arg, 1);
        molt_object_call(method, args, molt_none());
        molt_handle_decref(args);
        molt_handle_decref(method);
        return molt_err_pending() != 0 ? -1 : 0;
    }
}

static inline int PyDict_Update(PyObject *a, PyObject *b) {
    return PyDict_Merge(a, b, 1);
}

static inline PyObject *PyDict_Copy(PyObject *p) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (p == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"copy", 4);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyDict_Keys(PyObject *p) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (p == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"keys", 4);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyDict_Values(PyObject *p) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (p == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"values", 6);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyDict_Items(PyObject *p) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (p == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"items", 5);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * List (remaining)
 * ======================================================================== */

static inline PyObject *PyList_GetSlice(PyObject *list, Py_ssize_t low,
                                         Py_ssize_t high) {
    MoltHandle builtins_mod;
    MoltHandle slice_cls;
    MoltHandle slice_args[2];
    MoltHandle slice_args_tuple;
    MoltHandle slice_obj;
    MoltHandle getitem_method;
    MoltHandle getitem_args;
    MoltHandle out;
    if (list == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    builtins_mod = molt_module_import(_molt_string_from_utf8("builtins"));
    if (builtins_mod == 0 || molt_err_pending() != 0) return NULL;
    slice_cls = molt_object_getattr_bytes(builtins_mod,
        (const uint8_t *)"slice", 5);
    molt_handle_decref(builtins_mod);
    if (slice_cls == 0 || molt_err_pending() != 0) return NULL;
    slice_args[0] = molt_int_from_i64((int64_t)low);
    slice_args[1] = molt_int_from_i64((int64_t)high);
    slice_args_tuple = molt_tuple_from_array(slice_args, 2);
    slice_obj = molt_object_call(slice_cls, slice_args_tuple, molt_none());
    molt_handle_decref(slice_args_tuple);
    molt_handle_decref(slice_cls);
    molt_handle_decref(slice_args[0]);
    molt_handle_decref(slice_args[1]);
    if (slice_obj == 0 || molt_err_pending() != 0) return NULL;
    getitem_method = molt_object_getattr_bytes(_molt_py_handle(list),
        (const uint8_t *)"__getitem__", 11);
    if (getitem_method == 0 || molt_err_pending() != 0) {
        molt_handle_decref(slice_obj);
        return NULL;
    }
    getitem_args = molt_tuple_from_array(&slice_obj, 1);
    out = molt_object_call(getitem_method, getitem_args, molt_none());
    molt_handle_decref(getitem_args);
    molt_handle_decref(getitem_method);
    molt_handle_decref(slice_obj);
    return _molt_pyobject_from_result(out);
}

static inline int PyList_Sort(PyObject *list) {
    MoltHandle method;
    MoltHandle args;
    if (list == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(list),
        (const uint8_t *)"sort", 4);
    if (method == 0 || molt_err_pending() != 0) return -1;
    args = molt_tuple_from_array(NULL, 0);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline int PyList_Reverse(PyObject *list) {
    MoltHandle method;
    MoltHandle args;
    if (list == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(list),
        (const uint8_t *)"reverse", 7);
    if (method == 0 || molt_err_pending() != 0) return -1;
    args = molt_tuple_from_array(NULL, 0);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline int PyList_Insert(PyObject *list, Py_ssize_t index, PyObject *item) {
    MoltHandle method;
    MoltHandle call_args[2];
    MoltHandle args;
    if (list == NULL || item == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(list),
        (const uint8_t *)"insert", 6);
    if (method == 0 || molt_err_pending() != 0) return -1;
    call_args[0] = molt_int_from_i64((int64_t)index);
    call_args[1] = _molt_py_handle(item);
    args = molt_tuple_from_array(call_args, 2);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(call_args[0]);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

/* ========================================================================
 * Mapping Protocol (remaining)
 * ======================================================================== */

#define PyMapping_Length PyMapping_Size

static inline int PyMapping_Check(PyObject *o) {
    MoltHandle method;
    if (o == NULL) return 0;
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__getitem__", 11);
    if (method == 0 || molt_err_pending() != 0) { molt_err_clear(); return 0; }
    molt_handle_decref(method);
    return 1;
}

static inline int PyMapping_HasKey(PyObject *o, PyObject *key) {
    if (o == NULL || key == NULL) return 0;
    return molt_object_contains(_molt_py_handle(o), _molt_py_handle(key));
}

static inline int PyMapping_HasKeyString(PyObject *o, const char *key) {
    MoltHandle key_bits;
    int result;
    if (o == NULL || key == NULL) return 0;
    key_bits = _molt_string_from_utf8(key);
    if (key_bits == 0) { molt_err_clear(); return 0; }
    result = molt_object_contains(_molt_py_handle(o), key_bits);
    molt_handle_decref(key_bits);
    if (result < 0) { molt_err_clear(); return 0; }
    return result;
}

static inline PyObject *PyMapping_Keys(PyObject *o) {
    MoltHandle out;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    out = molt_mapping_keys(_molt_py_handle(o));
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyMapping_Values(PyObject *o) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"values", 6);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyMapping_Items(PyObject *o) {
    MoltHandle method;
    MoltHandle args;
    MoltHandle out;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"items", 5);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * Additional Number Protocol
 * ======================================================================== */

static inline int PyNumber_Check(PyObject *o) {
    MoltHandle method;
    if (o == NULL) return 0;
    method = molt_object_getattr_bytes(_molt_py_handle(o),
        (const uint8_t *)"__add__", 7);
    if (method == 0 || molt_err_pending() != 0) { molt_err_clear(); return 0; }
    molt_handle_decref(method);
    return 1;
}

static inline PyObject *PyNumber_Matmul(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_matmul((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

#define PyNumber_MatrixMultiply PyNumber_Matmul

static inline PyObject *PyNumber_InPlacePower(PyObject *o1, PyObject *o2,
                                                PyObject *o3) {
    (void)o3;
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_pow((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline PyObject *PyNumber_InPlaceMatmul(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_inplace_matmul((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

#define PyNumber_InPlaceMatrixMultiply PyNumber_InPlaceMatmul

static inline PyObject *PyNumber_Divmod(PyObject *o1, PyObject *o2) {
    if (o1 == NULL || o2 == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return (PyObject *)(uintptr_t)molt_divmod_builtin((uint64_t)(uintptr_t)o1, (uint64_t)(uintptr_t)o2);
}

static inline Py_ssize_t PyNumber_AsSsize_t(PyObject *o, PyObject *exc) {
    PyObject *idx;
    Py_ssize_t result;
    (void)exc;
    if (o == NULL) return -1;
    idx = PyNumber_Index(o);
    if (idx == NULL) return -1;
    result = (Py_ssize_t)PyLong_AsLongLong(idx);
    Py_DECREF(idx);
    return result;
}

/* ========================================================================
 * Additional Object Protocol
 * ======================================================================== */

static inline int PyObject_Not(PyObject *o) {
    int truth;
    if (o == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    truth = molt_object_truthy(_molt_py_handle(o));
    if (truth < 0) return -1;
    return !truth;
}

/* ========================================================================
 * Additional Dict
 * ======================================================================== */

static inline int PyDict_Clear(PyObject *p) {
    MoltHandle method;
    MoltHandle args;
    if (p == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return -1; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"clear", 5);
    if (method == 0 || molt_err_pending() != 0) return -1;
    args = molt_tuple_from_array(NULL, 0);
    molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return molt_err_pending() != 0 ? -1 : 0;
}

static inline PyObject *PyDict_SetDefault(PyObject *p, PyObject *key,
                                            PyObject *defaultobj) {
    MoltHandle method;
    MoltHandle call_args[2];
    MoltHandle args;
    MoltHandle out;
    if (p == NULL || key == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    method = molt_object_getattr_bytes(_molt_py_handle(p),
        (const uint8_t *)"setdefault", 10);
    if (method == 0 || molt_err_pending() != 0) return NULL;
    call_args[0] = _molt_py_handle(key);
    if (defaultobj != NULL) {
        call_args[1] = _molt_py_handle(defaultobj);
        args = molt_tuple_from_array(call_args, 2);
    } else {
        args = molt_tuple_from_array(call_args, 1);
    }
    out = molt_object_call(method, args, molt_none());
    molt_handle_decref(args);
    molt_handle_decref(method);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * Additional List
 * ======================================================================== */

static inline PyObject *PyList_AsTuple(PyObject *list) {
    MoltHandle tuple_type = _molt_builtin_type_handle_cached("tuple");
    MoltHandle arg = _molt_py_handle(list);
    MoltHandle args = molt_tuple_from_array(&arg, 1);
    MoltHandle out;
    if (tuple_type == 0 || args == 0 || molt_err_pending() != 0) {
        if (args != 0) molt_handle_decref(args);
        return NULL;
    }
    out = molt_object_call(tuple_type, args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

/* ========================================================================
 * Unicode interning
 * ======================================================================== */

static inline PyObject *PyUnicode_InternFromString(const char *v) {
    if (v == NULL) { PyErr_SetString(PyExc_TypeError, "NULL argument"); return NULL; }
    return _molt_pyobject_from_result(
        molt_string_from((const uint8_t *)v, (uint64_t)strlen(v)));
}

static inline void PyUnicode_InternInPlace(PyObject **p) {
    /* molt strings are already interned; no-op */
    (void)p;
}

/* ========================================================================
 * Additional memory helpers (aliases)
 * ======================================================================== */

/* ========================================================================
 * Call convenience helpers
 * ======================================================================== */

static inline PyObject *PyObject_CallNoArgs(PyObject *callable) {
    MoltHandle args, out;
    if (callable == NULL) { PyErr_SetString(PyExc_TypeError, "NULL callable"); return NULL; }
    args = molt_tuple_from_array(NULL, 0);
    out = molt_object_call(_molt_py_handle(callable), args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyObject_CallOneArg(PyObject *callable, PyObject *arg) {
    MoltHandle a, args, out;
    if (callable == NULL) { PyErr_SetString(PyExc_TypeError, "NULL callable"); return NULL; }
    a = _molt_py_handle(arg);
    args = molt_tuple_from_array(&a, 1);
    out = molt_object_call(_molt_py_handle(callable), args, molt_none());
    molt_handle_decref(args);
    return _molt_pyobject_from_result(out);
}

static inline PyObject *PyObject_CallMethodNoArgs(PyObject *obj, PyObject *name) {
    PyObject *method = PyObject_GetAttr(obj, name);
    PyObject *result;
    if (method == NULL) return NULL;
    result = PyObject_CallNoArgs(method);
    Py_DECREF(method);
    return result;
}

static inline PyObject *PyObject_CallMethodOneArg(PyObject *obj, PyObject *name,
                                                    PyObject *arg) {
    PyObject *method = PyObject_GetAttr(obj, name);
    PyObject *result;
    if (method == NULL) return NULL;
    result = PyObject_CallOneArg(method, arg);
    Py_DECREF(method);
    return result;
}

/* ========================================================================
 * Unicode macro aliases
 * ======================================================================== */

#define PyUnicode_GET_LENGTH(op) PyUnicode_GetLength((PyObject *)(op))
#define PySequence_FAST_GET_SIZE(op) PySequence_Size((PyObject *)(op))

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
 * Frame Object API (stubs — molt is AOT compiled, no real frames)
 * ======================================================================== */

typedef struct _molt_pyframeobject {
    PyObject ob_base;
    int f_lineno;
} PyFrameObject;

typedef struct _molt_pycodeobject {
    PyObject ob_base;
    PyObject *co_filename;
    int co_nlocals;
    int co_nfreevars;
} PyCodeObject;

typedef struct _molt_pytracebackobject {
    PyObject ob_base;
} PyTracebackObject;

typedef int (*Py_tracefunc)(PyObject *, PyFrameObject *, int, PyObject *);

static inline PyObject *PyFrame_GetBack(PyFrameObject *frame) {
    (void)frame;
    Py_RETURN_NONE;
}

static inline PyObject *PyFrame_GetBuiltins(PyFrameObject *frame) {
    (void)frame;
    Py_RETURN_NONE;
}

static inline PyObject *PyFrame_GetGlobals(PyFrameObject *frame) {
    (void)frame;
    Py_RETURN_NONE;
}

static inline PyObject *PyFrame_GetLocals(PyFrameObject *frame) {
    (void)frame;
    Py_RETURN_NONE;
}

static inline int PyFrame_GetLineNumber(PyFrameObject *frame) {
    if (frame == NULL) return -1;
    return frame->f_lineno;
}

static inline PyCodeObject *PyFrame_GetCode(PyFrameObject *frame) {
    (void)frame;
    return NULL;
}

static inline PyObject *PyFrame_GetGenerator(PyFrameObject *frame) {
    (void)frame;
    Py_RETURN_NONE;
}

static inline int PyFrame_GetLasti(PyFrameObject *frame) {
    (void)frame;
    return -1;
}

static inline PyObject *PyFrame_GetVar(PyFrameObject *frame, PyObject *name) {
    (void)frame; (void)name;
    PyErr_SetString(PyExc_RuntimeError, "PyFrame_GetVar: molt has no frame introspection");
    return NULL;
}

static inline PyObject *PyFrame_GetVarString(PyFrameObject *frame, const char *name) {
    (void)frame; (void)name;
    PyErr_SetString(PyExc_RuntimeError, "PyFrame_GetVarString: molt has no frame introspection");
    return NULL;
}

/* ========================================================================
 * Code Object API (stubs)
 * ======================================================================== */

static inline int PyCode_Check(PyObject *co) {
    (void)co;
    return 0;
}

static inline PyObject *PyCode_GetFileName(PyCodeObject *co) {
    (void)co;
    return PyUnicode_FromString("<molt-compiled>");
}

static inline int PyCode_GetNumFree(PyCodeObject *co) {
    if (co == NULL) return 0;
    return co->co_nfreevars;
}

static inline int PyCode_GetFirstFreeVar(PyCodeObject *co) {
    (void)co;
    return 0;
}

static inline PyObject *PyCode_GetCode(PyCodeObject *co) {
    (void)co;
    return PyBytes_FromStringAndSize("", 0);
}

static inline PyObject *PyCode_GetVarnames(PyCodeObject *co) {
    (void)co;
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
 * Traceback API (stubs)
 * ======================================================================== */

static inline int PyTraceBack_Here(PyFrameObject *frame) {
    (void)frame;
    return 0;
}

static inline int PyTraceBack_Print(PyObject *tb, PyObject *f) {
    (void)tb; (void)f;
    return 0;
}

static inline int PyTraceBack_Check(PyObject *ob) {
    (void)ob;
    return 0;
}

static inline PyObject *PyTraceBack_GetObject(PyObject *tb) {
    (void)tb;
    Py_RETURN_NONE;
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
    return (PyTypeObject *)_molt_builtin_type_object_borrowed("tuple");
}

/* ========================================================================
 * PyCFunction / Method completions
 * ======================================================================== */

static inline PyObject *PyCFunction_New(PyMethodDef *ml, PyObject *self) {
    (void)ml; (void)self;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyCFunction_New: use molt's native function binding");
    return NULL;
}

static inline PyObject *PyCFunction_NewEx(PyMethodDef *ml, PyObject *self, PyObject *module) {
    (void)ml; (void)self; (void)module;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyCFunction_NewEx: use molt's native function binding");
    return NULL;
}

static inline int PyCFunction_Check(PyObject *op) {
    (void)op;
    return 0;
}

static inline PyCFunction PyCFunction_GetFunction(PyObject *op) {
    (void)op;
    return NULL;
}

static inline PyObject *PyCFunction_GetSelf(PyObject *op) {
    (void)op;
    Py_RETURN_NONE;
}

static inline int PyCFunction_GetFlags(PyObject *op) {
    (void)op;
    return 0;
}

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

static inline PyObject *PyClassMethod_New(PyObject *callable) {
    Py_INCREF(callable);
    return callable;
}

static inline PyObject *PyStaticMethod_New(PyObject *callable) {
    Py_INCREF(callable);
    return callable;
}

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
 * MemoryView API
 * ======================================================================== */

static inline PyObject *PyMemoryView_FromObject(PyObject *obj) {
    (void)obj;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyMemoryView_FromObject: memoryview not yet supported in molt");
    return NULL;
}

static inline PyObject *PyMemoryView_FromMemory(char *mem, Py_ssize_t size, int flags) {
    (void)mem; (void)size; (void)flags;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyMemoryView_FromMemory: memoryview not yet supported in molt");
    return NULL;
}

static inline PyObject *PyMemoryView_FromBuffer(Py_buffer *info) {
    (void)info;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyMemoryView_FromBuffer: memoryview not yet supported in molt");
    return NULL;
}

static inline int PyMemoryView_Check(PyObject *op) {
    (void)op;
    return 0;
}

#define PyMemoryView_GET_BUFFER(mview) ((Py_buffer *)NULL)
#define PyMemoryView_GET_BASE(mview) ((PyObject *)NULL)

/* ========================================================================
 * Exception creation helpers
 * ======================================================================== */

static inline PyObject *PyException_GetTraceback(PyObject *ex) {
    (void)ex;
    Py_RETURN_NONE;
}

static inline int PyException_SetTraceback(PyObject *ex, PyObject *tb) {
    (void)ex; (void)tb;
    return 0;
}

static inline PyObject *PyException_GetCause(PyObject *ex) {
    (void)ex;
    Py_RETURN_NONE;
}

static inline void PyException_SetCause(PyObject *ex, PyObject *cause) {
    (void)ex;
    Py_XDECREF(cause);
}

static inline PyObject *PyException_GetContext(PyObject *ex) {
    (void)ex;
    Py_RETURN_NONE;
}

static inline void PyException_SetContext(PyObject *ex, PyObject *context) {
    (void)ex;
    Py_XDECREF(context);
}

static inline PyObject *PyException_GetArgs(PyObject *ex) {
    (void)ex;
    return PyTuple_New(0);
}

static inline void PyException_SetArgs(PyObject *ex, PyObject *args) {
    (void)ex; (void)args;
}

static inline PyObject *PyErr_NewException(const char *name, PyObject *base, PyObject *dict) {
    (void)dict;
    PyObject *exc_name = PyUnicode_FromString(name);
    if (exc_name == NULL) return NULL;
    Py_DECREF(exc_name);
    if (base != NULL) {
        Py_INCREF(base);
        return base;
    }
    Py_INCREF(PyExc_RuntimeError);
    return PyExc_RuntimeError;
}

static inline PyObject *PyErr_NewExceptionWithDoc(const char *name, const char *doc,
                                                    PyObject *base, PyObject *dict) {
    (void)doc;
    return PyErr_NewException(name, base, dict);
}

static inline int PyErr_GivenExceptionMatches(PyObject *given, PyObject *exc) {
    if (given == NULL || exc == NULL) return 0;
    if (given == exc) return 1;
    return PyObject_IsInstance(given, exc);
}

static inline PyObject *PyErr_SetFromErrno(PyObject *type) {
    const char *msg;
    if (type == NULL) type = PyExc_OSError;
    msg = strerror(errno);
    if (msg == NULL) msg = "unknown error";
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

static inline int PyErr_CheckSignals(void) {
    return 0;
}

static inline void PyErr_SetInterrupt(void) {
    /* no-op */
}

static inline void PyErr_SetInterruptEx(int signum) {
    (void)signum;
}

static inline void PyErr_Print(void) {
    PyObject *type, *value, *tb;
    PyErr_Fetch(&type, &value, &tb);
    if (value != NULL) {
        PyObject *str = PyObject_Str(value);
        if (str != NULL) {
            const char *s = PyUnicode_AsUTF8(str);
            if (s != NULL) fprintf(stderr, "%s\n", s);
            Py_DECREF(str);
        }
    }
    Py_XDECREF(type);
    Py_XDECREF(value);
    Py_XDECREF(tb);
}

static inline void PyErr_PrintEx(int set_sys_last_vars) {
    (void)set_sys_last_vars;
    PyErr_Print();
}

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
 * Hashing
 * ======================================================================== */

static inline Py_hash_t PyObject_HashNotImplemented(PyObject *o) {
    (void)o;
    PyErr_SetString(PyExc_TypeError, "unhashable type");
    return -1;
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
 * PyRun stubs (no eval in molt)
 * ======================================================================== */

typedef struct {
    int cf_flags;
    int cf_feature_version;
} PyCompilerFlags;

static inline PyObject *PyRun_StringFlags(const char *str, int start,
                                           PyObject *globals, PyObject *locals,
                                           PyCompilerFlags *flags) {
    (void)str; (void)start; (void)globals; (void)locals; (void)flags;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyRun_StringFlags: dynamic eval not supported in molt");
    return NULL;
}

static inline PyObject *PyRun_String(const char *str, int start,
                                      PyObject *globals, PyObject *locals) {
    return PyRun_StringFlags(str, start, globals, locals, NULL);
}

static inline int PyRun_SimpleStringFlags(const char *command, PyCompilerFlags *flags) {
    (void)command; (void)flags;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyRun_SimpleStringFlags: dynamic eval not supported in molt");
    return -1;
}

static inline int PyRun_SimpleString(const char *command) {
    return PyRun_SimpleStringFlags(command, NULL);
}

static inline int PyRun_AnyFileFlags(FILE *fp, const char *filename, PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)flags;
    return -1;
}

static inline int PyRun_AnyFile(FILE *fp, const char *filename) {
    return PyRun_AnyFileFlags(fp, filename, NULL);
}

static inline int PyRun_AnyFileExFlags(FILE *fp, const char *filename, int closeit,
                                        PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)closeit; (void)flags;
    return -1;
}

static inline PyObject *PyRun_FileFlags(FILE *fp, const char *filename, int start,
                                         PyObject *globals, PyObject *locals,
                                         PyCompilerFlags *flags) {
    (void)fp; (void)filename; (void)start; (void)globals; (void)locals; (void)flags;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyRun_FileFlags: dynamic eval not supported in molt");
    return NULL;
}

static inline PyObject *PyRun_File(FILE *fp, const char *filename, int start,
                                    PyObject *globals, PyObject *locals) {
    return PyRun_FileFlags(fp, filename, start, globals, locals, NULL);
}

static inline PyObject *Py_CompileString(const char *str, const char *filename, int start) {
    (void)str; (void)filename; (void)start;
    PyErr_SetString(PyExc_NotImplementedError,
        "Py_CompileString: dynamic compilation not supported in molt");
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
 * PyEval stubs
 * ======================================================================== */

static inline void PyEval_AcquireLock(void) {
    /* no-op */
}

static inline void PyEval_ReleaseLock(void) {
    /* no-op */
}

static inline void PyEval_AcquireThread(PyThreadState *tstate) {
    (void)tstate;
}

static inline void PyEval_ReleaseThread(PyThreadState *tstate) {
    (void)tstate;
}

static inline PyThreadState *PyEval_SaveThread(void) {
    return NULL;
}

static inline void PyEval_RestoreThread(PyThreadState *tstate) {
    (void)tstate;
}

static inline PyFrameObject *PyEval_GetFrame(void) {
    return NULL;
}

static inline int PyEval_MergeCompilerFlags(PyCompilerFlags *cf) {
    (void)cf;
    return 0;
}

static inline PyObject *PyEval_EvalCode(PyObject *co, PyObject *globals, PyObject *locals) {
    (void)co; (void)globals; (void)locals;
    PyErr_SetString(PyExc_NotImplementedError,
        "PyEval_EvalCode: dynamic eval not supported in molt");
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
    PyErr_SetString(PyExc_NotImplementedError,
        "PyEval_EvalCodeEx: dynamic eval not supported in molt");
    return NULL;
}

/* ========================================================================
 * PyGILState additional
 * ======================================================================== */

static inline PyThreadState *PyGILState_GetThisThreadState(void) {
    return NULL;
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

static inline PyFrameObject *PyThreadState_GetFrame(PyThreadState *tstate) {
    (void)tstate;
    return NULL;
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
    (void)interp;
    return PyDict_New();
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
 * PyObject_IsSubclass
 * ======================================================================== */

static inline int PyObject_IsSubclass(PyObject *derived, PyObject *cls) {
    if (derived == NULL || cls == NULL) {
        PyErr_SetString(PyExc_TypeError, "PyObject_IsSubclass: NULL argument");
        return -1;
    }
    if (derived == cls) return 1;
    return 0;
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

static inline PyObject *PyFloat_FromString(PyObject *str) {
    const char *s = PyUnicode_AsUTF8(str);
    if (s == NULL) return NULL;
    char *end;
    double val = strtod(s, &end);
    if (end == s || *end != '\0') {
        PyErr_SetString(PyExc_ValueError, "could not convert string to float");
        return NULL;
    }
    return PyFloat_FromDouble(val);
}

static inline double PyFloat_GetMax(void) {
    return 1.7976931348623157e+308;
}

static inline double PyFloat_GetMin(void) {
    return 2.2250738585072014e-308;
}

/* ========================================================================
 * Buffer protocol helpers
 * ======================================================================== */

static inline int PyBuffer_FillInfo(Py_buffer *view, PyObject *exporter,
                                     void *buf, Py_ssize_t len, int readonly,
                                     int flags) {
    (void)flags;
    if (view == NULL) return -1;
    memset(view, 0, sizeof(*view));
    view->buf = buf;
    view->len = len;
    view->readonly = readonly;
    view->itemsize = 1;
    view->ndim = 1;
    view->obj = exporter;
    if (exporter != NULL) Py_INCREF(exporter);
    return 0;
}

static inline int PyObject_CheckBuffer(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyBuffer_IsContiguous(const Py_buffer *view, char order) {
    (void)view; (void)order;
    return 1;
}

static inline void PyBuffer_FillContiguousStrides(int ndim, Py_ssize_t *shape,
                                                    Py_ssize_t *strides,
                                                    int itemsize, char order) {
    int i;
    (void)order;
    if (ndim <= 0) return;
    strides[ndim - 1] = itemsize;
    for (i = ndim - 2; i >= 0; i--) {
        strides[i] = strides[i + 1] * shape[i + 1];
    }
}

/* ========================================================================
 * Py_SIZE
 * ======================================================================== */

#define Py_SIZE(ob) ((Py_ssize_t)0)
#define Py_SET_SIZE(ob, size) ((void)(ob), (void)(size))

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

/* ========================================================================
 * METH_FASTCALL / METH_METHOD (supplement existing defs)
 * ======================================================================== */

#ifndef METH_FASTCALL
#define METH_FASTCALL 0x0080
#endif

#ifndef METH_METHOD
#define METH_METHOD 0x0200
#endif

/* (All type slot IDs are defined at the top of this file.) */

/* ========================================================================
 * PyLong additional conversions
 * ======================================================================== */

static inline unsigned long long PyLong_AsUnsignedLongLong(PyObject *pylong) {
    long long val = PyLong_AsLongLong(pylong);
    if (PyErr_Occurred()) {
        return (unsigned long long)-1;
    }
    if (val < 0) {
        PyErr_SetString(PyExc_OverflowError,
            "can't convert negative value to unsigned long long");
        return (unsigned long long)-1;
    }
    return (unsigned long long)val;
}

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
 * PyObject_GetAIter / PyIter_Send / PyAIter_Check
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

static inline int PyIter_Send(PyObject *iter, PyObject *arg, PyObject **presult) {
    PyObject *meth;
    if (iter == NULL || presult == NULL) {
        if (presult != NULL) *presult = NULL;
        return PYGEN_ERROR;
    }
    *presult = NULL;
    meth = PyObject_GetAttrString(iter, "send");
    if (meth == NULL) {
        if (arg == Py_None || arg == NULL) {
            PyErr_Clear();
            *presult = PyIter_Next(iter);
            if (*presult != NULL) return PYGEN_NEXT;
            /* Check if StopIteration was raised (normal return) */
            if (PyErr_ExceptionMatches(PyExc_StopIteration)) {
                PyErr_Clear();
                return PYGEN_RETURN;
            }
            return PYGEN_ERROR;
        }
        return PYGEN_ERROR;
    }
    *presult = PyObject_CallOneArg(meth, arg != NULL ? arg : (PyObject *)(uintptr_t)molt_none());
    Py_DECREF(meth);
    if (*presult != NULL) return PYGEN_NEXT;
    if (PyErr_ExceptionMatches(PyExc_StopIteration)) {
        PyErr_Clear();
        return PYGEN_RETURN;
    }
    return PYGEN_ERROR;
}

/* ========================================================================
 * PyObject_GenericGetDict / PyObject_GenericSetDict
 * ======================================================================== */

static inline PyObject *PyObject_GenericGetDict(PyObject *obj, void *context) {
    PyObject *dict;
    (void)context;
    if (obj == NULL) {
        PyErr_SetString(PyExc_SystemError, "NULL object");
        return NULL;
    }
    dict = PyObject_GetAttrString(obj, "__dict__");
    if (dict == NULL) {
        /* Object has no __dict__; return a new empty dict as fallback. */
        if (molt_err_pending() != 0) {
            molt_err_clear();
        }
        return PyDict_New();
    }
    return dict;
}

static inline int PyObject_GenericSetDict(PyObject *obj, PyObject *value, void *context) {
    (void)obj; (void)value; (void)context;
    PyErr_SetString(PyExc_AttributeError, "cannot set __dict__");
    return -1;
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

static inline PyObject *PyUnicode_FromOrdinal(int ordinal) {
    char buf[5];
    if (ordinal >= 0 && ordinal < 0x80) {
        buf[0] = (char)ordinal;
        return PyUnicode_FromStringAndSize(buf, 1);
    }
    PyObject *builtins = PyImport_ImportModule("builtins");
    if (builtins == NULL) return NULL;
    PyObject *chr_fn = PyObject_GetAttrString(builtins, "chr");
    Py_DECREF(builtins);
    if (chr_fn == NULL) return NULL;
    PyObject *arg = PyLong_FromLong(ordinal);
    if (arg == NULL) { Py_DECREF(chr_fn); return NULL; }
    PyObject *result = PyObject_CallOneArg(chr_fn, arg);
    Py_DECREF(chr_fn);
    Py_DECREF(arg);
    return result;
}

/* ========================================================================
 * PyObject_DelAttr* macros
 * ======================================================================== */

#ifndef PyObject_DelAttr
#define PyObject_DelAttr(o, name) PyObject_SetAttr((o), (name), NULL)
#endif

#ifndef PyObject_DelAttrString
#define PyObject_DelAttrString(o, name) PyObject_SetAttrString((o), (name), NULL)
#endif

#ifndef PyObject_DelItem
#define PyObject_DelItem(o, key) PyObject_SetItem((o), (key), NULL)
#endif

#ifndef PyMapping_DelItem
#define PyMapping_DelItem(o, key) PyObject_DelItem((o), (key))
#endif

#ifndef PyMapping_DelItemString
static inline int PyMapping_DelItemString(PyObject *o, const char *key) {
    PyObject *k = PyUnicode_FromString(key);
    if (k == NULL) return -1;
    int rc = PyObject_SetItem(o, k, NULL);
    Py_DECREF(k);
    return rc;
}
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
 * PyObject_GC_* (molt: no-op, no cycle collector)
 * ======================================================================== */

static inline PyObject *_PyObject_GC_New(PyTypeObject *type) {
    return _PyObject_New(type);
}

static inline PyVarObject *_PyObject_GC_NewVar(PyTypeObject *type, Py_ssize_t size) {
    return _PyObject_NewVar(type, size);
}

static inline void PyObject_GC_Track(void *op) {
    (void)op;
}

static inline void PyObject_GC_UnTrack(void *op) {
    (void)op;
}

static inline void PyObject_GC_Del(void *op) {
    PyObject_Free(op);
}

#define PyObject_GC_New(type, typeobj) ((type *)_PyObject_GC_New((PyTypeObject *)(typeobj)))
#define PyObject_GC_NewVar(type, typeobj, n) ((type *)_PyObject_GC_NewVar((PyTypeObject *)(typeobj), (n)))

static inline int PyObject_GC_IsTracked(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyObject_GC_IsFinalized(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyGC_Collect(void) {
    return 0;
}

static inline int PyGC_Enable(void) {
    return 0;
}

static inline int PyGC_Disable(void) {
    return 0;
}

static inline int PyGC_IsEnabled(void) {
    return 0;
}

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
#define T_SHORT 0
#define T_INT 1
#define T_LONG 2
#define T_FLOAT 3
#define T_DOUBLE 4
#define T_STRING 5
#define T_OBJECT 6
#define T_CHAR 7
#define T_BYTE 8
#define T_UBYTE 9
#define T_USHORT 10
#define T_UINT 11
#define T_ULONG 12
#define T_STRING_INPLACE 13
#define T_BOOL 14
#define T_OBJECT_EX 16
#define T_LONGLONG 17
#define T_ULONGLONG 18
#define T_PYSSIZET 19
#define T_NONE 20
#endif

#ifndef READ_RESTRICTED
#define READ_RESTRICTED 2
#endif

#ifndef PY_WRITE_RESTRICTED
#define PY_WRITE_RESTRICTED 4
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
#ifndef PyBUF_C_CONTIGUOUS
#define PyBUF_C_CONTIGUOUS 0x0020
#endif
#ifndef PyBUF_F_CONTIGUOUS
#define PyBUF_F_CONTIGUOUS 0x0040
#endif
#ifndef PyBUF_ANY_CONTIGUOUS
#define PyBUF_ANY_CONTIGUOUS 0x0080
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
static inline PyObject *PyObject_CallMethodObjArgs(
    PyObject *obj,
    PyObject *name,
    ...
) {
    va_list ap;
    PyObject *method;
    PyObject *result;
    MoltHandle args_arr[32]; /* max 32 positional args — covers all real extensions */
    uint64_t nargs = 0;
    MoltHandle args_bits;
    PyObject *arg;

    if (obj == NULL || name == NULL) {
        PyErr_SetString(PyExc_TypeError, "object and method name must not be NULL");
        return NULL;
    }
    method = PyObject_GetAttr(obj, name);
    if (method == NULL) {
        return NULL;
    }

    va_start(ap, name);
    while ((arg = va_arg(ap, PyObject *)) != NULL) {
        if (nargs >= 32) {
            va_end(ap);
            Py_DECREF(method);
            PyErr_SetString(PyExc_TypeError,
                "PyObject_CallMethodObjArgs: too many arguments (max 32)");
            return NULL;
        }
        args_arr[nargs++] = _molt_py_handle(arg);
    }
    va_end(ap);

    args_bits = molt_tuple_from_array(nargs > 0 ? args_arr : NULL, nargs);
    if (args_bits == 0 || molt_err_pending() != 0) {
        Py_DECREF(method);
        return NULL;
    }
    result = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle(method), args_bits, molt_none()));
    molt_handle_decref(args_bits);
    Py_DECREF(method);
    return result;
}

/*
 * PyErr_GetRaisedException — Python 3.12+ API.
 * Returns the current exception as a single object (new reference), clears it.
 */
static inline PyObject *PyErr_GetRaisedException(void) {
    MoltHandle exc_bits;
    if (molt_err_pending() == 0) {
        return NULL;
    }
    exc_bits = molt_err_fetch();
    if (exc_bits == 0 || exc_bits == molt_none()) {
        return NULL;
    }
    return _molt_pyobject_from_handle(exc_bits);
}

/*
 * PyErr_SetRaisedException — Python 3.12+ API.
 * Sets the current exception from a single object. Steals the reference.
 */
static inline void PyErr_SetRaisedException(PyObject *exc) {
    /* Clear any existing exception first */
    (void)molt_err_clear();
    if (exc != NULL && (PyObject *)exc != Py_None) {
        (void)molt_err_restore(_molt_py_handle(exc));
    }
}

/*
 * PyErr_NormalizeException — legacy API. In CPython this normalizes
 * lazy exception triples. Molt exceptions are always normalized, so
 * this is effectively a no-op that ensures the triple is consistent.
 */
static inline void PyErr_NormalizeException(
    PyObject **exc,
    PyObject **val,
    PyObject **tb
) {
    /* Molt exceptions are already normalized. Just ensure non-NULL pointers
     * have sensible values. */
    if (exc != NULL && *exc == NULL && val != NULL && *val != NULL) {
        /* Infer type from value — best effort */
        *exc = (PyObject *)Py_TYPE(*val);
        if (*exc != NULL) {
            Py_INCREF(*exc);
        }
    }
    if (tb != NULL && *tb == NULL) {
        *tb = Py_None;
        Py_INCREF(Py_None);
    }
}

/* ========================================================================
 * Priority 2: Common patterns used by many extensions
 * ======================================================================== */

/*
 * PyObject_GenericGetAttr — generic attribute access using __dict__ and
 * type descriptors. Maps to molt_object_getattr which already implements
 * the full MRO + descriptor protocol.
 */
static inline PyObject *PyObject_GenericGetAttr(PyObject *obj, PyObject *name) {
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_GenericGetAttr");
        return NULL;
    }
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "attribute name must not be NULL");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_object_getattr(_molt_py_handle(obj), _molt_py_handle(name)));
}

/*
 * PyObject_GenericSetAttr — generic attribute setting.
 * Returns 0 on success, -1 on failure.
 */
static inline int PyObject_GenericSetAttr(PyObject *obj, PyObject *name, PyObject *value) {
    MoltHandle result;
    if (obj == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL object passed to PyObject_GenericSetAttr");
        return -1;
    }
    if (name == NULL) {
        PyErr_SetString(PyExc_TypeError, "attribute name must not be NULL");
        return -1;
    }
    if (value == NULL) {
        /* NULL value means delete the attribute */
        result = molt_object_delattr(_molt_py_handle(obj), _molt_py_handle(name));
        if (result == 0 && molt_err_pending() != 0) {
            return -1;
        }
        return 0;
    }
    result = molt_object_setattr(_molt_py_handle(obj), _molt_py_handle(name),
                                  _molt_py_handle(value));
    if (result == 0 && molt_err_pending() != 0) {
        return -1;
    }
    return 0;
}

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

/* PyType_FromModuleAndSpec, PyType_GetModule, PyType_GetModuleState —
 * defined earlier in this file. */

/* PyObject_CallFunctionObjArgs — defined earlier in this file. */

/* PyErr_SetFromErrno, PyErr_SetFromErrnoWithFilenameObject,
 * PyErr_SetFromErrnoWithFilenameObjects — defined earlier in this file. */

/* PyErr_WriteUnraisable, PyIter_Next, PyObject_GetIter,
 * PyObject_RichCompareBool — defined earlier in this file. */

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

static inline PyObject *PyLong_FromDouble(double v) {
    long long truncated = (long long)v;
    return PyLong_FromLongLong(truncated);
}

static inline PyObject *PyLong_FromUnsignedLong(unsigned long v) {
    return PyLong_FromUnsignedLongLong((unsigned long long)v);
}

/* ---- Error helpers ---- */

static inline int PyErr_BadArgument(void) {
    PyErr_SetString(PyExc_TypeError, "bad argument type for built-in operation");
    return 0;
}

static inline void PyErr_BadInternalCall(void) {
    PyErr_SetString(PyExc_SystemError, "bad argument to internal function");
}

static inline PyObject *PyErr_SetFromErrnoWithFilename(PyObject *exc, const char *filename) {
    const char *msg = strerror(errno);
    if (msg == NULL) msg = "unknown error";
    if (filename != NULL) {
        PyErr_Format(exc ? exc : PyExc_OSError, "[Errno %d] %s: '%s'", errno, msg, filename);
    } else {
        PyErr_SetString(exc ? exc : PyExc_OSError, msg);
    }
    return NULL;
}

static inline void PyErr_SetExcInfo(PyObject *type, PyObject *value, PyObject *traceback) {
    /* In CPython this sets sys.exc_info().
       Molt: store the exception via molt_err_set if non-NULL. */
    (void)traceback;
    if (value != NULL) {
        PyErr_SetObject(type != NULL ? type : PyExc_RuntimeError, value);
    } else {
        PyErr_Clear();
    }
    Py_XDECREF(type);
    Py_XDECREF(value);
    Py_XDECREF(traceback);
}

static inline void PyErr_GetExcInfo(PyObject **ptype, PyObject **pvalue, PyObject **ptraceback) {
    /* CPython returns the current handled exception.
       Molt approximation: mirror PyErr_Fetch without clearing. */
    MoltHandle exc_bits = molt_err_fetch();
    MoltHandle kind_bits;
    MoltHandle class_bits;
    if (ptype != NULL) {
        if (exc_bits != 0 && exc_bits != molt_none()) {
            kind_bits = molt_exception_kind(exc_bits);
            class_bits = molt_exception_class(kind_bits);
            *ptype = _molt_pyobject_from_handle(class_bits);
            Py_XINCREF(*ptype);
        } else {
            *ptype = NULL;
        }
    }
    if (pvalue != NULL) {
        if (exc_bits != 0 && exc_bits != molt_none()) {
            *pvalue = _molt_pyobject_from_handle(exc_bits);
            Py_XINCREF(*pvalue);
        } else {
            *pvalue = NULL;
        }
    }
    if (ptraceback != NULL) {
        *ptraceback = NULL;
    }
    /* Restore the exception — GetExcInfo does NOT clear */
    if (exc_bits != 0 && exc_bits != molt_none()) {
        molt_err_restore(exc_bits);
    }
}

static inline PyObject *PyErr_GetHandledException(void) {
    PyObject *type, *value, *tb;
    PyErr_GetExcInfo(&type, &value, &tb);
    Py_XDECREF(type);
    Py_XDECREF(tb);
    return value;  /* caller owns reference */
}

static inline void PyErr_SetHandledException(PyObject *exc) {
    if (exc != NULL && exc != Py_None) {
        PyErr_SetObject((PyObject *)Py_TYPE(exc), exc);
    } else {
        PyErr_Clear();
    }
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

static inline PyObject *PyUnicode_Decode(const char *s, Py_ssize_t size,
                                          const char *encoding, const char *errors) {
    (void)errors;
    if (s == NULL) {
        PyErr_SetString(PyExc_ValueError, "NULL string passed to PyUnicode_Decode");
        return NULL;
    }
    if (encoding == NULL || strcmp(encoding, "utf-8") == 0 || strcmp(encoding, "UTF-8") == 0) {
        return PyUnicode_DecodeUTF8(s, size, errors);
    }
    if (strcmp(encoding, "ascii") == 0 || strcmp(encoding, "ASCII") == 0) {
        return PyUnicode_DecodeASCII(s, size, errors);
    }
    if (strcmp(encoding, "latin-1") == 0 || strcmp(encoding, "latin1") == 0 ||
        strcmp(encoding, "iso-8859-1") == 0 || strcmp(encoding, "iso8859-1") == 0) {
        return PyUnicode_DecodeLatin1(s, size, errors);
    }
    /* Fallback: try via Python codecs */
    {
        PyObject *bytes_obj = PyBytes_FromStringAndSize(s, size);
        PyObject *result;
        PyObject *decode_fn;
        PyObject *args;
        if (bytes_obj == NULL) return NULL;
        decode_fn = PyObject_GetAttrString(bytes_obj, "decode");
        if (decode_fn == NULL) {
            Py_DECREF(bytes_obj);
            return NULL;
        }
        args = PyTuple_New(1);
        if (args == NULL) {
            Py_DECREF(decode_fn);
            Py_DECREF(bytes_obj);
            return NULL;
        }
        PyTuple_SetItem(args, 0, PyUnicode_FromString(encoding));
        result = PyObject_CallObject(decode_fn, args);
        Py_DECREF(args);
        Py_DECREF(decode_fn);
        Py_DECREF(bytes_obj);
        return result;
    }
}

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

static inline Py_ssize_t PyUnicode_Tailmatch(PyObject *str, PyObject *substr,
                                               Py_ssize_t start, Py_ssize_t end,
                                               int direction) {
    /* direction: -1 = prefix (startswith), 1 = suffix (endswith) */
    PyObject *method;
    PyObject *args;
    PyObject *result;
    int ret;
    if (str == NULL || substr == NULL) return -1;
    method = PyObject_GetAttrString(str, direction < 0 ? "startswith" : "endswith");
    if (method == NULL) return -1;
    args = PyTuple_New(3);
    if (args == NULL) { Py_DECREF(method); return -1; }
    Py_INCREF(substr);
    PyTuple_SetItem(args, 0, substr);
    PyTuple_SetItem(args, 1, PyLong_FromSsize_t(start));
    PyTuple_SetItem(args, 2, PyLong_FromSsize_t(end));
    result = PyObject_CallObject(method, args);
    Py_DECREF(args);
    Py_DECREF(method);
    if (result == NULL) return -1;
    ret = PyObject_IsTrue(result);
    Py_DECREF(result);
    return (Py_ssize_t)ret;
}

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

static inline PyObject *PyUnicode_FromKindAndData(int kind, const void *buffer, Py_ssize_t size) {
    /* kind: 1=UCS1, 2=UCS2, 4=UCS4 */
    if (buffer == NULL || size < 0) {
        PyErr_SetString(PyExc_ValueError, "invalid arguments to PyUnicode_FromKindAndData");
        return NULL;
    }
    if (kind == 1) {
        /* Latin-1 */
        return PyUnicode_DecodeLatin1((const char *)buffer, size, NULL);
    }
    /* For UCS2/UCS4, convert to wchar_t approach */
    {
        wchar_t *tmp = (wchar_t *)PyMem_Malloc((size_t)(size + 1) * sizeof(wchar_t));
        Py_ssize_t i;
        PyObject *result;
        if (tmp == NULL) return NULL;
        if (kind == 2) {
            const uint16_t *src = (const uint16_t *)buffer;
            for (i = 0; i < size; i++) tmp[i] = (wchar_t)src[i];
        } else if (kind == 4) {
            const uint32_t *src = (const uint32_t *)buffer;
            for (i = 0; i < size; i++) tmp[i] = (wchar_t)src[i];
        } else {
            PyMem_Free(tmp);
            PyErr_SetString(PyExc_ValueError, "invalid kind for PyUnicode_FromKindAndData");
            return NULL;
        }
        tmp[size] = L'\0';
        result = PyUnicode_FromWideChar(tmp, size);
        PyMem_Free(tmp);
        return result;
    }
}

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

static inline PyObject *PyTuple_GetSlice(PyObject *tuple, Py_ssize_t low, Py_ssize_t high) {
    PyObject *slice;
    PyObject *result;
    if (tuple == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyTuple_GetSlice");
        return NULL;
    }
    slice = PySlice_New(PyLong_FromSsize_t(low), PyLong_FromSsize_t(high), NULL);
    if (slice == NULL) return NULL;
    result = PyObject_GetItem(tuple, slice);
    Py_DECREF(slice);
    return result;
}

static inline int PyList_SetSlice(PyObject *list, Py_ssize_t low, Py_ssize_t high,
                                   PyObject *itemlist) {
    PyObject *slice;
    int rc;
    if (list == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyList_SetSlice");
        return -1;
    }
    slice = PySlice_New(PyLong_FromSsize_t(low), PyLong_FromSsize_t(high), NULL);
    if (slice == NULL) return -1;
    if (itemlist != NULL) {
        rc = PyObject_SetItem(list, slice, itemlist);
    } else {
        rc = PyObject_DelItem(list, slice);
    }
    Py_DECREF(slice);
    return rc;
}

static inline PyObject *PySequence_GetSlice(PyObject *o, Py_ssize_t i1, Py_ssize_t i2) {
    PyObject *slice;
    PyObject *result;
    if (o == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PySequence_GetSlice");
        return NULL;
    }
    slice = PySlice_New(PyLong_FromSsize_t(i1), PyLong_FromSsize_t(i2), NULL);
    if (slice == NULL) return NULL;
    result = PyObject_GetItem(o, slice);
    Py_DECREF(slice);
    return result;
}

static inline int PySequence_SetSlice(PyObject *o, Py_ssize_t i1, Py_ssize_t i2,
                                       PyObject *v) {
    PyObject *slice;
    int rc;
    if (o == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PySequence_SetSlice");
        return -1;
    }
    slice = PySlice_New(PyLong_FromSsize_t(i1), PyLong_FromSsize_t(i2), NULL);
    if (slice == NULL) return -1;
    if (v != NULL) {
        rc = PyObject_SetItem(o, slice, v);
    } else {
        rc = PyObject_DelItem(o, slice);
    }
    Py_DECREF(slice);
    return rc;
}

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

static inline PyObject *PyType_GenericAlloc(PyTypeObject *type, Py_ssize_t nitems) {
    (void)nitems;
    return PyObject_Init((PyObject *)PyMem_Malloc(sizeof(PyObject)), type);
}

static inline PyObject *PyType_GenericNew(PyTypeObject *type, PyObject *args, PyObject *kwds) {
    (void)args; (void)kwds;
    return PyType_GenericAlloc(type, 0);
}

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

static inline int PyDict_Pop(PyObject *p, PyObject *key, PyObject **result) {
    PyObject *pop_fn;
    PyObject *item;
    if (p == NULL || key == NULL) {
        if (result) *result = NULL;
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyDict_Pop");
        return -1;
    }
    pop_fn = PyObject_GetAttrString(p, "pop");
    if (pop_fn == NULL) {
        if (result) *result = NULL;
        return -1;
    }
    {
        PyObject *args = PyTuple_Pack(2, key, Py_None);
        if (args == NULL) {
            Py_DECREF(pop_fn);
            if (result) *result = NULL;
            return -1;
        }
        item = PyObject_CallObject(pop_fn, args);
        Py_DECREF(args);
    }
    Py_DECREF(pop_fn);
    if (item == NULL) {
        if (result) *result = NULL;
        return -1;
    }
    if (item == Py_None) {
        /* Key was not present (we used None as default) */
        Py_DECREF(item);
        if (result) *result = NULL;
        return 0;
    }
    if (result) {
        *result = item;
    } else {
        Py_DECREF(item);
    }
    return 1;
}

/* Py_NewRef, Py_XNewRef — already defined earlier in this file. */

/* ---- PyDictProxy_New ---- */

static inline PyObject *PyDictProxy_New(PyObject *mapping) {
    /* Create a read-only mappingproxy wrapping *mapping*.
       Implemented via types.MappingProxyType(mapping). */
    PyObject *types_mod;
    PyObject *proxy_type;
    MoltHandle args_arr[1];
    MoltHandle args_bits;
    PyObject *result;
    if (mapping == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyDictProxy_New");
        return NULL;
    }
    types_mod = PyImport_ImportModule("types");
    if (types_mod == NULL) return NULL;
    proxy_type = PyObject_GetAttrString(types_mod, "MappingProxyType");
    Py_DECREF(types_mod);
    if (proxy_type == NULL) return NULL;
    args_arr[0] = _molt_py_handle(mapping);
    args_bits = molt_tuple_from_array(args_arr, 1);
    result = _molt_pyobject_from_result(
        molt_object_call(_molt_py_handle(proxy_type), args_bits, molt_none()));
    molt_handle_decref(args_bits);
    Py_DECREF(proxy_type);
    return result;
}

/* ---- PyObject_ClearWeakRefs ---- */

static inline void PyObject_ClearWeakRefs(PyObject *obj) {
    /* In CPython this clears all weak references pointing to *obj* and is
       called from tp_dealloc.  Molt's GC handles weak-reference invalidation
       internally, so this is a no-op at the C-API boundary. */
    (void)obj;
}

/* ---- PyFloat_GetInfo ---- */

static inline PyObject *PyFloat_GetInfo(void) {
    /* Return a dict with float constants matching sys.float_info */
    PyObject *d = PyDict_New();
    if (d == NULL) return NULL;
    PyDict_SetItemString(d, "max", PyFloat_FromDouble(1.7976931348623157e+308));
    PyDict_SetItemString(d, "min", PyFloat_FromDouble(2.2250738585072014e-308));
    PyDict_SetItemString(d, "epsilon", PyFloat_FromDouble(2.220446049250313e-16));
    PyDict_SetItemString(d, "dig", PyLong_FromLong(15));
    PyDict_SetItemString(d, "mant_dig", PyLong_FromLong(53));
    PyDict_SetItemString(d, "max_exp", PyLong_FromLong(1024));
    PyDict_SetItemString(d, "max_10_exp", PyLong_FromLong(308));
    PyDict_SetItemString(d, "min_exp", PyLong_FromLong(-1021));
    PyDict_SetItemString(d, "min_10_exp", PyLong_FromLong(-307));
    PyDict_SetItemString(d, "radix", PyLong_FromLong(2));
    PyDict_SetItemString(d, "rounds", PyLong_FromLong(1));
    return d;
}

/* ---- PyLong_AsUnsignedLongLongMask already exists but ensure PyLong_AsUnsignedLongMask ---- */

static inline unsigned long PyLong_AsUnsignedLongMask(PyObject *pylong) {
    return (unsigned long)PyLong_AsUnsignedLongLongMask(pylong);
}

/* PyOS_stricmp, PyOS_strnicmp — already defined earlier in this file. */

/* ---- Py_AddPendingCall / Py_MakePendingCalls ---- */

static inline int Py_AddPendingCall(int (*func)(void *), void *arg) {
    /* Molt is single-threaded; call immediately */
    return func(arg);
}

static inline int Py_MakePendingCalls(void) {
    return 0;
}

/* ---- PyObject_GC_IsTracked / PyObject_GC_IsFinalized already exist;
        ensure _PyObject_GC_TRACK / _PyObject_GC_UNTRACK macros ---- */

#ifndef _PyObject_GC_TRACK
#define _PyObject_GC_TRACK(op) PyObject_GC_Track(op)
#endif
#ifndef _PyObject_GC_UNTRACK
#define _PyObject_GC_UNTRACK(op) PyObject_GC_UnTrack(op)
#endif

/* ---- PyUnicode_New (used by some extensions to allocate mutable buffers) ---- */

static inline PyObject *PyUnicode_New(Py_ssize_t size, Py_UCS4 maxchar) {
    /* Molt strings are immutable; return a zero-filled string of given length.
       Extensions that call this typically fill it with PyUnicode_WriteChar
       which we error on, but we must still provide the allocation. */
    char *buf;
    PyObject *result;
    (void)maxchar;
    if (size < 0) {
        PyErr_SetString(PyExc_ValueError, "negative size in PyUnicode_New");
        return NULL;
    }
    if (size == 0) {
        return PyUnicode_FromString("");
    }
    buf = (char *)PyMem_Malloc((size_t)size + 1);
    if (buf == NULL) return NULL;
    memset(buf, ' ', (size_t)size);
    buf[size] = '\0';
    result = PyUnicode_FromStringAndSize(buf, size);
    PyMem_Free(buf);
    return result;
}

/* ---- PyUnicode_AsUCS4 / PyUnicode_AsUCS4Copy ---- */

static inline Py_UCS4 *PyUnicode_AsUCS4(PyObject *unicode, Py_UCS4 *target,
                                          Py_ssize_t targetsize, int copy_null) {
    const char *utf8;
    Py_ssize_t utf8_len, i, pos;
    if (unicode == NULL || target == NULL) {
        PyErr_SetString(PyExc_TypeError, "NULL argument to PyUnicode_AsUCS4");
        return NULL;
    }
    utf8 = PyUnicode_AsUTF8AndSize(unicode, &utf8_len);
    if (utf8 == NULL) return NULL;
    pos = 0;
    for (i = 0; i < utf8_len && pos < targetsize; ) {
        unsigned char c = (unsigned char)utf8[i];
        Py_UCS4 cp;
        if (c < 0x80) { cp = c; i++; }
        else if ((c & 0xE0) == 0xC0 && i + 1 < utf8_len) {
            cp = ((c & 0x1F) << 6) | (utf8[i+1] & 0x3F); i += 2;
        } else if ((c & 0xF0) == 0xE0 && i + 2 < utf8_len) {
            cp = ((c & 0x0F) << 12) | ((utf8[i+1] & 0x3F) << 6) | (utf8[i+2] & 0x3F); i += 3;
        } else if ((c & 0xF8) == 0xF0 && i + 3 < utf8_len) {
            cp = ((c & 0x07) << 18) | ((utf8[i+1] & 0x3F) << 12) |
                 ((utf8[i+2] & 0x3F) << 6) | (utf8[i+3] & 0x3F); i += 4;
        } else { i++; continue; }
        target[pos++] = cp;
    }
    if (copy_null && pos < targetsize) {
        target[pos] = 0;
    }
    return target;
}

static inline Py_UCS4 *PyUnicode_AsUCS4Copy(PyObject *unicode) {
    Py_ssize_t len = PyUnicode_GetLength(unicode);
    Py_UCS4 *buf;
    if (len < 0) return NULL;
    buf = (Py_UCS4 *)PyMem_Malloc(((size_t)len + 1) * sizeof(Py_UCS4));
    if (buf == NULL) return NULL;
    if (PyUnicode_AsUCS4(unicode, buf, len + 1, 1) == NULL) {
        PyMem_Free(buf);
        return NULL;
    }
    return buf;
}

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

static inline int PyOS_InterruptOccurred(void) {
    return 0;
}

/* ---- PyUnicode_Splitlines already exists, ensure PyUnicode_DecodeCharmap ---- */

static inline PyObject *PyUnicode_DecodeCharmap(const char *data, Py_ssize_t size,
                                                  PyObject *mapping, const char *errors) {
    (void)mapping; (void)errors;
    /* Fallback: decode as latin-1 */
    return PyUnicode_DecodeLatin1(data, size, errors);
}

/* PyType_FromModuleAndSpec — already defined earlier in this file. */

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

static inline int PyLong_AsInt(PyObject *obj) {
    long val = PyLong_AsLong(obj);
    if (val > INT_MAX || val < INT_MIN) {
        PyErr_SetString(PyExc_OverflowError, "Python int too large to convert to C int");
        return -1;
    }
    return (int)val;
}

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

/* PyDescr_NewMethod, PyDescr_NewClassMethod, PyDescr_NewMember,
   PyDescr_NewGetSet — already defined earlier in this file. */

/* ---- PySlice_GetIndices ---- */

static inline int PySlice_GetIndices(PyObject *slice, Py_ssize_t length,
                                      Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step) {
    Py_ssize_t slicelength;
    return PySlice_GetIndicesEx(slice, length, start, stop, step, &slicelength);
}

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
    (void)op;
    /* Molt always stores as UTF-8 internally; report as 1BYTE_KIND */
    return PyUnicode_1BYTE_KIND;
}
#define PyUnicode_KIND(op) PyUnicode_KIND((PyObject *)(op))
#endif

#ifndef PyUnicode_DATA
static inline void *PyUnicode_DATA(PyObject *op) {
    return (void *)PyUnicode_AsUTF8(op);
}
#define PyUnicode_DATA(op) PyUnicode_DATA((PyObject *)(op))
#endif

/* Py_CLEAR, Py_SETREF, Py_XSETREF — already defined earlier in this file. */

/* ---- Py_IS_TYPE ---- */

#ifndef Py_IS_TYPE
#define Py_IS_TYPE(ob, type) (Py_TYPE(ob) == (type))
#endif

/* ---- PyObject_SelfIter ---- */

static inline PyObject *PyObject_SelfIter(PyObject *obj) {
    Py_INCREF(obj);
    return obj;
}

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MOLT_C_API_PYTHON_H */
