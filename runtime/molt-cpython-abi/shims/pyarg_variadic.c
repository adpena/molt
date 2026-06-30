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

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Forward declarations for Rust-implemented helpers. */
typedef struct _MoltPyObject PyObject;
typedef ptrdiff_t Py_ssize_t;

extern PyObject PyExc_TypeError;
extern PyObject PyExc_ValueError;
extern PyObject PyExc_NotImplementedError;
extern PyObject Py_None;

extern void PyErr_SetString(PyObject *exc, const char *message);
extern void Py_INCREF(PyObject *op);
extern void Py_DECREF(PyObject *op);
extern PyObject *PyLong_FromLong(long value);
extern PyObject *PyLong_FromLongLong(long long value);
extern PyObject *PyLong_FromUnsignedLong(unsigned long value);
extern PyObject *PyLong_FromUnsignedLongLong(unsigned long long value);
extern PyObject *PyFloat_FromDouble(double value);
extern PyObject *PyBool_FromLong(long value);
extern PyObject *PyUnicode_FromStringAndSize(const char *value, Py_ssize_t size);
extern PyObject *PyBytes_FromStringAndSize(const char *value, Py_ssize_t size);
extern PyObject *PyTuple_New(Py_ssize_t size);
extern int PyTuple_SetItem(PyObject *tuple, Py_ssize_t index, PyObject *value);
extern int PyTuple_Check(PyObject *op);
extern PyObject *PyObject_CallObject(PyObject *callable, PyObject *args);
extern PyObject *PyObject_GetAttrString(PyObject *op, const char *name);

/* Maximum number of output pointers PyArg_ParseTuple can write. */
#define MOLT_PYARG_MAX_OUTS 32

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
 * Stops at '|', ':', ';', or end of string.
 */
static int count_format_outs(const char *fmt) {
    int count = 0;
    for (const char *p = fmt; *p; p++) {
        char c = *p;
        if (c == '|' || c == ':' || c == ';') break;
        switch (c) {
        case 's': case 'z': case 'y':
            count++;
            if (*(p+1) == '#') { count++; p++; } /* s#, y# take two outs */
            break;
        case 'i': case 'l': case 'd': case 'f': case 'O':
        case 'p': case 'n': case 'L': case 'K': case 'H':
        case 'I': case 'k': case 'B': case 'C': case 'b':
            count++;
            break;
        case '(': case ')': case '!': case 'e': case 'w':
            break; /* skip grouping / encoding flags */
        default:
            break;
        }
    }
    return count < MOLT_PYARG_MAX_OUTS ? count : MOLT_PYARG_MAX_OUTS;
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
    void *outs[MOLT_PYARG_MAX_OUTS];
    int n = count_format_outs(format);
    for (int i = 0; i < n; i++) {
        outs[i] = va_arg(ap, void *);
    }
    return molt_pyarg_parse_tuple_inner(args, format, outs, n);
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
    (void)kwargs;
    (void)kwlist;
    va_list ap;
    va_start(ap, kwlist);
    int result = collect_and_dispatch(args, format, ap);
    va_end(ap);
    return result;
}

int PyArg_VaParseTupleAndKeywords(
    PyObject *args,
    PyObject *kwargs,
    const char *format,
    char **kwlist,
    va_list ap)
{
    (void)kwargs;
    (void)kwlist;
    return collect_and_dispatch(args, format, ap);
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
    /* Build a synthetic "OOO..." format with up to `max` entries. */
    char fmt[MOLT_PYARG_MAX_OUTS + 4];
    int take = (int)(max < MOLT_PYARG_MAX_OUTS ? max : MOLT_PYARG_MAX_OUTS);
    int i;
    for (i = 0; i < take; i++) fmt[i] = 'O';
    fmt[i] = '|'; /* mark remaining as optional */
    fmt[i+1] = '\0';

    void *outs[MOLT_PYARG_MAX_OUTS];
    va_list ap;
    va_start(ap, max);
    for (int j = 0; j < take; j++) {
        outs[j] = va_arg(ap, void *);
    }
    va_end(ap);

    return molt_pyarg_parse_tuple_inner(args, fmt, outs, take);
}

int PyOS_snprintf(char *str, size_t size, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    int result = vsnprintf(str, size, format, ap);
    va_end(ap);
    return result;
}

static void molt_set_type_error(const char *message) {
    PyErr_SetString(&PyExc_TypeError, message);
}

