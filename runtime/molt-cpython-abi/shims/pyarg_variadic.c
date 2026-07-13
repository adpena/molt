/*
 * pyarg_variadic.c — C shim for variadic CPython argument parsing functions.
 *
 * These must be written in C because Rust stable doesn't support exporting
 * variadic extern "C" functions (requires nightly #![feature(c_variadic)]).
 *
 * The heavy logic lives in the Rust side (errors.rs parse_args_inner).
 * These shims convert va_list → a fixed-width array of void* pointers that
 * the Rust implementation can consume without variadic machinery.
 *
 * SIMD optimisations in the Rust side handle the hot-path type dispatch.
 */

#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

/* Forward declarations for Rust-implemented helpers. */
typedef ptrdiff_t Py_ssize_t;
typedef struct _typeobject PyTypeObject;
typedef struct _object {
    Py_ssize_t ob_refcnt;
    PyTypeObject *ob_type;
} PyObject;

#define MOLT_VARARG_MAX_ARGS 64

extern PyObject *PyObject_GetAttr(PyObject *op, PyObject *name);
extern PyObject *PyObject_GetAttrString(PyObject *op, const char *name);
extern PyObject *PyObject_Call(PyObject *callable, PyObject *args, PyObject *kwargs);
extern PyObject *PyTuple_New(Py_ssize_t size);
extern int PyTuple_SetItem(PyObject *op, Py_ssize_t i, PyObject *value);
extern PyObject *PyLong_FromLong(long value);
extern PyObject *PyLong_FromUnsignedLong(unsigned long value);
extern PyObject *PyLong_FromLongLong(long long value);
extern PyObject *PyLong_FromUnsignedLongLong(unsigned long long value);
extern PyObject *PyLong_FromSsize_t(Py_ssize_t value);
extern PyObject *PyFloat_FromDouble(double value);
typedef struct { double real; double imag; } Py_complex;
extern PyObject *PyComplex_FromCComplex(Py_complex value);
extern PyObject *PyBool_FromLong(long value);
extern PyObject *PyUnicode_FromString(const char *s);
extern PyObject *PyUnicode_FromStringAndSize(const char *s, Py_ssize_t size);
extern PyObject *PyUnicode_FromOrdinal(int ordinal);
extern const char *PyUnicode_AsUTF8(PyObject *op);
extern const char *PyUnicode_AsUTF8AndSize(PyObject *op, Py_ssize_t *size);
extern int PyUnicode_Check(PyObject *op);
extern PyObject *PyObject_Repr(PyObject *op);
extern PyObject *PyObject_Str(PyObject *op);
extern PyObject *PyObject_ASCII(PyObject *op);
extern int PyType_Check(PyObject *op);
extern PyObject *molt_capi_type_fully_qualified_name(PyTypeObject *type);
extern PyObject *PyBytes_FromStringAndSize(const char *s, Py_ssize_t size);
extern PyObject *PyList_New(Py_ssize_t size);
extern int PyList_SetItem(PyObject *op, Py_ssize_t i, PyObject *value);
extern PyObject *PyDict_New(void);
extern int PyDict_SetItem(PyObject *op, PyObject *key, PyObject *value);
extern int PyErr_WarnEx(PyObject *category, const char *message, Py_ssize_t stack_level);
extern void PyErr_SetString(PyObject *exc_type, const char *message);
extern void PyErr_SetObject(PyObject *exc_type, PyObject *value);
extern PyObject *PyErr_Occurred(void);
extern PyObject *PyErr_NoMemory(void);
extern void PyErr_WriteUnraisable(PyObject *obj);
extern void molt_capi_err_format_unraisable(const unsigned char *message, size_t len);
extern void Py_INCREF(PyObject *op);
extern void Py_DECREF(PyObject *op);
extern PyObject Py_None;
extern PyObject PyExc_TypeError;
extern PyObject PyExc_ValueError;
extern PyObject PyExc_OverflowError;
extern PyObject PyExc_SystemError;

/*
 * Rust entry point — called with a flat array of output void* pointers.
 * Implemented in errors.rs as `pyarg_parse_tuple_inner`.
 */
extern int molt_pyarg_parse_tuple_inner(
    PyObject *args,
    const char *format,
    void **outs,
    int n_outs);

/*
 * Count the number of output pointers a format string requires.
 * Stops at ':', ';', or end of string. Optional fields after '|' still have
 * output pointers when present, so collect them for the shared Rust parser.
 */
static size_t count_format_outs(const char *fmt) {
    size_t count = 0;
    for (const char *p = fmt; *p; p++) {
        char c = *p;
        if (c == ':' || c == ';') break;
        switch (c) {
        case 'O':
            /* 'O' takes one out; 'O!'/'O&' consume a SECOND vararg (the type
             * object / converter fn) — the whole reason the O! header-clobber
             * bug existed was this count omitting it. Skip the modifier char. */
            count++;
            if (*(p+1) == '!' || *(p+1) == '&') { count++; p++; }
            break;
        case 's': case 'z': case 'y':
            count++;
            if (*(p+1) == '#' || *(p+1) == '*') {
                if (*(p+1) == '#') count++;
                p++;
            }
            break;
        case 'e':
            count += 2;
            if (*(p+1) == 's' || *(p+1) == 't') p++;
            if (*(p+1) == '#') { count++; p++; }
            break;
        case 'w':
            count++;
            if (*(p+1) == '*') p++;
            break;
        case 'i': case 'l': case 'd': case 'f':
        case 'p': case 'n': case 'L': case 'K': case 'H':
        case 'I': case 'k': case 'B': case 'C': case 'b':
        case 'h': case 'c': case 'S': case 'Y': case 'U': case 'D':
            count++;
            break;
        case '(': case ')': case '|': case '$':
            break; /* skip grouping / optional-marker / encoding flags */
        default:
            break;
        }
    }
    return count;
}

/*
 * Collect va_list pointers into a fixed-width array and dispatch to Rust.
 *
 * PERFORMANCE: This function is on the hot path — called for every C extension
 * function entry. The count_format_outs loop is O(format_len) but format
 * strings are short (typically ≤12 chars) and CPU branch-predicted well.
 * The va_arg loop has no branches per iteration (pointer-width reads only).
 */
static int collect_and_dispatch(
    PyObject *args,
    const char *format,
    va_list ap)
{
    size_t n = count_format_outs(format);
    void **outs = n == 0 ? NULL : (void **)malloc(n * sizeof(*outs));
    if (n != 0 && outs == NULL) {
        PyErr_SetString(&PyExc_TypeError, "PyArg_ParseTuple vararg allocation failed");
        return 0;
    }
    for (size_t i = 0; i < n; i++) {
        outs[i] = va_arg(ap, void *);
    }
    int result = molt_pyarg_parse_tuple_inner(args, format, outs, (int)n);
    free(outs);
    return result;
}

int PyArg_VaParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    va_list vargs)
{
    (void)kwargs;
    (void)kwlist;
    va_list ap;
    va_copy(ap, vargs);
    int result = collect_and_dispatch(args, format, ap);
    va_end(ap);
    return result;
}

int _PyArg_VaParseTupleAndKeywords_SizeT(PyObject *args, PyObject *kwargs,
    const char *format, char **kwlist, va_list vargs) {
    return PyArg_VaParseTupleAndKeywords(args, kwargs, format, kwlist, vargs);
}

int _PyArg_VaParse_SizeT(PyObject *args, const char *format, va_list vargs) {
    va_list ap;
    va_copy(ap, vargs);
    int result = collect_and_dispatch(args, format, ap);
    va_end(ap);
    return result;
}

int _PyArg_ParseTuple_SizeT(PyObject *args, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int result = collect_and_dispatch(args, format, ap);
    va_end(ap);
    return result;
}

int _PyArg_ParseTupleAndKeywords_SizeT(PyObject *args, PyObject *kwargs,
    const char *format, char **kwlist, ...) {
    va_list ap;
    va_start(ap, kwlist);
    int result = PyArg_VaParseTupleAndKeywords(args, kwargs, format, kwlist, ap);
    va_end(ap);
    return result;
}

