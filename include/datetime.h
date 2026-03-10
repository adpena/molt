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

#ifdef __cplusplus
}
#endif

#undef _MOLT_DATETIME_UNUSED

#endif