static int format_to_buffer(const char *format, va_list ap, char **out, size_t *out_len) {
    char stack_buf[1024];
    va_list copy;
    int needed;
    *out = NULL;
    *out_len = 0;
    if (format == NULL) {
        format = "";
    }
    va_copy(copy, ap);
    needed = vsnprintf(stack_buf, sizeof(stack_buf), format, copy);
    va_end(copy);
    if (needed < 0) {
        PyErr_SetString(&PyExc_ValueError, "failed to format CPython ABI message");
        return 0;
    }
    if ((size_t)needed < sizeof(stack_buf)) {
        char *buf = (char *)malloc((size_t)needed + 1);
        if (buf == NULL) {
            PyErr_SetString(&PyExc_TypeError, "out of memory in Molt CPython ABI");
            return 0;
        }
        memcpy(buf, stack_buf, (size_t)needed + 1);
        *out = buf;
        *out_len = (size_t)needed;
        return 1;
    }
    char *buf = (char *)malloc((size_t)needed + 1);
    if (buf == NULL) {
        PyErr_SetString(&PyExc_TypeError, "out of memory in Molt CPython ABI");
        return 0;
    }
    va_copy(copy, ap);
    (void)vsnprintf(buf, (size_t)needed + 1, format, copy);
    va_end(copy);
    *out = buf;
    *out_len = (size_t)needed;
    return 1;
}

PyObject *PyErr_FormatV(PyObject *exc, const char *format, va_list ap) {
    char *message;
    size_t len;
    (void)len;
    if (!format_to_buffer(format, ap, &message, &len)) {
        return NULL;
    }
    PyErr_SetString(exc != NULL ? exc : &PyExc_TypeError, message);
    free(message);
    return NULL;
}

int PyErr_WarnFormat(
    PyObject *category,
    Py_ssize_t stack_level,
    const char *format,
    ...)
{
    va_list ap;
    char *message;
    size_t len;
    (void)category;
    (void)stack_level;
    va_start(ap, format);
    int ok = format_to_buffer(format, ap, &message, &len);
    va_end(ap);
    if (!ok) {
        return -1;
    }
    free(message);
    return 0;
}

static PyObject *molt_new_none(void) {
    Py_INCREF(&Py_None);
    return &Py_None;
}

struct object_vec {
    PyObject **items;
    size_t len;
    size_t capacity;
};

static void object_vec_dispose(struct object_vec *vec) {
    if (vec->items != NULL) {
        for (size_t i = 0; i < vec->len; i++) {
            if (vec->items[i] != NULL) {
                Py_DECREF(vec->items[i]);
            }
        }
    }
    free(vec->items);
    vec->items = NULL;
    vec->len = 0;
    vec->capacity = 0;
}

static int object_vec_push(struct object_vec *vec, PyObject *item) {
    if (item == NULL) {
        return 0;
    }
    if (vec->len == vec->capacity) {
        size_t new_capacity = vec->capacity == 0 ? 8 : vec->capacity * 2;
        PyObject **grown =
            (PyObject **)realloc(vec->items, new_capacity * sizeof(PyObject *));
        if (grown == NULL) {
            Py_DECREF(item);
            PyErr_SetString(&PyExc_TypeError, "out of memory in Molt CPython ABI");
            return 0;
        }
        vec->items = grown;
        vec->capacity = new_capacity;
    }
    vec->items[vec->len++] = item;
    return 1;
}

static void skip_buildvalue_separators(const char **cursor) {
    while (**cursor == ' ' || **cursor == '\t' || **cursor == '\n' ||
           **cursor == '\r' || **cursor == ',') {
        (*cursor)++;
    }
}

static PyObject *tuple_from_owned_items(struct object_vec *vec) {
    PyObject *tuple = PyTuple_New((Py_ssize_t)vec->len);
    if (tuple == NULL) {
        return NULL;
    }
    for (size_t i = 0; i < vec->len; i++) {
        PyObject *item = vec->items[i];
        vec->items[i] = NULL;
        if (PyTuple_SetItem(tuple, (Py_ssize_t)i, item) < 0) {
            Py_DECREF(item);
            Py_DECREF(tuple);
            return NULL;
        }
    }
    return tuple;
}

static PyObject *parse_buildvalue_item(const char **cursor, va_list *ap);