PyObject *PyTuple_Pack(Py_ssize_t n, ...) {
    if (n < 0 || n > MOLT_VARARG_MAX_ARGS) return NULL;
    PyObject *tuple = PyTuple_New(n);
    if (tuple == NULL) return NULL;

    va_list ap;
    va_start(ap, n);
    for (Py_ssize_t i = 0; i < n; i++) {
        PyObject *item = va_arg(ap, PyObject *);
        if (item == NULL) {
            va_end(ap);
            Py_DECREF(tuple);
            return NULL;
        }
        Py_INCREF(item);
        if (PyTuple_SetItem(tuple, i, item) != 0) {
            Py_DECREF(item);
            va_end(ap);
            Py_DECREF(tuple);
            return NULL;
        }
    }
    va_end(ap);
    return tuple;
}

static void molt_buildvalue_skip_separators(const char **cursor) {
    while (**cursor == ' ' || **cursor == '\t' || **cursor == '\n' ||
           **cursor == '\r' || **cursor == ',') {
        (*cursor)++;
    }
}

static PyObject *molt_buildvalue_parse_item(const char **cursor, va_list *ap);

static PyObject *molt_buildvalue_parse_tuple(const char **cursor, va_list *ap) {
    PyObject *items[MOLT_VARARG_MAX_ARGS];
    Py_ssize_t len = 0;
    for (;;) {
        molt_buildvalue_skip_separators(cursor);
        if (**cursor == ')') {
            (*cursor)++;
            break;
        }
        if (**cursor == '\0') {
            PyErr_SetString(&PyExc_TypeError, "unterminated tuple format in Py_BuildValue");
            goto error;
        }
        if (len >= MOLT_VARARG_MAX_ARGS) {
            PyErr_SetString(&PyExc_TypeError, "too many Py_BuildValue tuple items");
            goto error;
        }
        items[len] = molt_buildvalue_parse_item(cursor, ap);
        if (items[len] == NULL) goto error;
        len++;
        molt_buildvalue_skip_separators(cursor);
    }

    PyObject *tuple = PyTuple_New(len);
    if (tuple == NULL) goto error;
    for (Py_ssize_t i = 0; i < len; i++) {
        if (PyTuple_SetItem(tuple, i, items[i]) != 0) {
            Py_DECREF(items[i]);
            for (Py_ssize_t j = i + 1; j < len; j++) Py_DECREF(items[j]);
            Py_DECREF(tuple);
            return NULL;
        }
        items[i] = NULL;
    }
    return tuple;

error:
    for (Py_ssize_t i = 0; i < len; i++) {
        if (items[i] != NULL) Py_DECREF(items[i]);
    }
    return NULL;
}

static PyObject *molt_buildvalue_parse_list(const char **cursor, va_list *ap) {
    PyObject *items[MOLT_VARARG_MAX_ARGS];
    Py_ssize_t len = 0;
    for (;;) {
        molt_buildvalue_skip_separators(cursor);
        if (**cursor == ']') {
            (*cursor)++;
            break;
        }
        if (**cursor == '\0') {
            PyErr_SetString(&PyExc_TypeError, "unterminated list format in Py_BuildValue");
            goto error;
        }
        if (len >= MOLT_VARARG_MAX_ARGS) {
            PyErr_SetString(&PyExc_TypeError, "too many Py_BuildValue list items");
            goto error;
        }
        items[len] = molt_buildvalue_parse_item(cursor, ap);
        if (items[len] == NULL) goto error;
        len++;
        molt_buildvalue_skip_separators(cursor);
    }

    PyObject *list = PyList_New(len);
    if (list == NULL) goto error;
    for (Py_ssize_t i = 0; i < len; i++) {
        if (PyList_SetItem(list, i, items[i]) != 0) {
            Py_DECREF(items[i]);
            for (Py_ssize_t j = i + 1; j < len; j++) Py_DECREF(items[j]);
            Py_DECREF(list);
            return NULL;
        }
        items[i] = NULL;
    }
    return list;

error:
    for (Py_ssize_t i = 0; i < len; i++) {
        if (items[i] != NULL) Py_DECREF(items[i]);
    }
    return NULL;
}

static PyObject *molt_buildvalue_parse_dict(const char **cursor, va_list *ap) {
    PyObject *dict = PyDict_New();
    if (dict == NULL) return NULL;
    for (;;) {
        molt_buildvalue_skip_separators(cursor);
        if (**cursor == '}') {
            (*cursor)++;
            break;
        }
        if (**cursor == '\0') {
            PyErr_SetString(&PyExc_TypeError, "unterminated dict format in Py_BuildValue");
            Py_DECREF(dict);
            return NULL;
        }
        PyObject *key = molt_buildvalue_parse_item(cursor, ap);
        if (key == NULL) {
            Py_DECREF(dict);
            return NULL;
        }
        molt_buildvalue_skip_separators(cursor);
        if (**cursor == '}' || **cursor == '\0') {
            PyErr_SetString(&PyExc_TypeError,
                            "dict format in Py_BuildValue has an odd number of items");
            Py_DECREF(key);
            Py_DECREF(dict);
            return NULL;
        }
        PyObject *value = molt_buildvalue_parse_item(cursor, ap);
        if (value == NULL) {
            Py_DECREF(key);
            Py_DECREF(dict);
            return NULL;
        }
        int rc = PyDict_SetItem(dict, key, value);
        Py_DECREF(key);
        Py_DECREF(value);
        if (rc != 0) {
            Py_DECREF(dict);
            return NULL;
        }
        molt_buildvalue_skip_separators(cursor);
    }
    return dict;
}

static PyObject *molt_buildvalue_parse_item(const char **cursor, va_list *ap) {
    molt_buildvalue_skip_separators(cursor);
    char code = **cursor;
    if (code == '\0') {
        PyErr_SetString(&PyExc_TypeError, "unexpected end of format in Py_BuildValue");
        return NULL;
    }
    if (code == '(') {
        (*cursor)++;
        return molt_buildvalue_parse_tuple(cursor, ap);
    }
    if (code == '[') {
        (*cursor)++;
        return molt_buildvalue_parse_list(cursor, ap);
    }
    if (code == '{') {
        (*cursor)++;
        return molt_buildvalue_parse_dict(cursor, ap);
    }
    (*cursor)++;
    switch (code) {
    case 'O':
    case 'S':
    case 'U': {
        // 'O'/'S'/'U' all take a borrowed PyObject* and return a new
        // reference; the S/U type distinction is advisory in CPython's
        // builder and not enforced here.
        PyObject *obj = va_arg(*ap, PyObject *);
        if (obj == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue object format received NULL");
            return NULL;
        }
        Py_INCREF(obj);
        return obj;
    }
    case 'N': {
        PyObject *obj = va_arg(*ap, PyObject *);
        if (obj == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue 'N' received NULL");
            return NULL;
        }
        return obj;
    }
    case 'i':
        return PyLong_FromLong((long)va_arg(*ap, int));
    case 'b':
        return PyLong_FromLong((long)(signed char)va_arg(*ap, int));
    case 'B':
        return PyLong_FromUnsignedLong((unsigned long)(unsigned char)va_arg(*ap, int));
    case 'h':
        return PyLong_FromLong((long)(short)va_arg(*ap, int));
    case 'H':
        return PyLong_FromUnsignedLong((unsigned long)(unsigned short)va_arg(*ap, int));
    case 'I':
        return PyLong_FromUnsignedLong((unsigned long)va_arg(*ap, unsigned int));
    case 'l':
        return PyLong_FromLong(va_arg(*ap, long));
    case 'n':
        return PyLong_FromSsize_t(va_arg(*ap, Py_ssize_t));
    case 'k':
        return PyLong_FromUnsignedLong(va_arg(*ap, unsigned long));
    case 'K':
        return PyLong_FromUnsignedLongLong(va_arg(*ap, unsigned long long));
    case 'L':
        return PyLong_FromLongLong(va_arg(*ap, long long));
    case 'd':
    case 'f':
        return PyFloat_FromDouble(va_arg(*ap, double));
    case 'D': {
        Py_complex *value = va_arg(*ap, Py_complex *);
        if (value == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue 'D' received NULL");
            return NULL;
        }
        return PyComplex_FromCComplex(*value);
    }
    case 'p':
        return PyBool_FromLong(va_arg(*ap, int) != 0);
    case 's':
    case 'z': {
        const char *text = va_arg(*ap, const char *);
        int has_len = (**cursor == '#');
        if (has_len) {
            (*cursor)++;
        }
        // CPython Py_BuildValue: 's'/'z' (and 'U') with a NULL C string pointer
        // yield None — any '#' length arg is still consumed from varargs and
        // ignored. This is NOT 'z'-only: 's' behaves identically per the C-API
        // docs (numpy _multiarray_umath init passes `s`/NULL expecting None).
        if (text == NULL) {
            if (has_len) {
                (void)va_arg(*ap, Py_ssize_t);
            }
            Py_INCREF(&Py_None);
            return &Py_None;
        }
        if (has_len) {
            Py_ssize_t len = va_arg(*ap, Py_ssize_t);
            return PyUnicode_FromStringAndSize(text, len);
        }
        return PyUnicode_FromString(text);
    }
    case 'y': {
        const char *bytes = va_arg(*ap, const char *);
        int has_len = (**cursor == '#');
        if (has_len) {
            (*cursor)++;
        }
        // CPython: 'y' with a NULL pointer yields None (any '#' length arg is
        // consumed and ignored), matching 's'/'z' above.
        if (bytes == NULL) {
            if (has_len) {
                (void)va_arg(*ap, Py_ssize_t);
            }
            Py_INCREF(&Py_None);
            return &Py_None;
        }
        Py_ssize_t len = has_len ? va_arg(*ap, Py_ssize_t) : (Py_ssize_t)strlen(bytes);
        return PyBytes_FromStringAndSize(bytes, len);
    }
    case 'c': {
        // A single byte, returned as a length-1 bytes object.
        char ch = (char)va_arg(*ap, int);
        return PyBytes_FromStringAndSize(&ch, 1);
    }
    case 'C': {
        int ordinal = va_arg(*ap, int);
        return PyUnicode_FromOrdinal(ordinal);
    }
    default: {
        char detail[64];
        snprintf(detail, sizeof(detail),
                 "unsupported format unit '%c' (0x%02x) in Py_BuildValue",
                 (code >= 32 && code < 127) ? code : '?', (unsigned char)code);
        PyErr_SetString(&PyExc_TypeError, detail);
        return NULL;
    }
    }
}

