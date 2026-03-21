#ifndef MOLT_C_API_PYTHON_H
#define MOLT_C_API_PYTHON_H

#include <stdarg.h>
#include <errno.h>
#include <limits.h>
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

typedef intptr_t Py_ssize_t;
typedef Py_ssize_t Py_hash_t;
typedef struct _molt_pyobject PyObject;
typedef PyObject PyTypeObject;
typedef int PyGILState_STATE;
typedef uint32_t Py_UCS4;
typedef MoltBufferView Py_buffer;

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
    Py_ssize_t len = 0;
    if (PyUnicode_AsUTF8AndSize(value, &len) == NULL) {
        return -1;
    }
    return len;
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

#define PyUnicode_4BYTE_KIND 4

static inline PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size) {
    if (value == NULL && size > 0) {
        PyErr_SetString(PyExc_TypeError, "bytes source must not be NULL when size > 0");
        return NULL;
    }
    return _molt_pyobject_from_result(
        molt_bytes_from((const uint8_t *)value, size < 0 ? 0u : (uint64_t)size));
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
    return molt_buffer_acquire(_molt_py_handle(obj), view);
}

static inline void PyBuffer_Release(Py_buffer *view) {
    if (view == NULL) {
        return;
    }
    (void)molt_buffer_release(view);
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

static inline void *PyType_GetSlot(PyTypeObject *type, int slot) {
    (void)type;
    (void)slot;
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
    PyObject *module;
    MoltHandle name_bits;
    MoltHandle module_bits;
    if (name == NULL || name[0] == '\0') {
        PyErr_SetString(PyExc_ValueError, "module name must not be empty");
        return NULL;
    }
    module = PyImport_ImportModule(name);
    if (module != NULL) {
        Py_DECREF(module);
        return module;
    }
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
    Py_DECREF(module);
    return module;
}

static inline PyObject *PyImport_GetModuleDict(void) {
    PyObject *sys_mod = PyImport_ImportModule("sys");
    PyObject *modules;
    if (sys_mod == NULL) {
        return NULL;
    }
    modules = PyObject_GetAttrString(sys_mod, "modules");
    Py_DECREF(sys_mod);
    if (modules == NULL) {
        return NULL;
    }
    Py_DECREF(modules);
    return modules;
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
    PyObject *builtins_dict;
    if (builtins_mod == NULL) {
        return NULL;
    }
    builtins_dict = PyObject_GetAttrString(builtins_mod, "__dict__");
    Py_DECREF(builtins_mod);
    if (builtins_dict == NULL) {
        return NULL;
    }
    Py_DECREF(builtins_dict);
    return builtins_dict;
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
    PyObject *obj;
    if (sys_mod == NULL) {
        return NULL;
    }
    obj = PyObject_GetAttrString(sys_mod, name);
    Py_DECREF(sys_mod);
    if (obj == NULL) {
        return NULL;
    }
    Py_DECREF(obj);
    return obj;
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

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MOLT_C_API_PYTHON_H */