static PyObject *parse_buildvalue_tuple(const char **cursor, va_list *ap) {
    struct object_vec vec = {0};
    PyObject *tuple = NULL;
    for (;;) {
        PyObject *item;
        skip_buildvalue_separators(cursor);
        if (**cursor == ')') {
            (*cursor)++;
            tuple = tuple_from_owned_items(&vec);
            object_vec_dispose(&vec);
            return tuple;
        }
        if (**cursor == '\0') {
            object_vec_dispose(&vec);
            molt_set_type_error("unterminated tuple format in Py_BuildValue");
            return NULL;
        }
        item = parse_buildvalue_item(cursor, ap);
        if (item == NULL || !object_vec_push(&vec, item)) {
            object_vec_dispose(&vec);
            return NULL;
        }
    }
}

static PyObject *parse_buildvalue_item(const char **cursor, va_list *ap) {
    char code;
    skip_buildvalue_separators(cursor);
    code = **cursor;
    if (code == '\0') {
        molt_set_type_error("unexpected end of format in Py_BuildValue");
        return NULL;
    }
    (*cursor)++;

    if (code == '(') {
        return parse_buildvalue_tuple(cursor, ap);
    }
    if (code == '[' || code == '{') {
        molt_set_type_error("list and dict Py_BuildValue formats need Molt container ABI custody");
        return NULL;
    }

    switch (code) {
    case 'O':
    case 'S':
    case 'U': {
        PyObject *obj = va_arg(*ap, PyObject *);
        if (obj == NULL) {
            molt_set_type_error("Py_BuildValue object argument must not be NULL");
            return NULL;
        }
        Py_INCREF(obj);
        return obj;
    }
    case 'N': {
        PyObject *obj = va_arg(*ap, PyObject *);
        if (obj == NULL) {
            molt_set_type_error("Py_BuildValue 'N' argument must not be NULL");
            return NULL;
        }
        return obj;
    }
    case 'i':
    case 'b':
    case 'B':
    case 'h':
    case 'H':
        return PyLong_FromLong((long)va_arg(*ap, int));
    case 'l':
        return PyLong_FromLong(va_arg(*ap, long));
    case 'n':
        return PyLong_FromLongLong((long long)va_arg(*ap, Py_ssize_t));
    case 'k':
        return PyLong_FromUnsignedLong(va_arg(*ap, unsigned long));
    case 'K':
        return PyLong_FromUnsignedLongLong(va_arg(*ap, unsigned long long));
    case 'L':
        return PyLong_FromLongLong(va_arg(*ap, long long));
    case 'd':
    case 'f':
        return PyFloat_FromDouble(va_arg(*ap, double));
    case 'p':
        return PyBool_FromLong((long)(va_arg(*ap, int) != 0));
    case 'c': {
        unsigned char ch = (unsigned char)va_arg(*ap, int);
        return PyBytes_FromStringAndSize((const char *)&ch, 1);
    }
    case 's':
    case 'z':
    case 'y': {
        const char *text = va_arg(*ap, const char *);
        int has_length = (**cursor == '#');
        Py_ssize_t len;
        if (has_length) {
            (*cursor)++;
            len = va_arg(*ap, Py_ssize_t);
            if (len < 0) {
                PyErr_SetString(&PyExc_ValueError, "negative length in Py_BuildValue");
                return NULL;
            }
        } else {
            len = text == NULL ? 0 : (Py_ssize_t)strlen(text);
        }
        if (text == NULL && code == 'z') {
            return molt_new_none();
        }
        if (text == NULL) {
            molt_set_type_error("Py_BuildValue string argument must not be NULL");
            return NULL;
        }
        if (code == 'y') {
            return PyBytes_FromStringAndSize(text, len);
        }
        return PyUnicode_FromStringAndSize(text, len);
    }
    default: {
        char message[128];
        snprintf(
            message,
            sizeof(message),
            "unsupported Py_BuildValue format unit '%c' in Molt CPython ABI",
            code);
        molt_set_type_error(message);
        return NULL;
    }
    }
}