PyObject *Py_VaBuildValue(const char *format, va_list vargs) {
    if (format == NULL) {
        PyErr_SetString(&PyExc_TypeError, "format must not be NULL");
        return NULL;
    }
    va_list ap;
    va_copy(ap, vargs);
    const char *cursor = format;
    PyObject *items[MOLT_VARARG_MAX_ARGS];
    Py_ssize_t len = 0;

    for (;;) {
        molt_buildvalue_skip_separators(&cursor);
        if (*cursor == '\0') break;
        if (len >= MOLT_VARARG_MAX_ARGS) {
            PyErr_SetString(&PyExc_TypeError, "too many Py_BuildValue items");
            goto error;
        }
        items[len] = molt_buildvalue_parse_item(&cursor, &ap);
        if (items[len] == NULL) goto error;
        len++;
    }
    va_end(ap);

    if (len == 0) {
        Py_INCREF(&Py_None);
        return &Py_None;
    }
    if (len == 1) {
        return items[0];
    }
    PyObject *tuple = PyTuple_New(len);
    if (tuple == NULL) goto post_va_error;
    for (Py_ssize_t i = 0; i < len; i++) {
        if (PyTuple_SetItem(tuple, i, items[i]) != 0) {
            Py_DECREF(items[i]);
            for (Py_ssize_t j = i + 1; j < len; j++) Py_DECREF(items[j]);
            Py_DECREF(tuple);
            return NULL;
        }
        items[i] = NULL;
    }
    return tuple;

error:
    va_end(ap);
post_va_error:
    for (Py_ssize_t i = 0; i < len; i++) {
        if (items[i] != NULL) Py_DECREF(items[i]);
    }
    return NULL;
}

PyObject *Py_BuildValue(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = Py_VaBuildValue(format, ap);
    va_end(ap);
    return result;
}

PyObject *_Py_BuildValue_SizeT(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = Py_VaBuildValue(format, ap);
    va_end(ap);
    return result;
}

int PyArg_ParseTuple(PyObject *args, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int result = collect_and_dispatch(args, format, ap);
    va_end(ap);
    return result;
}

int PyArg_ParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    ...)
{
    va_list ap;
    va_start(ap, kwlist);
    int result = PyArg_VaParseTupleAndKeywords(args, kwargs, format, kwlist, ap);
    va_end(ap);
    return result;
}

int PyArg_UnpackTuple(
    PyObject *args,
    const char *name,
    Py_ssize_t min,
    Py_ssize_t max,
    ...)
{
    (void)name;
    (void)min;
    if (max < 0 || max > INT_MAX - 2) return 0;
    int take = (int)max;
    char *fmt = (char *)malloc((size_t)take + 2);
    void **outs = take == 0 ? NULL : (void **)malloc((size_t)take * sizeof(*outs));
    if (fmt == NULL || (take != 0 && outs == NULL)) {
        free(fmt);
        free(outs);
        PyErr_SetString(&PyExc_TypeError, "PyArg_UnpackTuple allocation failed");
        return 0;
    }
    int i;
    for (i = 0; i < take; i++) fmt[i] = 'O';
    fmt[i] = '|';
    fmt[i+1] = '\0';
    va_list ap;
    va_start(ap, max);
    for (int j = 0; j < take; j++) {
        outs[j] = va_arg(ap, void *);
    }
    va_end(ap);

    int result = molt_pyarg_parse_tuple_inner(args, fmt, outs, take);
    free(outs);
    free(fmt);
    return result;
}

static PyObject *molt_call_with_collected_args(PyObject *callable, va_list ap) {
    PyObject *items[MOLT_VARARG_MAX_ARGS];
    int n = 0;
    for (;;) {
        PyObject *item = va_arg(ap, PyObject *);
        if (item == NULL) break;
        if (n >= MOLT_VARARG_MAX_ARGS) return NULL;
        items[n++] = item;
    }

    PyObject *tuple = PyTuple_New((Py_ssize_t)n);
    if (tuple == NULL) return NULL;
    for (int i = 0; i < n; i++) {
        Py_INCREF(items[i]);
        if (PyTuple_SetItem(tuple, (Py_ssize_t)i, items[i]) != 0) {
            Py_DECREF(items[i]);
            Py_DECREF(tuple);
            return NULL;
        }
    }
    PyObject *result = PyObject_Call(callable, tuple, NULL);
    Py_DECREF(tuple);
    return result;
}

static int molt_callfunction_format_starts_tuple(const char *format) {
    if (format == NULL) return 0;
    while (*format == ' ' || *format == '\t' || *format == '\n' ||
           *format == '\r' || *format == ',') {
        format++;
    }
    return *format == '(';
}

static int molt_callfunction_top_level_item_count(const char *format) {
    if (format == NULL) return 0;
    const char *cursor = format;
    int count = 0;
    while (*cursor != '\0') {
        molt_buildvalue_skip_separators(&cursor);
        if (*cursor == '\0') break;
        if (*cursor == '(') return -1;
        count++;
        cursor++;
        if (*cursor == '#') cursor++;
    }
    return count;
}

/* Shared `PyObject_CallFunction` / `PyObject_CallMethod` argument builder:
 * CPython semantics — an empty/NULL format is a no-arg call; a format that
 * builds a single non-tuple value becomes a 1-tuple; a tuple-shaped or
 * multi-item format is the args tuple itself. Returns a NEW args tuple, or
 * NULL with an exception set. */
