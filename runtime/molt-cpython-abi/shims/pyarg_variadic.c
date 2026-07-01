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
#include <math.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Forward declarations for Rust-implemented helpers. */
typedef struct _object PyObject;
typedef ptrdiff_t Py_ssize_t;

/* Maximum number of output pointers PyArg_ParseTuple can write. */
#define MOLT_PYARG_MAX_OUTS 32
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
extern PyObject *PyBool_FromLong(long value);
extern PyObject *PyUnicode_FromString(const char *s);
extern PyObject *PyUnicode_FromStringAndSize(const char *s, Py_ssize_t size);
extern const char *PyUnicode_AsUTF8(PyObject *op);
extern PyObject *PyObject_Repr(PyObject *op);
extern PyObject *PyObject_Str(PyObject *op);
extern PyObject *PyBytes_FromStringAndSize(const char *s, Py_ssize_t size);
extern int PyErr_WarnEx(PyObject *category, const char *message, Py_ssize_t stack_level);
extern void PyErr_SetString(PyObject *exc_type, const char *message);
extern void PyErr_WriteUnraisable(PyObject *obj);
extern void Py_INCREF(PyObject *op);
extern void Py_DECREF(PyObject *op);
extern PyObject Py_None;
extern PyObject PyExc_TypeError;

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
static int count_format_outs(const char *fmt) {
    int count = 0;
    for (const char *p = fmt; *p; p++) {
        char c = *p;
        if (c == ':' || c == ';') break;
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
    (*cursor)++;
    switch (code) {
    case 'O': {
        PyObject *obj = va_arg(*ap, PyObject *);
        if (obj == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue 'O' received NULL");
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
    case 'p':
        return PyBool_FromLong(va_arg(*ap, int) != 0);
    case 's':
    case 'z': {
        const char *text = va_arg(*ap, const char *);
        int has_len = (**cursor == '#');
        if (text == NULL && code == 'z') {
            Py_INCREF(&Py_None);
            return &Py_None;
        }
        if (text == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue string argument is NULL");
            return NULL;
        }
        if (has_len) {
            (*cursor)++;
            Py_ssize_t len = va_arg(*ap, Py_ssize_t);
            return PyUnicode_FromStringAndSize(text, len);
        }
        return PyUnicode_FromString(text);
    }
    case 'y': {
        const char *bytes = va_arg(*ap, const char *);
        int has_len = (**cursor == '#');
        if (bytes == NULL) {
            PyErr_SetString(&PyExc_TypeError, "Py_BuildValue bytes argument is NULL");
            return NULL;
        }
        Py_ssize_t len = has_len ? va_arg(*ap, Py_ssize_t) : (Py_ssize_t)strlen(bytes);
        if (has_len) (*cursor)++;
        return PyBytes_FromStringAndSize(bytes, len);
    }
    default:
        PyErr_SetString(&PyExc_TypeError, "unsupported format unit in Py_BuildValue");
        return NULL;
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

PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...) {
    if (callable == NULL) return NULL;

    PyObject *args = NULL;
    if (format == NULL || format[0] == '\0') {
        args = PyTuple_New(0);
    }
    else {
        va_list ap;
        va_start(ap, format);
        PyObject *built = Py_VaBuildValue(format, ap);
        va_end(ap);
        if (built == NULL) return NULL;

        int top_level_count = molt_callfunction_top_level_item_count(format);
        if (molt_callfunction_format_starts_tuple(format) || top_level_count != 1) {
            args = built;
        }
        else {
            args = PyTuple_Pack(1, built);
            Py_DECREF(built);
        }
    }

    if (args == NULL) return NULL;
    PyObject *result = PyObject_Call(callable, args, NULL);
    Py_DECREF(args);
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

PyObject *PyObject_CallMethod(
    PyObject *callable,
    const char *name,
    const char *format,
    ...)
{
    if (callable == NULL || name == NULL) return NULL;
    if (format != NULL && format[0] != '\0') return NULL;
    PyObject *method = PyObject_GetAttrString(callable, name);
    if (method == NULL) return NULL;
    PyObject *result = PyObject_Call(method, NULL, NULL);
    Py_DECREF(method);
    return result;
}

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} MoltUnicodeFormatBuffer;

static int molt_unicode_format_reserve(MoltUnicodeFormatBuffer *buf, size_t extra) {
    if (extra > (size_t)-1 - buf->len) return 0;
    size_t need = buf->len + extra + 1;
    if (need <= buf->cap) return 1;
    size_t cap = buf->cap == 0 ? 64 : buf->cap;
    while (cap < need) {
        if (cap > (size_t)-1 / 2) return 0;
        cap *= 2;
    }
    char *data = (char *)realloc(buf->data, cap);
    if (data == NULL) return 0;
    buf->data = data;
    buf->cap = cap;
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
    if (text == NULL) text = "(null)";
    return molt_unicode_format_append_bytes(buf, text, strlen(text));
}

static int molt_unicode_format_append_signed(
    MoltUnicodeFormatBuffer *buf,
    long long value)
{
    char scratch[64];
    int n = snprintf(scratch, sizeof(scratch), "%lld", value);
    if (n < 0 || (size_t)n >= sizeof(scratch)) return 0;
    return molt_unicode_format_append_bytes(buf, scratch, (size_t)n);
}

static int molt_unicode_format_append_object(
    MoltUnicodeFormatBuffer *buf,
    PyObject *object,
    int repr)
{
    if (object == NULL) return 0;
    PyObject *rendered = repr ? PyObject_Repr(object) : PyObject_Str(object);
    if (rendered == NULL) return 0;
    const char *text = PyUnicode_AsUTF8(rendered);
    int ok = text != NULL && molt_unicode_format_append_cstr(buf, text);
    Py_DECREF(rendered);
    return ok;
}

static int molt_unicode_format_fill(
    MoltUnicodeFormatBuffer *buf,
    const char *format,
    va_list vargs)
{
    if (format == NULL) return 0;
    va_list ap;
    va_copy(ap, vargs);
    const char *p = format;
    while (*p != '\0') {
        if (*p != '%') {
            if (!molt_unicode_format_append_bytes(buf, p, 1)) goto error;
            p++;
            continue;
        }
        p++;
        if (*p == '%') {
            if (!molt_unicode_format_append_bytes(buf, "%", 1)) goto error;
            p++;
            continue;
        }
        while (*p == '#' || *p == '0' || *p == '-' || *p == ' ' || *p == '+') p++;
        while (*p >= '0' && *p <= '9') p++;
        if (*p == '.') {
            p++;
            while (*p >= '0' && *p <= '9') p++;
        }
        int long_count = 0;
        int size_modifier = 0;
        if (*p == 'z') {
            size_modifier = 1;
            p++;
        }
        else {
            while (*p == 'l') {
                long_count++;
                p++;
            }
        }
        char spec = *p++;
        switch (spec) {
        case 's':
            if (!molt_unicode_format_append_cstr(buf, va_arg(ap, const char *))) goto error;
            break;
        case 'S':
            if (!molt_unicode_format_append_object(buf, va_arg(ap, PyObject *), 0)) goto error;
            break;
        case 'R':
            if (!molt_unicode_format_append_object(buf, va_arg(ap, PyObject *), 1)) goto error;
            break;
        case 'd':
        case 'i':
            if (size_modifier) {
                if (!molt_unicode_format_append_signed(buf, (long long)va_arg(ap, Py_ssize_t))) goto error;
            }
            else if (long_count >= 2) {
                if (!molt_unicode_format_append_signed(buf, va_arg(ap, long long))) goto error;
            }
            else if (long_count == 1) {
                if (!molt_unicode_format_append_signed(buf, (long long)va_arg(ap, long))) goto error;
            }
            else {
                if (!molt_unicode_format_append_signed(buf, (long long)va_arg(ap, int))) goto error;
            }
            break;
        default:
            goto error;
        }
    }
    va_end(ap);
    return 1;

error:
    va_end(ap);
    return 0;
}

PyObject *PyUnicode_FromFormatV(const char *format, va_list vargs) {
    MoltUnicodeFormatBuffer buf = {0};
    if (!molt_unicode_format_fill(&buf, format, vargs)) {
        free(buf.data);
        return NULL;
    }
    PyObject *result = PyUnicode_FromStringAndSize(buf.data == NULL ? "" : buf.data, (Py_ssize_t)buf.len);
    free(buf.data);
    return result;
}

PyObject *PyUnicode_FromFormat(const char *format, ...) {
    if (format == NULL) return NULL;
    va_list ap;
    va_start(ap, format);
    PyObject *result = PyUnicode_FromFormatV(format, ap);
    va_end(ap);
    return result;
}

PyObject *PyErr_FormatV(PyObject *type, const char *format, va_list vargs) {
    MoltUnicodeFormatBuffer buf = {0};
    if (molt_unicode_format_fill(&buf, format, vargs)) {
        PyErr_SetString(type, buf.data == NULL ? "" : buf.data);
        free(buf.data);
    }
    else {
        free(buf.data);
        PyErr_SetString(type, format == NULL ? "" : format);
    }
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
    if (format != NULL) {
        va_list ap;
        va_start(ap, format);
        vfprintf(stderr, format, ap);
        fputc('\n', stderr);
        va_end(ap);
    }
    PyErr_WriteUnraisable(NULL);
}

void PySys_WriteStderr(const char *format, ...) {
    if (format == NULL) return;
    va_list ap;
    va_start(ap, format);
    vfprintf(stderr, format, ap);
    va_end(ap);
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
    if (format == NULL) return PyErr_WarnEx(category, "", stack_level);
    return PyErr_WarnEx(category, format, stack_level);
}