static PyObject *buildvalue_from_va_list(const char *format, va_list *ap) {
    const char *cursor;
    struct object_vec vec = {0};
    PyObject *result = NULL;
    if (format == NULL) {
        molt_set_type_error("Py_BuildValue format must not be NULL");
        return NULL;
    }
    cursor = format;
    for (;;) {
        PyObject *item;
        skip_buildvalue_separators(&cursor);
        if (*cursor == '\0') {
            break;
        }
        item = parse_buildvalue_item(&cursor, ap);
        if (item == NULL || !object_vec_push(&vec, item)) {
            object_vec_dispose(&vec);
            return NULL;
        }
    }
    if (vec.len == 0) {
        object_vec_dispose(&vec);
        return molt_new_none();
    }
    if (vec.len == 1) {
        result = vec.items[0];
        vec.items[0] = NULL;
        object_vec_dispose(&vec);
        return result;
    }
    result = tuple_from_owned_items(&vec);
    object_vec_dispose(&vec);
    return result;
}

PyObject *Py_BuildValue(const char *format, ...) {
    va_list ap;
    PyObject *result;
    va_start(ap, format);
    result = buildvalue_from_va_list(format, &ap);
    va_end(ap);
    return result;
}

static PyObject *call_with_format_args(
    PyObject *callable,
    const char *format,
    va_list *ap)
{
    PyObject *args_obj;
    PyObject *call_args;
    PyObject *result;
    if (callable == NULL) {
        molt_set_type_error("callable must not be NULL");
        return NULL;
    }
    if (format == NULL || format[0] == '\0') {
        return PyObject_CallObject(callable, NULL);
    }
    args_obj = buildvalue_from_va_list(format, ap);
    if (args_obj == NULL) {
        return NULL;
    }
    if (PyTuple_Check(args_obj)) {
        call_args = args_obj;
        Py_INCREF(call_args);
    } else {
        struct object_vec vec = {0};
        Py_INCREF(args_obj);
        if (!object_vec_push(&vec, args_obj)) {
            Py_DECREF(args_obj);
            return NULL;
        }
        call_args = tuple_from_owned_items(&vec);
        object_vec_dispose(&vec);
        if (call_args == NULL) {
            Py_DECREF(args_obj);
            return NULL;
        }
    }
    result = PyObject_CallObject(callable, call_args);
    Py_DECREF(call_args);
    Py_DECREF(args_obj);
    return result;
}

PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...) {
    va_list ap;
    PyObject *result;
    va_start(ap, format);
    result = call_with_format_args(callable, format, &ap);
    va_end(ap);
    return result;
}

PyObject *PyObject_CallMethod(
    PyObject *obj,
    const char *name,
    const char *format,
    ...)
{
    va_list ap;
    PyObject *method;
    PyObject *result;
    if (obj == NULL || name == NULL) {
        molt_set_type_error("object and method name must not be NULL");
        return NULL;
    }
    method = PyObject_GetAttrString(obj, name);
    if (method == NULL) {
        return NULL;
    }
    va_start(ap, format);
    result = call_with_format_args(method, format, &ap);
    va_end(ap);
    Py_DECREF(method);
    return result;
}

PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...) {
    va_list ap;
    struct object_vec vec = {0};
    PyObject *args;
    PyObject *result;
    if (callable == NULL) {
        molt_set_type_error("callable must not be NULL");
        return NULL;
    }
    va_start(ap, callable);
    for (;;) {
        PyObject *arg = va_arg(ap, PyObject *);
        if (arg == NULL) {
            break;
        }
        Py_INCREF(arg);
        if (!object_vec_push(&vec, arg)) {
            va_end(ap);
            object_vec_dispose(&vec);
            return NULL;
        }
    }
    va_end(ap);
    args = tuple_from_owned_items(&vec);
    object_vec_dispose(&vec);
    if (args == NULL) {
        return NULL;
    }
    result = PyObject_CallObject(callable, args);
    Py_DECREF(args);
    return result;
}

static int append_bytes(char **buf, size_t *len, size_t *cap, const char *src, size_t n) {
    if (*len + n + 1 > *cap) {
        size_t new_cap = *cap == 0 ? 128 : *cap;
        while (*len + n + 1 > new_cap) {
            new_cap *= 2;
        }
        char *grown = (char *)realloc(*buf, new_cap);
        if (grown == NULL) {
            PyErr_SetString(&PyExc_TypeError, "out of memory in PyUnicode_FromFormat");
            return 0;
        }
        *buf = grown;
        *cap = new_cap;
    }
    memcpy(*buf + *len, src, n);
    *len += n;
    (*buf)[*len] = '\0';
    return 1;
}

static int append_cstr(char **buf, size_t *len, size_t *cap, const char *src) {
    return append_bytes(buf, len, cap, src == NULL ? "(null)" : src, src == NULL ? 6 : strlen(src));
}

