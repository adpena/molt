#ifndef MOLT_CPYTHON_ABI_DATETIME_H
#define MOLT_CPYTHON_ABI_DATETIME_H

#include <Python.h>

#ifdef __cplusplus
extern "C" {
#endif

extern PyTypeObject PyDateTime_DateType;
extern PyTypeObject PyDateTime_DateTimeType;
extern PyTypeObject PyDateTime_TimeType;
extern PyTypeObject PyDateTime_DeltaType;
extern PyTypeObject PyDateTime_TZInfoType;
extern PyObject PyDateTime_TimeZone_UTC_Object;

extern PyObject *molt_cpython_abi_date_from_date(
    int year,
    int month,
    int day,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_datetime_from_date_and_time(
    int year,
    int month,
    int day,
    int hour,
    int minute,
    int second,
    int usecond,
    PyObject *tzinfo,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_datetime_from_date_and_time_and_fold(
    int year,
    int month,
    int day,
    int hour,
    int minute,
    int second,
    int usecond,
    PyObject *tzinfo,
    int fold,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_time_from_time(
    int hour,
    int minute,
    int second,
    int usecond,
    PyObject *tzinfo,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_time_from_time_and_fold(
    int hour,
    int minute,
    int second,
    int usecond,
    PyObject *tzinfo,
    int fold,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_delta_from_delta(
    int days,
    int seconds,
    int useconds,
    int normalize,
    PyTypeObject *typeobj);
extern PyObject *molt_cpython_abi_timezone_from_timezone(
    PyObject *offset,
    PyObject *name);
extern PyObject *molt_cpython_abi_datetime_from_timestamp(
    PyObject *typeobj,
    PyObject *args,
    PyObject *kw);
extern PyObject *molt_cpython_abi_date_from_timestamp(
    PyObject *typeobj,
    PyObject *args);

typedef struct {
    PyTypeObject *DateType;
    PyTypeObject *DateTimeType;
    PyTypeObject *TimeType;
    PyTypeObject *DeltaType;
    PyTypeObject *TZInfoType;
    PyObject *TimeZone_UTC;
    PyObject *(*Date_FromDate)(int, int, int, PyTypeObject *);
    PyObject *(*DateTime_FromDateAndTime)(
        int, int, int, int, int, int, int, PyObject *, PyTypeObject *);
    PyObject *(*Time_FromTime)(int, int, int, int, PyObject *, PyTypeObject *);
    PyObject *(*Delta_FromDelta)(int, int, int, int, PyTypeObject *);
    PyObject *(*TimeZone_FromTimeZone)(PyObject *, PyObject *);
    PyObject *(*DateTime_FromTimestamp)(PyObject *, PyObject *, PyObject *);
    PyObject *(*Date_FromTimestamp)(PyObject *, PyObject *);
    PyObject *(*DateTime_FromDateAndTimeAndFold)(
        int, int, int, int, int, int, int, PyObject *, int, PyTypeObject *);
    PyObject *(*Time_FromTimeAndFold)(int, int, int, int, PyObject *, int, PyTypeObject *);
} PyDateTime_CAPI;

#if defined(__GNUC__) || defined(__clang__)
#define _MOLT_CPYTHON_ABI_DATETIME_UNUSED __attribute__((unused))
#else
#define _MOLT_CPYTHON_ABI_DATETIME_UNUSED
#endif

static PyDateTime_CAPI _molt_datetime_capi_singleton _MOLT_CPYTHON_ABI_DATETIME_UNUSED = {
    &PyDateTime_DateType,
    &PyDateTime_DateTimeType,
    &PyDateTime_TimeType,
    &PyDateTime_DeltaType,
    &PyDateTime_TZInfoType,
    &PyDateTime_TimeZone_UTC_Object,
    molt_cpython_abi_date_from_date,
    molt_cpython_abi_datetime_from_date_and_time,
    molt_cpython_abi_time_from_time,
    molt_cpython_abi_delta_from_delta,
    molt_cpython_abi_timezone_from_timezone,
    molt_cpython_abi_datetime_from_timestamp,
    molt_cpython_abi_date_from_timestamp,
    molt_cpython_abi_datetime_from_date_and_time_and_fold,
    molt_cpython_abi_time_from_time_and_fold,
};
static PyDateTime_CAPI *PyDateTimeAPI _MOLT_CPYTHON_ABI_DATETIME_UNUSED = NULL;

static inline void _molt_datetime_import(void) {
    if (PyDateTimeAPI == NULL) {
        PyDateTimeAPI = &_molt_datetime_capi_singleton;
    }
}

#define PyDateTime_IMPORT _molt_datetime_import()

#define PyDate_FromDate(year, month, day) \
    molt_cpython_abi_date_from_date((year), (month), (day), &PyDateTime_DateType)
#define PyDateTime_FromDateAndTime(year, month, day, hour, minute, second, usecond) \
    molt_cpython_abi_datetime_from_date_and_time( \
        (year), (month), (day), (hour), (minute), (second), (usecond), Py_None, \
        &PyDateTime_DateTimeType)
#define PyDateTime_FromDateAndTimeAndFold(year, month, day, hour, minute, second, usecond, fold) \
    molt_cpython_abi_datetime_from_date_and_time_and_fold( \
        (year), (month), (day), (hour), (minute), (second), (usecond), Py_None, (fold), \
        &PyDateTime_DateTimeType)
#define PyTime_FromTime(hour, minute, second, usecond) \
    molt_cpython_abi_time_from_time( \
        (hour), (minute), (second), (usecond), Py_None, &PyDateTime_TimeType)
#define PyTime_FromTimeAndFold(hour, minute, second, usecond, fold) \
    molt_cpython_abi_time_from_time_and_fold( \
        (hour), (minute), (second), (usecond), Py_None, (fold), &PyDateTime_TimeType)
#define PyDelta_FromDSU(days, seconds, useconds) \
    molt_cpython_abi_delta_from_delta((days), (seconds), (useconds), 1, &PyDateTime_DeltaType)
#define PyTimeZone_FromOffset(offset) \
    molt_cpython_abi_timezone_from_timezone((offset), NULL)
#define PyTimeZone_FromOffsetAndName(offset, name) \
    molt_cpython_abi_timezone_from_timezone((offset), (name))
#define PyDateTime_FromTimestamp(args) \
    molt_cpython_abi_datetime_from_timestamp((PyObject *)&PyDateTime_DateTimeType, (args), NULL)
#define PyDate_FromTimestamp(args) \
    molt_cpython_abi_date_from_timestamp((PyObject *)&PyDateTime_DateType, (args))
#define PyDateTime_TimeZone_UTC (&PyDateTime_TimeZone_UTC_Object)

static inline int PyDate_Check(PyObject *obj) {
    return PyObject_TypeCheck(obj, &PyDateTime_DateType);
}

static inline int PyDateTime_Check(PyObject *obj) {
    return PyObject_TypeCheck(obj, &PyDateTime_DateTimeType);
}

static inline int PyTime_Check(PyObject *obj) {
    return PyObject_TypeCheck(obj, &PyDateTime_TimeType);
}

static inline int PyDelta_Check(PyObject *obj) {
    return PyObject_TypeCheck(obj, &PyDateTime_DeltaType);
}

#ifdef __cplusplus
}
#endif

#undef _MOLT_CPYTHON_ABI_DATETIME_UNUSED

#endif