static PyObject *molt_callfunction_build_args(const char *format, va_list ap) {
    if (format == NULL || format[0] == '\0') {
        return PyTuple_New(0);
    }
    PyObject *built = Py_VaBuildValue(format, ap);
    if (built == NULL) return NULL;

    int top_level_count = molt_callfunction_top_level_item_count(format);
    if (molt_callfunction_format_starts_tuple(format) || top_level_count != 1) {
        return built;
    }
    PyObject *args = PyTuple_Pack(1, built);
    Py_DECREF(built);
    return args;
}

static PyObject *molt_object_call_function_va(
    PyObject *callable, const char *format, va_list ap) {
    if (callable == NULL) return NULL;
    PyObject *args = molt_callfunction_build_args(format, ap);
    if (args == NULL) return NULL;
    PyObject *result = PyObject_Call(callable, args, NULL);
    Py_DECREF(args);
    return result;
}

PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = molt_object_call_function_va(callable, format, ap);
    va_end(ap);
    return result;
}

PyObject *_PyObject_CallFunction_SizeT(PyObject *callable, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = molt_object_call_function_va(callable, format, ap);
    va_end(ap);
    return result;
}

PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...) {
    if (callable == NULL) return NULL;
    va_list ap;
    va_start(ap, callable);
    PyObject *result = molt_call_with_collected_args(callable, ap);
    va_end(ap);
    return result;
}

PyObject *PyObject_CallMethodObjArgs(PyObject *callable, PyObject *name, ...) {
    if (callable == NULL || name == NULL) return NULL;
    PyObject *method = PyObject_GetAttr(callable, name);
    if (method == NULL) return NULL;

    va_list ap;
    va_start(ap, name);
    PyObject *result = molt_call_with_collected_args(method, ap);
    va_end(ap);
    Py_DECREF(method);
    return result;
}

static PyObject *molt_object_call_method_va(
    PyObject *callable,
    const char *name,
    const char *format,
    va_list ap)
{
    if (callable == NULL || name == NULL) return NULL;
    PyObject *method = PyObject_GetAttrString(callable, name);
    if (method == NULL) return NULL;

    /* CPython semantics: build the args from the format exactly like
     * PyObject_CallFunction. The old shim returned bare NULL (no exception)
     * for ANY non-empty format — a silent failure that strands the
     * extension's error check. */
    PyObject *args = molt_callfunction_build_args(format, ap);

    if (args == NULL) {
        Py_DECREF(method);
        return NULL;
    }
    PyObject *result = PyObject_Call(method, args, NULL);
    Py_DECREF(args);
    Py_DECREF(method);
    return result;
}

PyObject *PyObject_CallMethod(
    PyObject *callable,
    const char *name,
    const char *format,
    ...)
{
    va_list ap;
    va_start(ap, format);
    PyObject *result = molt_object_call_method_va(callable, name, format, ap);
    va_end(ap);
    return result;
}

PyObject *_PyObject_CallMethod_SizeT(
    PyObject *callable,
    const char *name,
    const char *format,
    ...)
{
    va_list ap;
    va_start(ap, format);
    PyObject *result = molt_object_call_method_va(callable, name, format, ap);
    va_end(ap);
    return result;
}

#define MOLT_UNICODE_FORMAT_INLINE_CAPACITY 256

typedef struct {
    char *data;
    size_t len;
    size_t cap;
    size_t heap_allocations;
    size_t heap_allocation_limit;
    char inline_data[MOLT_UNICODE_FORMAT_INLINE_CAPACITY];
} MoltUnicodeFormatBuffer;

static void molt_unicode_format_buffer_init(MoltUnicodeFormatBuffer *buf) {
    buf->data = buf->inline_data;
    buf->len = 0;
    buf->cap = sizeof(buf->inline_data);
    buf->heap_allocations = 0;
    buf->heap_allocation_limit = (size_t)-1;
    buf->inline_data[0] = '\0';
}

static void molt_unicode_format_buffer_dealloc(MoltUnicodeFormatBuffer *buf) {
    if (buf->data != buf->inline_data) free(buf->data);
    buf->data = buf->inline_data;
    buf->len = 0;
    buf->cap = sizeof(buf->inline_data);
}

static void molt_unicode_format_buffer_absorb_allocations(
    MoltUnicodeFormatBuffer *parent,
    const MoltUnicodeFormatBuffer *child)
{
    parent->heap_allocations += child->heap_allocations;
}

enum {
    MOLT_FMT_LEFT = 1 << 0,
    MOLT_FMT_ZERO = 1 << 1,
    MOLT_FMT_ALT = 1 << 2,
};

typedef enum {
    MOLT_FMT_DEFAULT,
    MOLT_FMT_LONG,
    MOLT_FMT_LONG_LONG,
    MOLT_FMT_SIZE,
    MOLT_FMT_PTRDIFF,
    MOLT_FMT_INTMAX,
} MoltUnicodeFormatLength;

typedef struct {
    unsigned int flags;
    Py_ssize_t width;
    Py_ssize_t precision;
    MoltUnicodeFormatLength length;
    char conversion;
} MoltUnicodeFormatSpec;

static int molt_unicode_format_set_error(PyObject *type, const char *message) {
    PyErr_SetString(type, message);
    return 0;
}

static int molt_unicode_format_system_error(const char *message) {
    return molt_unicode_format_set_error(&PyExc_SystemError, message);
}

static int molt_unicode_format_overflow(const char *message) {
    return molt_unicode_format_set_error(&PyExc_OverflowError, message);
}

static int molt_unicode_format_reserve(MoltUnicodeFormatBuffer *buf, size_t extra) {
    if (extra > (size_t)PTRDIFF_MAX
        || buf->len > (size_t)PTRDIFF_MAX - extra
        || extra > (size_t)-1 - buf->len - 1) {
        return molt_unicode_format_overflow("formatted Unicode string is too long");
    }
    size_t need = buf->len + extra + 1;
    if (need <= buf->cap) return 1;
    size_t cap = buf->cap;
    while (cap < need) {
        if (cap > (size_t)-1 / 2) {
            cap = need;
            break;
        }
        cap *= 2;
    }
    if (cap > buf->heap_allocation_limit) {
        PyErr_NoMemory();
        return 0;
    }
    char *data;
    if (buf->data == buf->inline_data) {
        data = (char *)malloc(cap);
        if (data != NULL) memcpy(data, buf->inline_data, buf->len + 1);
    }
    else {
        data = (char *)realloc(buf->data, cap);
    }
    if (data == NULL) {
        PyErr_NoMemory();
        return 0;
    }
    buf->data = data;
    buf->cap = cap;
    buf->heap_allocations++;
    return 1;
}

static int molt_unicode_format_append_bytes(
    MoltUnicodeFormatBuffer *buf,
    const char *data,
    size_t len)
{
    if (!molt_unicode_format_reserve(buf, len)) return 0;
    if (len != 0) memcpy(buf->data + buf->len, data, len);
    buf->len += len;
    buf->data[buf->len] = '\0';
    return 1;
}

static int molt_unicode_format_append_cstr(
    MoltUnicodeFormatBuffer *buf,
    const char *text)
{
    if (text == NULL) {
        return molt_unicode_format_system_error("NULL string argument in PyUnicode_FromFormat");
    }
    return molt_unicode_format_append_bytes(buf, text, strlen(text));
}

static int molt_unicode_format_append_repeat(
    MoltUnicodeFormatBuffer *buf,
    char value,
    Py_ssize_t count)
{
    if (count <= 0) return 1;
    size_t amount = (size_t)count;
    if (!molt_unicode_format_reserve(buf, amount)) return 0;
    memset(buf->data + buf->len, value, amount);
    buf->len += amount;
    buf->data[buf->len] = '\0';
    return 1;
}

