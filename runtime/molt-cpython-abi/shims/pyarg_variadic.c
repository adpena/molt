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
#include <string.h>

/* Forward declarations for Rust-implemented helpers. */
typedef void PyObject;
typedef ptrdiff_t Py_ssize_t;

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

int PyArg_UnpackTuple(
    PyObject *args,
    const char *name,
    Py_ssize_t min,
    Py_ssize_t max,
    ...)
{
    (void)name;
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
