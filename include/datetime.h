#ifndef MOLT_C_API_DATETIME_H
#define MOLT_C_API_DATETIME_H

#include <Python.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * CPython 3.12 datetime C-API (Include/datetime.h) for the source-compat header
 * tier.
 *
 * This was formerly a fail-open, memory-UNSAFE stub: `PyDateTime_CAPI` was a
 * 4-byte integer-placeholder struct (so `PyDateTimeAPI->DateType` read past the
 * struct bounds — OOB), and `PyDate_/PyDateTime_/PyDelta_Check` unconditionally
 * yielded false (wrong branch, silently), with `PyTime_Check` missing entirely.
 *
 * It now mirrors CPython faithfully: the exact 15-field `PyDateTime_CAPI`, and
 * `PyDateTime_IMPORT` resolves `PyDateTimeAPI` from the `datetime.datetime_CAPI`
 * capsule the molt runtime publishes (runtime/molt-cpython-abi/src/api/
 * datetime.rs, exactly as CPython's `_datetimemodule.c` does). The `*_Check`
 * functions perform the real `PyObject_TypeCheck` against the capsule's type
 * objects. NULL-guarded so a caller that skipped `PyDateTime_IMPORT` gets a safe
 * 0 rather than a NULL-deref/OOB read.
 *
 * The struct layout (field order + count) is ABI and MUST stay byte-identical to
 * runtime/molt-cpython-abi/include/datetime.h and the runtime capsule in
 * runtime/molt-cpython-abi/src/api/datetime.rs. datetime *constructors*
 * (PyDate_FromDate, PyDelta_FromDSU, PyDateTime_TimeZone_UTC, ...) are provided
 * by <Python.h> (molt/Python.h) in this tier and are intentionally NOT redefined
 * here.
 */

#if defined(__GNUC__) || defined(__clang__)
#define _MOLT_DATETIME_UNUSED __attribute__((unused))
#else
#define _MOLT_DATETIME_UNUSED
#endif

typedef struct {
    /* type objects */
    PyTypeObject *DateType;
    PyTypeObject *DateTimeType;
    PyTypeObject *TimeType;
    PyTypeObject *DeltaType;
    PyTypeObject *TZInfoType;
    /* singletons */
    PyObject *TimeZone_UTC;
    /* constructors */
    PyObject *(*Date_FromDate)(int, int, int, PyTypeObject *);
    PyObject *(*DateTime_FromDateAndTime)(
        int, int, int, int, int, int, int, PyObject *, PyTypeObject *);
    PyObject *(*Time_FromTime)(int, int, int, int, PyObject *, PyTypeObject *);
    PyObject *(*Delta_FromDelta)(int, int, int, int, PyTypeObject *);
    PyObject *(*TimeZone_FromTimeZone)(PyObject *offset, PyObject *name);
    /* constructors for the DB API */
    PyObject *(*DateTime_FromTimestamp)(PyObject *, PyObject *, PyObject *);
    PyObject *(*Date_FromTimestamp)(PyObject *, PyObject *);
    /* PEP 495 constructors */
    PyObject *(*DateTime_FromDateAndTimeAndFold)(
        int, int, int, int, int, int, int, PyObject *, int, PyTypeObject *);
    PyObject *(*Time_FromTimeAndFold)(
        int, int, int, int, PyObject *, int, PyTypeObject *);
} PyDateTime_CAPI;

#define PyDateTime_CAPSULE_NAME "datetime.datetime_CAPI"

/* The module-owned PyDateTime_CAPI, populated once by PyDateTime_IMPORT. */
static PyDateTime_CAPI *PyDateTimeAPI _MOLT_DATETIME_UNUSED = NULL;

/* CPython: PyDateTimeAPI = (PyDateTime_CAPI *)PyCapsule_Import(name, 0). */
#define PyDateTime_IMPORT \
    (PyDateTimeAPI = (PyDateTime_CAPI *)PyCapsule_Import(PyDateTime_CAPSULE_NAME, 0))

/* Real type checks against the imported capsule's type objects. */
static inline int PyDate_Check(PyObject *op) {
    return PyDateTimeAPI != NULL && PyObject_TypeCheck(op, PyDateTimeAPI->DateType);
}
static inline int PyDate_CheckExact(PyObject *op) {
    return PyDateTimeAPI != NULL && Py_IS_TYPE(op, PyDateTimeAPI->DateType);
}
static inline int PyDateTime_Check(PyObject *op) {
    return PyDateTimeAPI != NULL && PyObject_TypeCheck(op, PyDateTimeAPI->DateTimeType);
}
static inline int PyDateTime_CheckExact(PyObject *op) {
    return PyDateTimeAPI != NULL && Py_IS_TYPE(op, PyDateTimeAPI->DateTimeType);
}
static inline int PyTime_Check(PyObject *op) {
    return PyDateTimeAPI != NULL && PyObject_TypeCheck(op, PyDateTimeAPI->TimeType);
}
static inline int PyTime_CheckExact(PyObject *op) {
    return PyDateTimeAPI != NULL && Py_IS_TYPE(op, PyDateTimeAPI->TimeType);
}
static inline int PyDelta_Check(PyObject *op) {
    return PyDateTimeAPI != NULL && PyObject_TypeCheck(op, PyDateTimeAPI->DeltaType);
}
static inline int PyDelta_CheckExact(PyObject *op) {
    return PyDateTimeAPI != NULL && Py_IS_TYPE(op, PyDateTimeAPI->DeltaType);
}
static inline int PyTZInfo_Check(PyObject *op) {
    return PyDateTimeAPI != NULL && PyObject_TypeCheck(op, PyDateTimeAPI->TZInfoType);
}
static inline int PyTZInfo_CheckExact(PyObject *op) {
    return PyDateTimeAPI != NULL && Py_IS_TYPE(op, PyDateTimeAPI->TZInfoType);
}

#ifdef __cplusplus
}
#endif

#undef _MOLT_DATETIME_UNUSED

#endif /* MOLT_C_API_DATETIME_H */