static int molt_unicode_format_append_codepoint(
    MoltUnicodeFormatBuffer *buf,
    uint32_t codepoint)
{
    unsigned char encoded[4];
    size_t len;
    if (codepoint <= 0x7f) {
        encoded[0] = (unsigned char)codepoint;
        len = 1;
    }
    else if (codepoint <= 0x7ff) {
        encoded[0] = (unsigned char)(0xc0 | (codepoint >> 6));
        encoded[1] = (unsigned char)(0x80 | (codepoint & 0x3f));
        len = 2;
    }
    else if (codepoint <= 0xffff) {
        encoded[0] = (unsigned char)(0xe0 | (codepoint >> 12));
        encoded[1] = (unsigned char)(0x80 | ((codepoint >> 6) & 0x3f));
        encoded[2] = (unsigned char)(0x80 | (codepoint & 0x3f));
        len = 3;
    }
    else {
        encoded[0] = (unsigned char)(0xf0 | (codepoint >> 18));
        encoded[1] = (unsigned char)(0x80 | ((codepoint >> 12) & 0x3f));
        encoded[2] = (unsigned char)(0x80 | ((codepoint >> 6) & 0x3f));
        encoded[3] = (unsigned char)(0x80 | (codepoint & 0x3f));
        len = 4;
    }
    return molt_unicode_format_append_bytes(buf, (const char *)encoded, len);
}

/* Return 1 for a valid scalar, 0 for an invalid byte, and -1 for an
 * incomplete sequence at the end of the supplied span. */
static int molt_unicode_format_decode_utf8(
    const unsigned char *text,
    size_t len,
    uint32_t *codepoint,
    size_t *used)
{
    unsigned char first;
    uint32_t value;
    size_t need;
    uint32_t minimum;
    if (len == 0) return -1;
    first = text[0];
    if (first < 0x80) {
        *codepoint = first;
        *used = 1;
        return 1;
    }
    if (first >= 0xc2 && first <= 0xdf) {
        need = 2;
        minimum = 0x80;
        value = first & 0x1f;
    }
    else if (first >= 0xe0 && first <= 0xef) {
        need = 3;
        minimum = 0x800;
        value = first & 0x0f;
    }
    else if (first >= 0xf0 && first <= 0xf4) {
        need = 4;
        minimum = 0x10000;
        value = first & 0x07;
    }
    else {
        *used = 1;
        return 0;
    }
    if (len < need) return -1;
    for (size_t i = 1; i < need; i++) {
        if ((text[i] & 0xc0) != 0x80) {
            *used = 1;
            return 0;
        }
        value = (value << 6) | (text[i] & 0x3f);
    }
    if (value < minimum || value > 0x10ffff || (value >= 0xd800 && value <= 0xdfff)) {
        *used = 1;
        return 0;
    }
    *codepoint = value;
    *used = need;
    return 1;
}

static int molt_unicode_format_append_padded(
    MoltUnicodeFormatBuffer *buf,
    const char *data,
    size_t len,
    Py_ssize_t characters,
    Py_ssize_t width,
    unsigned int flags)
{
    Py_ssize_t padding = width > characters ? width - characters : 0;
    if (!(flags & MOLT_FMT_LEFT) && !molt_unicode_format_append_repeat(buf, ' ', padding)) {
        return 0;
    }
    if (!molt_unicode_format_append_bytes(buf, data, len)) return 0;
    if ((flags & MOLT_FMT_LEFT) && !molt_unicode_format_append_repeat(buf, ' ', padding)) {
        return 0;
    }
    return 1;
}

/* CPython preserves a distinct negative precision sentinel for `.*`: for
 * string-like conversions it means a zero-character result, while -1 means
 * that no precision was supplied. */
static Py_ssize_t molt_unicode_format_string_precision(Py_ssize_t precision) {
    return precision < -1 ? 0 : precision;
}

static int molt_unicode_format_append_utf8_value(
    MoltUnicodeFormatBuffer *buf,
    const char *text,
    size_t len,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags,
    int precision_is_bytes,
    int truncate_incomplete)
{
    precision = molt_unicode_format_string_precision(precision);
    if (len > (size_t)PTRDIFF_MAX) {
        return molt_unicode_format_overflow("formatted Unicode value is too long");
    }
    size_t limit = len;
    size_t offset = 0;
    Py_ssize_t characters = 0;
    int valid_prefix = 1;
    if (precision_is_bytes && precision >= 0 && (size_t)precision < limit) {
        limit = (size_t)precision;
        truncate_incomplete = 1;
    }

    /* Valid UTF-8 is overwhelmingly the common path. Scan once to establish
     * the character count and precision boundary, then append the original
     * bytes directly into the one formatter buffer. */
    while (offset < limit && (precision_is_bytes || precision < 0 || characters < precision)) {
        uint32_t codepoint;
        size_t used = 1;
        int status = molt_unicode_format_decode_utf8(
            (const unsigned char *)text + offset,
            limit - offset,
            &codepoint,
            &used);
        if (status < 0 && truncate_incomplete) break;
        if (status <= 0) {
            valid_prefix = 0;
            break;
        }
        offset += used;
        characters++;
    }
    if (valid_prefix) {
        return molt_unicode_format_append_padded(
            buf, text, offset, characters, width, flags);
    }

    /* Invalid UTF-8 needs U+FFFD substitution. Only this cold path allocates a
     * per-conversion transformation buffer. */
    MoltUnicodeFormatBuffer value;
    molt_unicode_format_buffer_init(&value);
    offset = 0;
    characters = 0;
    while (offset < limit && (precision_is_bytes || precision < 0 || characters < precision)) {
        uint32_t codepoint = 0xfffd;
        size_t used = 1;
        int status = molt_unicode_format_decode_utf8(
            (const unsigned char *)text + offset,
            limit - offset,
            &codepoint,
            &used);
        if (status < 0 && truncate_incomplete) break;
        if (status < 0) {
            /* CPython's replacement decoder treats one incomplete terminal
             * sequence as one decoding error, not one error per remaining
             * byte. Precision-created incomplete suffixes were handled above
             * and are deliberately dropped instead. */
            codepoint = 0xfffd;
            used = limit - offset;
        }
        else if (status == 0) {
            codepoint = 0xfffd;
            used = 1;
        }
        if (!molt_unicode_format_append_codepoint(&value, codepoint)) {
            molt_unicode_format_buffer_absorb_allocations(buf, &value);
            molt_unicode_format_buffer_dealloc(&value);
            return 0;
        }
        offset += used;
        characters++;
    }
    int ok = molt_unicode_format_append_padded(
        buf,
        value.data == NULL ? "" : value.data,
        value.len,
        characters,
        width,
        flags);
    molt_unicode_format_buffer_absorb_allocations(buf, &value);
    molt_unicode_format_buffer_dealloc(&value);
    return ok;
}

static int molt_unicode_format_append_c_utf8(
    MoltUnicodeFormatBuffer *buf,
    const char *text,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags)
{
    if (text == NULL) {
        return molt_unicode_format_system_error("NULL string argument in PyUnicode_FromFormat");
    }
    return molt_unicode_format_append_utf8_value(
        buf, text, strlen(text), width, precision, flags, 1, 0);
}

static int molt_unicode_format_append_wide(
    MoltUnicodeFormatBuffer *buf,
    const wchar_t *text,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags)
{
    precision = molt_unicode_format_string_precision(precision);
    MoltUnicodeFormatBuffer value;
    molt_unicode_format_buffer_init(&value);
    Py_ssize_t characters = 0;
    Py_ssize_t items = 0;
    if (text == NULL) {
        return molt_unicode_format_system_error("NULL wide string argument in PyUnicode_FromFormat");
    }
    while (text[items] != 0 && (precision < 0 || items < precision)) {
        uint32_t codepoint = (uint32_t)text[items++];
#if WCHAR_MAX <= 0xffff
        if (codepoint >= 0xd800 && codepoint <= 0xdbff
            && text[items] != 0
            && (precision < 0 || items < precision)) {
            uint32_t low = (uint32_t)text[items];
            if (low >= 0xdc00 && low <= 0xdfff) {
                items++;
                codepoint = 0x10000 + ((codepoint - 0xd800) << 10) + (low - 0xdc00);
            }
        }
#endif
        if (codepoint > 0x10ffff || (codepoint >= 0xd800 && codepoint <= 0xdfff)) {
            codepoint = 0xfffd;
        }
        if (!molt_unicode_format_append_codepoint(&value, codepoint)) {
            molt_unicode_format_buffer_absorb_allocations(buf, &value);
            molt_unicode_format_buffer_dealloc(&value);
            return 0;
        }
        characters++;
    }
    int ok = molt_unicode_format_append_padded(
        buf,
        value.data == NULL ? "" : value.data,
        value.len,
        characters,
        width,
        flags);
    molt_unicode_format_buffer_absorb_allocations(buf, &value);
    molt_unicode_format_buffer_dealloc(&value);
    return ok;
}

