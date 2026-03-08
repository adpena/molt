#ifndef MOLT_C_API_DATETIME_H
#define MOLT_C_API_DATETIME_H

#include <Python.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    int _molt_reserved;
} PyDateTime_CAPI;

#if defined(__GNUC__) || defined(__clang__)
#define _MOLT_DATETIME_UNUSED __attribute__((unused))
#else
#define _MOLT_DATETIME_UNUSED
#endif

static PyDateTime_CAPI _molt_datetime_capi_singleton _MOLT_DATETIME_UNUSED = {0};
static PyDateTime_CAPI *PyDateTimeAPI _MOLT_DATETIME_UNUSED = NULL;

static inline void _molt_datetime_import(void) {
    if (PyDateTimeAPI == NULL) {
        PyDateTimeAPI = &_molt_datetime_capi_singleton;
    }
}

#define PyDateTime_IMPORT _molt_datetime_import()

static inline int PyDate_Check(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyDateTime_Check(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyDelta_Check(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline int PyTime_Check(PyObject *obj) {
    (void)obj;
    return 0;
}

static inline PyObject *_molt_datetime_attr(const char *name) {
    PyObject *datetime_module;
    PyObject *attr;
    datetime_module = PyImport_ImportModule("datetime");
    if (datetime_module == NULL) {
        return NULL;
    }
    attr = PyObject_GetAttrString(datetime_module, name);
    Py_DECREF(datetime_module);
    return attr;
}

static inline PyObject *PyDate_FromDate(int year, int month, int day) {
    PyObject *date_type = _molt_datetime_attr("date");
    PyObject *out;
    if (date_type == NULL) {
        return NULL;
    }
    out = PyObject_CallFunction(date_type, "iii", year, month, day);
    Py_DECREF(date_type);
    return out;
}

static inline PyObject *PyDateTime_FromDateAndTime(
    int year,
    int month,
    int day,
    int hour,
    int minute,
    int second,
    int usecond
) {
    PyObject *datetime_type = _molt_datetime_attr("datetime");
    PyObject *out;
    if (datetime_type == NULL) {
        return NULL;
    }
    out = PyObject_CallFunction(
        datetime_type, "iiiiiii", year, month, day, hour, minute, second, usecond);
    Py_DECREF(datetime_type);
    return out;
}

static inline PyObject *PyDelta_FromDSU(int days, int seconds, int useconds) {
    PyObject *timedelta_type = _molt_datetime_attr("timedelta");
    PyObject *out;
    if (timedelta_type == NULL) {
        return NULL;
    }
    out = PyObject_CallFunction(timedelta_type, "iii", days, seconds, useconds);
    Py_DECREF(timedelta_type);
    return out;
}

static inline PyObject *_molt_datetime_timezone_utc(void) {
    PyObject *timezone_type = _molt_datetime_attr("timezone");
    PyObject *utc = NULL;
    if (timezone_type == NULL) {
        return NULL;
    }
    utc = PyObject_GetAttrString(timezone_type, "utc");
    Py_DECREF(timezone_type);
    return utc;
}

#define PyDateTime_TimeZone_UTC _molt_datetime_timezone_utc()

#ifdef __cplusplus
}
#endif

#undef _MOLT_DATETIME_UNUSED

#endif