static int append_formatted_int(
    char **buf,
    size_t *len,
    size_t *cap,
    const char *fmt,
    long long value)
{
    char tmp[64];
    int n = snprintf(tmp, sizeof(tmp), fmt, value);
    return n >= 0 && append_bytes(buf, len, cap, tmp, (size_t)n);
}

static int append_formatted_uint(
    char **buf,
    size_t *len,
    size_t *cap,
    const char *fmt,
    unsigned long long value)
{
    char tmp[64];
    int n = snprintf(tmp, sizeof(tmp), fmt, value);
    return n >= 0 && append_bytes(buf, len, cap, tmp, (size_t)n);
}

static PyObject *unicode_from_format_v(const char *format, va_list *ap) {
    char *buf = NULL;
    size_t len = 0;
    size_t cap = 0;
    const char *p;
    PyObject *result;
    if (format == NULL) {
        molt_set_type_error("format must not be NULL");
        return NULL;
    }
    for (p = format; *p != '\0'; p++) {
        if (*p != '%') {
            if (!append_bytes(&buf, &len, &cap, p, 1)) {
                free(buf);
                return NULL;
            }
            continue;
        }
        p++;
        if (*p == '%') {
            if (!append_bytes(&buf, &len, &cap, "%", 1)) {
                free(buf);
                return NULL;
            }
            continue;
        }

        int long_count = 0;
        int ssize = 0;
        if (*p == 'z') {
            ssize = 1;
            p++;
        } else {
            while (*p == 'l' && long_count < 2) {
                long_count++;
                p++;
            }
        }

        switch (*p) {
        case 's':
            if (!append_cstr(&buf, &len, &cap, va_arg(*ap, const char *))) {
                free(buf);
                return NULL;
            }
            break;
        case 'd':
        case 'i':
            if (ssize) {
                if (!append_formatted_int(&buf, &len, &cap, "%lld", (long long)va_arg(*ap, Py_ssize_t))) {
                    free(buf);
                    return NULL;
                }
            } else if (long_count == 2) {
                if (!append_formatted_int(&buf, &len, &cap, "%lld", va_arg(*ap, long long))) {
                    free(buf);
                    return NULL;
                }
            } else if (long_count == 1) {
                if (!append_formatted_int(&buf, &len, &cap, "%ld", va_arg(*ap, long))) {
                    free(buf);
                    return NULL;
                }
            } else if (!append_formatted_int(&buf, &len, &cap, "%d", va_arg(*ap, int))) {
                free(buf);
                return NULL;
            }
            break;
        case 'u':
            if (ssize) {
                if (!append_formatted_uint(&buf, &len, &cap, "%llu", (unsigned long long)va_arg(*ap, Py_ssize_t))) {
                    free(buf);
                    return NULL;
                }
            } else if (long_count == 2) {
                if (!append_formatted_uint(&buf, &len, &cap, "%llu", va_arg(*ap, unsigned long long))) {
                    free(buf);
                    return NULL;
                }
            } else if (long_count == 1) {
                if (!append_formatted_uint(&buf, &len, &cap, "%lu", va_arg(*ap, unsigned long))) {
                    free(buf);
                    return NULL;
                }
            } else if (!append_formatted_uint(&buf, &len, &cap, "%u", va_arg(*ap, unsigned int))) {
                free(buf);
                return NULL;
            }
            break;
        case 'p': {
            char tmp[32];
            int n = snprintf(tmp, sizeof(tmp), "%p", va_arg(*ap, void *));
            if (n < 0 || !append_bytes(&buf, &len, &cap, tmp, (size_t)n)) {
                free(buf);
                return NULL;
            }
            break;
        }
        default: {
            char message[128];
            snprintf(
                message,
                sizeof(message),
                "unsupported PyUnicode_FromFormat unit '%%%c' in Molt CPython ABI",
                *p == '\0' ? '?' : *p);
            molt_set_type_error(message);
            free(buf);
            return NULL;
        }
        }
    }
    if (buf == NULL && !append_bytes(&buf, &len, &cap, "", 0)) {
        return NULL;
    }
    result = PyUnicode_FromStringAndSize(buf, (Py_ssize_t)len);
    free(buf);
    return result;
}

PyObject *PyUnicode_FromFormat(const char *format, ...) {
    va_list ap;
    PyObject *result;
    va_start(ap, format);
    result = unicode_from_format_v(format, &ap);
    va_end(ap);
    return result;
}