static int molt_unicode_format_append_unicode(
    MoltUnicodeFormatBuffer *buf,
    PyObject *unicode,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags)
{
    Py_ssize_t size = 0;
    if (unicode == NULL) {
        return molt_unicode_format_system_error("NULL object argument in PyUnicode_FromFormat");
    }
    if (!PyUnicode_Check(unicode)) {
        return molt_unicode_format_set_error(
            &PyExc_TypeError,
            "PyUnicode_FromFormat expected a Unicode object");
    }
    const char *text = PyUnicode_AsUTF8AndSize(unicode, &size);
    if (text == NULL) {
        if (PyErr_Occurred() == NULL) {
            molt_unicode_format_system_error("Unicode conversion failed without an exception");
        }
        return 0;
    }
    if (size < 0) {
        return molt_unicode_format_system_error("Unicode conversion returned a negative size");
    }
    return molt_unicode_format_append_utf8_value(
        buf, text, (size_t)size, width, precision, flags, 0, 0);
}

static int molt_unicode_format_append_rendered(
    MoltUnicodeFormatBuffer *buf,
    PyObject *object,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags,
    char conversion)
{
    PyObject *rendered;
    if (object == NULL) {
        return molt_unicode_format_system_error("NULL object argument in PyUnicode_FromFormat");
    }
    if (conversion == 'S') rendered = PyObject_Str(object);
    else if (conversion == 'R') rendered = PyObject_Repr(object);
    else rendered = PyObject_ASCII(object);
    if (rendered == NULL) {
        if (PyErr_Occurred() == NULL) {
            molt_unicode_format_system_error("object rendering failed without an exception");
        }
        return 0;
    }
    int ok = molt_unicode_format_append_unicode(buf, rendered, width, precision, flags);
    Py_DECREF(rendered);
    return ok;
}

static int molt_unicode_format_append_type_name(
    MoltUnicodeFormatBuffer *buf,
    PyTypeObject *type,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags)
{
    if (type == NULL) {
        return molt_unicode_format_system_error("NULL type argument in PyUnicode_FromFormat");
    }
    PyObject *name = molt_capi_type_fully_qualified_name(type);
    if (name == NULL) {
        if (PyErr_Occurred() == NULL) {
            molt_unicode_format_system_error("type name conversion failed without an exception");
        }
        return 0;
    }
    int ok;
    if (flags & MOLT_FMT_ALT) {
        Py_ssize_t size = 0;
        const char *text = PyUnicode_AsUTF8AndSize(name, &size);
        if (text == NULL || size < 0) {
            if (PyErr_Occurred() == NULL) {
                molt_unicode_format_system_error("type name conversion failed without an exception");
            }
            ok = 0;
        }
        else {
            MoltUnicodeFormatBuffer alternate;
            molt_unicode_format_buffer_init(&alternate);
            ok = molt_unicode_format_append_bytes(&alternate, text, (size_t)size);
            if (ok) {
                char *dot = NULL;
                for (size_t i = 0; i < alternate.len; i++) {
                    if (alternate.data[i] == '.') dot = alternate.data + i;
                }
                if (dot != NULL) *dot = ':';
                ok = molt_unicode_format_append_utf8_value(
                    buf,
                    alternate.data == NULL ? "" : alternate.data,
                    alternate.len,
                    width,
                    precision,
                    flags,
                    0,
                    0);
            }
            molt_unicode_format_buffer_absorb_allocations(buf, &alternate);
            molt_unicode_format_buffer_dealloc(&alternate);
        }
    }
    else {
        ok = molt_unicode_format_append_unicode(buf, name, width, precision, flags);
    }
    Py_DECREF(name);
    return ok;
}

static size_t molt_unicode_format_unsigned_digits(
    char *out,
    uintmax_t value,
    unsigned int base,
    int uppercase)
{
    char reverse[sizeof(uintmax_t) * CHAR_BIT + 1];
    const char *alphabet = uppercase ? "0123456789ABCDEF" : "0123456789abcdef";
    size_t len = 0;
    do {
        reverse[len++] = alphabet[value % base];
        value /= base;
    } while (value != 0);
    for (size_t i = 0; i < len; i++) out[i] = reverse[len - i - 1];
    return len;
}

static int molt_unicode_format_checked_add(
    Py_ssize_t left,
    Py_ssize_t right,
    Py_ssize_t *result)
{
    if (left < 0 || right < 0 || left > PTRDIFF_MAX - right) {
        return molt_unicode_format_overflow("formatted integer is too long");
    }
    *result = left + right;
    return 1;
}

static int molt_unicode_format_append_integer(
    MoltUnicodeFormatBuffer *buf,
    int negative,
    uintmax_t magnitude,
    unsigned int base,
    int uppercase,
    Py_ssize_t width,
    Py_ssize_t precision,
    unsigned int flags)
{
    char digits[sizeof(uintmax_t) * CHAR_BIT + 1];
    size_t digits_len = molt_unicode_format_unsigned_digits(digits, magnitude, base, uppercase);
    Py_ssize_t digit_count = (Py_ssize_t)digits_len;
    Py_ssize_t minimum_digits = precision > digit_count ? precision : digit_count;
    Py_ssize_t content;
    if (!molt_unicode_format_checked_add(
            minimum_digits,
            negative ? 1 : 0,
            &content)) {
        return 0;
    }
    Py_ssize_t target_width = width > content ? width : content;
    Py_ssize_t zeros = minimum_digits - digit_count;
    Py_ssize_t spaces = target_width - content;
    if ((flags & MOLT_FMT_ZERO) && !(flags & MOLT_FMT_LEFT)) {
        /* target_width >= content >= sign + digit_count, so both
         * subtractions are defined and non-negative. */
        zeros = target_width - (negative ? 1 : 0) - digit_count;
        spaces = 0;
    }
    if (!(flags & MOLT_FMT_LEFT) && !molt_unicode_format_append_repeat(buf, ' ', spaces)) {
        return 0;
    }
    if (negative && !molt_unicode_format_append_bytes(buf, "-", 1)) return 0;
    if (!molt_unicode_format_append_repeat(buf, '0', zeros)) return 0;
    if (!molt_unicode_format_append_bytes(buf, digits, digits_len)) return 0;
    if ((flags & MOLT_FMT_LEFT) && !molt_unicode_format_append_repeat(buf, ' ', spaces)) {
        return 0;
    }
    return 1;
}

static int molt_unicode_format_parse_decimal(
    const char **cursor,
    Py_ssize_t *value,
    const char *label)
{
    const char *p = *cursor;
    Py_ssize_t result = 0;
    while (*p >= '0' && *p <= '9') {
        int digit = *p - '0';
        if (result > (PTRDIFF_MAX - digit) / 10) {
            char message[96];
            snprintf(message, sizeof(message), "%s too big", label);
            molt_unicode_format_set_error(&PyExc_ValueError, message);
            return 0;
        }
        result = result * 10 + digit;
        p++;
    }
    *cursor = p;
    *value = result;
    return 1;
}

