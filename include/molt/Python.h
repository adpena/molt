#ifndef MOLT_C_API_PYTHON_H
#define MOLT_C_API_PYTHON_H

#include <stdarg.h>
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
typedef struct _molt_pyobject PyObject;
typedef PyObject PyTypeObject;

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);

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

static inline const char *PyUnicode_AsUTF8AndSize(PyObject *value, Py_ssize_t *size_out);

#ifndef PYTHON_API_VERSION
#define PYTHON_API_VERSION 1013
#endif

#define METH_VARARGS 0x0001
#define METH_KEYWORDS 0x0002
#define METH_NOARGS 0x0004
#define METH_O 0x0008

#define PyModuleDef_HEAD_INIT NULL

#define Py_SUCCESS 0
#define Py_FAILURE -1

#define PY_MAJOR_VERSION 3
#define PY_MINOR_VERSION 12
#define PY_MICRO_VERSION 0

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

#define PyExc_TypeError _molt_pyexc_type_error()
#define PyExc_ValueError _molt_pyexc_value_error()
#define PyExc_RuntimeError _molt_pyexc_runtime_error()
#define PyExc_OverflowError _molt_pyexc_overflow_error()
#define PyExc_ImportError _molt_pyexc_import_error()
#define PyExc_PermissionError _molt_pyexc_permission_error()
#define PyExc_KeyError _molt_pyexc_key_error()

static inline int Py_IsInitialized(void) {
    return 1;
}

static inline void Py_Initialize(void) {
    (void)molt_init();
}

static inline void Py_Finalize(void) {
    (void)molt_shutdown();
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

#define Py_None _molt_pyobject_from_handle(molt_none())
#define Py_True _molt_pyobject_from_handle(molt_bool_from_i32(1))
#define Py_False _molt_pyobject_from_handle(molt_bool_from_i32(0))

#define Py_RETURN_NONE                                                             \
    do {                                                                           \
        Py_INCREF(Py_None);                                                        \
        return Py_None;                                                            \
    } while (0)

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

static inline int PyObject_IsTrue(PyObject *obj) {
    return molt_object_truthy(_molt_py_handle(obj));
}

static inline PyObject *PyObject_Str(PyObject *obj) {
    return _molt_pyobject_from_result(molt_object_str(_molt_py_handle(obj)));
}

static inline PyObject *PyObject_Repr(PyObject *obj) {
    return _molt_pyobject_from_result(molt_object_repr(_molt_py_handle(obj)));
}

static inline int PyType_Ready(PyTypeObject *type) {
    if (type == NULL) {
        PyErr_SetString(PyExc_TypeError, "type must not be NULL");
        return -1;
    }
    return molt_type_ready(_molt_py_handle((PyObject *)type));
}

static inline PyObject *PyModule_Create2(PyModuleDef *def, int api_version) {
    MoltHandle name_bits;
    MoltHandle module_bits;
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
    return _molt_pyobject_from_result(module_bits);
}

#define PyModule_Create(def) PyModule_Create2((def), PYTHON_API_VERSION)

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

static inline PyObject *PyLong_FromLong(long value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline PyObject *PyLong_FromLongLong(long long value) {
    return _molt_pyobject_from_result(molt_int_from_i64((int64_t)value));
}

static inline long PyLong_AsLong(PyObject *obj) {
    return (long)molt_int_as_i64(_molt_py_handle(obj));
}

static inline long long PyLong_AsLongLong(PyObject *obj) {
    return (long long)molt_int_as_i64(_molt_py_handle(obj));
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

static inline Py_ssize_t PySequence_Size(PyObject *seq) {
    return (Py_ssize_t)molt_sequence_length(_molt_py_handle(seq));
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