static int molt_unicode_format_parse_spec(
    const char **cursor,
    va_list *args,
    MoltUnicodeFormatSpec *spec)
{
    const char *p = *cursor;
    spec->flags = 0;
    spec->width = -1;
    spec->precision = -1;
    spec->length = MOLT_FMT_DEFAULT;
    for (;;) {
        if (*p == '-') spec->flags |= MOLT_FMT_LEFT;
        else if (*p == '0') spec->flags |= MOLT_FMT_ZERO;
        else if (*p == '#') spec->flags |= MOLT_FMT_ALT;
        else break;
        p++;
    }
    if (*p == '*') {
        int width = va_arg(*args, int);
        p++;
        if (width < 0) {
            spec->flags |= MOLT_FMT_LEFT;
            if (width == INT_MIN) {
                return molt_unicode_format_set_error(&PyExc_ValueError, "width too big");
            }
            width = -width;
        }
        spec->width = width;
    }
    else if (*p >= '0' && *p <= '9') {
        if (!molt_unicode_format_parse_decimal(&p, &spec->width, "width")) return 0;
    }
    if (*p == '.') {
        p++;
        if (*p == '*') {
            int precision = va_arg(*args, int);
            p++;
            spec->precision = precision < 0 ? -2 : precision;
        }
        else if (*p >= '0' && *p <= '9') {
            if (!molt_unicode_format_parse_decimal(&p, &spec->precision, "precision")) return 0;
        }
    }
    if (*p == 'l') {
        if (p[1] == 'l') {
            spec->length = MOLT_FMT_LONG_LONG;
            p += 2;
        }
        else {
            spec->length = MOLT_FMT_LONG;
            p++;
        }
    }
    else if (*p == 'z') {
        spec->length = MOLT_FMT_SIZE;
        p++;
    }
    else if (*p == 't') {
        spec->length = MOLT_FMT_PTRDIFF;
        p++;
    }
    else if (*p == 'j') {
        spec->length = MOLT_FMT_INTMAX;
        p++;
    }
    spec->conversion = *p;
    if (*p == '\0') {
        return molt_unicode_format_system_error("incomplete format in PyUnicode_FromFormat");
    }
    *cursor = p + 1;
    return 1;
}

static intmax_t molt_unicode_format_take_signed(
    va_list *args,
    MoltUnicodeFormatLength length)
{
    switch (length) {
    case MOLT_FMT_LONG: return va_arg(*args, long);
    case MOLT_FMT_LONG_LONG: return va_arg(*args, long long);
    case MOLT_FMT_SIZE: return va_arg(*args, Py_ssize_t);
    case MOLT_FMT_PTRDIFF: return va_arg(*args, ptrdiff_t);
    case MOLT_FMT_INTMAX: return va_arg(*args, intmax_t);
    default: return va_arg(*args, int);
    }
}

static uintmax_t molt_unicode_format_take_unsigned(
    va_list *args,
    MoltUnicodeFormatLength length)
{
    switch (length) {
    case MOLT_FMT_LONG: return va_arg(*args, unsigned long);
    case MOLT_FMT_LONG_LONG: return va_arg(*args, unsigned long long);
    case MOLT_FMT_SIZE: return va_arg(*args, size_t);
    case MOLT_FMT_PTRDIFF: return (uintmax_t)va_arg(*args, ptrdiff_t);
    case MOLT_FMT_INTMAX: return va_arg(*args, uintmax_t);
    default: return va_arg(*args, unsigned int);
    }
}

static int molt_unicode_format_validate_spec(const MoltUnicodeFormatSpec *spec) {
    char conversion = spec->conversion;
    if (conversion == 'd' || conversion == 'i' || conversion == 'o'
        || conversion == 'u' || conversion == 'x' || conversion == 'X') {
        return 1;
    }
    if (conversion == 's' || conversion == 'V') {
        if (spec->length == MOLT_FMT_DEFAULT || spec->length == MOLT_FMT_LONG) return 1;
        return molt_unicode_format_system_error("invalid length modifier in PyUnicode_FromFormat");
    }
    if (spec->length != MOLT_FMT_DEFAULT) {
        return molt_unicode_format_system_error("invalid length modifier in PyUnicode_FromFormat");
    }
    if ((conversion == 'c' || conversion == 'p')
        && (spec->width >= 0 || spec->precision >= 0)) {
        return molt_unicode_format_system_error("width or precision not allowed for this format");
    }
    return 1;
}

static int molt_unicode_format_append_spec(
    MoltUnicodeFormatBuffer *buf,
    const MoltUnicodeFormatSpec *spec,
    va_list *args)
{
    if (!molt_unicode_format_validate_spec(spec)) return 0;
    switch (spec->conversion) {
    case 'd':
    case 'i': {
        intmax_t value = molt_unicode_format_take_signed(args, spec->length);
        int negative = value < 0;
        uintmax_t magnitude = negative
            ? (uintmax_t)(-(value + 1)) + 1
            : (uintmax_t)value;
        return molt_unicode_format_append_integer(
            buf, negative, magnitude, 10, 0,
            spec->width, spec->precision, spec->flags);
    }
    case 'o':
    case 'u':
    case 'x':
    case 'X': {
        uintmax_t value = molt_unicode_format_take_unsigned(args, spec->length);
        unsigned int base = spec->conversion == 'o' ? 8 : (spec->conversion == 'u' ? 10 : 16);
        return molt_unicode_format_append_integer(
            buf, 0, value, base, spec->conversion == 'X',
            spec->width, spec->precision, spec->flags);
    }
    case 'c': {
        int ordinal = va_arg(*args, int);
        if (ordinal < 0 || ordinal > 0x10ffff) {
            return molt_unicode_format_overflow("character argument not in range(0x110000)");
        }
        if (ordinal >= 0xd800 && ordinal <= 0xdfff) {
            /* CPython Unicode can store lone surrogate code points. Molt's
             * current runtime string authority is UTF-8-only, so emitting the
             * three surrogate UTF-8 bytes would create an invalid runtime
             * string and replacing it would silently change the value. Fail
             * closed until the Unicode storage authority can represent it. */
            return molt_unicode_format_system_error(
                "%c lone surrogate requires non-UTF-8 Unicode storage");
        }
        return molt_unicode_format_append_codepoint(buf, (uint32_t)ordinal);
    }
    case 'p': {
        void *pointer = va_arg(*args, void *);
        char number[2 * sizeof(void *) + 32];
        int written = snprintf(number, sizeof(number), "%p", pointer);
        if (written < 0 || (size_t)written >= sizeof(number)) {
            return molt_unicode_format_system_error("%p rendering failed");
        }
        size_t len = (size_t)written;
        if (len >= 2 && number[0] == '0' && number[1] == 'X') {
            number[1] = 'x';
        }
        else if (len < 2 || number[0] != '0' || number[1] != 'x') {
            if (len > sizeof(number) - 3) {
                return molt_unicode_format_overflow("%p rendering is too long");
            }
            memmove(number + 2, number, len + 1);
            number[0] = '0';
            number[1] = 'x';
            len += 2;
        }
        return molt_unicode_format_append_bytes(buf, number, len);
    }
    case 's':
        if (spec->length == MOLT_FMT_LONG) {
            return molt_unicode_format_append_wide(
                buf, va_arg(*args, const wchar_t *),
                spec->width, spec->precision, spec->flags);
        }
        return molt_unicode_format_append_c_utf8(
            buf, va_arg(*args, const char *),
            spec->width, spec->precision, spec->flags);
    case 'U':
        return molt_unicode_format_append_unicode(
            buf, va_arg(*args, PyObject *),
            spec->width, spec->precision, spec->flags);
    case 'V': {
        PyObject *unicode = va_arg(*args, PyObject *);
        if (spec->length == MOLT_FMT_LONG) {
            const wchar_t *fallback = va_arg(*args, const wchar_t *);
            return unicode != NULL
                ? molt_unicode_format_append_unicode(
                    buf, unicode, spec->width, spec->precision, spec->flags)
                : molt_unicode_format_append_wide(
                    buf, fallback, spec->width, spec->precision, spec->flags);
        }
        const char *fallback = va_arg(*args, const char *);
        return unicode != NULL
            ? molt_unicode_format_append_unicode(
                buf, unicode, spec->width, spec->precision, spec->flags)
            : molt_unicode_format_append_c_utf8(
                buf, fallback, spec->width, spec->precision, spec->flags);
    }
    case 'S':
    case 'R':
    case 'A':
        return molt_unicode_format_append_rendered(
            buf, va_arg(*args, PyObject *),
            spec->width, spec->precision, spec->flags, spec->conversion);
    case 'T': {
        PyObject *object = va_arg(*args, PyObject *);
        if (object == NULL) {
            return molt_unicode_format_system_error("NULL object argument for %T");
        }
        PyTypeObject *type = object->ob_type;
        if (type == NULL) {
            return molt_unicode_format_system_error("object has NULL type for %T");
        }
        Py_INCREF((PyObject *)type);
        int result = molt_unicode_format_append_type_name(
            buf, type, spec->width, spec->precision, spec->flags);
        Py_DECREF((PyObject *)type);
        return result;
    }
    case 'N': {
        PyObject *type = va_arg(*args, PyObject *);
        if (type == NULL || !PyType_Check(type)) {
            return molt_unicode_format_set_error(&PyExc_TypeError, "%N argument must be a type");
        }
        return molt_unicode_format_append_type_name(
            buf, (PyTypeObject *)type, spec->width, spec->precision, spec->flags);
    }
    default:
        return molt_unicode_format_system_error("invalid format string in PyUnicode_FromFormat");
    }
}

static int molt_unicode_format_fill(
    MoltUnicodeFormatBuffer *buf,
    const char *format,
    va_list vargs)
{
    if (format == NULL) {
        return molt_unicode_format_system_error("format must not be NULL");
    }
    for (const unsigned char *q = (const unsigned char *)format; *q != '\0'; q++) {
        if (*q > 0x7f) {
            return molt_unicode_format_set_error(
                &PyExc_ValueError,
                "PyUnicode_FromFormatV expects an ASCII-encoded format string");
        }
    }
    va_list args;
    va_copy(args, vargs);
    const char *cursor = format;
    while (*cursor != '\0') {
        const char *percent = strchr(cursor, '%');
        if (percent == NULL) {
            if (!molt_unicode_format_append_cstr(buf, cursor)) goto error;
            break;
        }
        if (!molt_unicode_format_append_bytes(buf, cursor, (size_t)(percent - cursor))) goto error;
        cursor = percent + 1;
        if (*cursor == '%') {
            if (!molt_unicode_format_append_bytes(buf, "%", 1)) goto error;
            cursor++;
            continue;
        }
        MoltUnicodeFormatSpec spec;
        if (!molt_unicode_format_parse_spec(&cursor, &args, &spec)) goto error;
        if (!molt_unicode_format_append_spec(buf, &spec, &args)) goto error;
    }
    va_end(args);
    return 1;

error:
    va_end(args);
    if (PyErr_Occurred() == NULL) {
        molt_unicode_format_system_error("PyUnicode_FromFormatV failed without an exception");
    }
    return 0;
}

static PyObject *molt_unicode_from_format_v_impl(
    const char *format,
    va_list vargs,
    size_t *temporary_heap_allocations,
    size_t heap_allocation_limit)
{
    MoltUnicodeFormatBuffer buf;
    molt_unicode_format_buffer_init(&buf);
    buf.heap_allocation_limit = heap_allocation_limit;
    if (!molt_unicode_format_fill(&buf, format, vargs)) {
        if (temporary_heap_allocations != NULL) {
            *temporary_heap_allocations = buf.heap_allocations;
        }
        molt_unicode_format_buffer_dealloc(&buf);
        return NULL;
    }
    PyObject *result = PyUnicode_FromStringAndSize(buf.data, (Py_ssize_t)buf.len);
    if (temporary_heap_allocations != NULL) {
        *temporary_heap_allocations = buf.heap_allocations;
    }
    molt_unicode_format_buffer_dealloc(&buf);
    if (result == NULL && PyErr_Occurred() == NULL) {
        molt_unicode_format_system_error("Unicode allocation failed without an exception");
    }
    return result;
}

PyObject *PyUnicode_FromFormatV(const char *format, va_list vargs) {
    return molt_unicode_from_format_v_impl(format, vargs, NULL, (size_t)-1);
}

/* Fixed-output diagnostic wrapper used by formatter allocation boundary tests.
 * It executes the exact production parser/renderer and reports only temporary
 * C heap allocations; the final runtime Unicode allocation is intentionally
 * outside this count. */
PyObject *molt_capi_unicode_from_format_probe(
    size_t *temporary_heap_allocations,
    size_t heap_allocation_limit,
    const char *format,
    ...)
{
    va_list ap;
    va_start(ap, format);
    PyObject *result = molt_unicode_from_format_v_impl(
        format, ap, temporary_heap_allocations, heap_allocation_limit);
    va_end(ap);
    return result;
}

PyObject *PyUnicode_FromFormat(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = PyUnicode_FromFormatV(format, ap);
    va_end(ap);
    return result;
}

PyObject *PyErr_FormatV(PyObject *type, const char *format, va_list vargs) {
    PyObject *message = PyUnicode_FromFormatV(format, vargs);
    if (message == NULL) {
        /* The formatter owns the exact error.  Never replace repr/ASCII/OOM or
         * invalid-format failures with the literal format text. */
        return NULL;
    }
    PyErr_SetObject(type, message);
    Py_DECREF(message);
    return NULL;
}

PyObject *PyErr_Format(PyObject *type, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *result = PyErr_FormatV(type, format, ap);
    va_end(ap);
    return result;
}

void PyErr_FormatUnraisable(const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *message = PyUnicode_FromFormatV(format, ap);
    va_end(ap);
    if (message == NULL) {
        PyErr_WriteUnraisable(NULL);
        return;
    }
    Py_ssize_t size = 0;
    const char *text = PyUnicode_AsUTF8AndSize(message, &size);
    if (text == NULL || size < 0) {
        Py_DECREF(message);
        PyErr_WriteUnraisable(NULL);
        return;
    }
    molt_capi_err_format_unraisable(
        (const unsigned char *)text,
        (size_t)size
    );
    Py_DECREF(message);
}

void PySys_WriteStderr(const char *format, ...) {
    if (format == NULL) return;
    va_list ap;
    va_start(ap, format);
    vfprintf(stderr, format, ap);
    va_end(ap);
}

/* C-runtime errno accessors for PyErr_SetFromErrno (errors.rs). The C runtime
 * is the only portable authority for errno: Rust's last_os_error() reads
 * GetLastError() on Windows, a DIFFERENT channel from the errno a C extension
 * just set. */
int molt_capi_errno(void) {
    return errno;
}

void molt_capi_set_errno(int value) {
    errno = value;
}

const char *molt_capi_strerror(int errnum) {
    return strerror(errnum);
}

int PyOS_vsnprintf(char *str, size_t size, const char *format, va_list va) {
    return vsnprintf(str, size, format, va);
}

int PyOS_snprintf(char *str, size_t size, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int result = PyOS_vsnprintf(str, size, format, ap);
    va_end(ap);
    return result;
}

double PyOS_string_to_double(const char *str, char **endptr, PyObject *overflow_exception) {
    if (str == NULL) {
        if (endptr != NULL) {
            *endptr = NULL;
        }
        return -1.0;
    }

    errno = 0;
    char *local_end = NULL;
    double result = strtod(str, &local_end);
    if (endptr != NULL) {
        *endptr = local_end;
    }
    if (errno == ERANGE && overflow_exception != NULL &&
        (result == HUGE_VAL || result == -HUGE_VAL)) {
        PyErr_SetString(overflow_exception, "float overflow");
    }
    return result;
}

long PyOS_strtol(const char *str, char **endptr, int base) {
    if (str == NULL) {
        if (endptr != NULL) {
            *endptr = NULL;
        }
        errno = EINVAL;
        return 0;
    }
    return strtol(str, endptr, base);
}

unsigned long PyOS_strtoul(const char *str, char **endptr, int base) {
    if (str == NULL) {
        if (endptr != NULL) {
            *endptr = NULL;
        }
        errno = EINVAL;
        return 0;
    }
    return strtoul(str, endptr, base);
}

int PyErr_WarnFormat(PyObject *category, Py_ssize_t stack_level, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    PyObject *message = PyUnicode_FromFormatV(format, ap);
    va_end(ap);
    if (message == NULL) return -1;
    Py_ssize_t size = 0;
    const char *text = PyUnicode_AsUTF8AndSize(message, &size);
    if (text == NULL || size < 0) {
        Py_DECREF(message);
        return -1;
    }
    int result = PyErr_WarnEx(category, text, stack_level);
    Py_DECREF(message);
    return result;
}
