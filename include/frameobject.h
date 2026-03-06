#ifndef MOLT_FRAMEOBJECT_H
#define MOLT_FRAMEOBJECT_H

#include <Python.h>

typedef struct _frame {
    PyObject ob_base;
    PyCodeObject *f_code;
    struct _frame *f_back;
    PyObject *f_locals;
    PyObject *f_globals;
    PyObject *f_builtins;
    int f_lasti;
} PyFrameObject;

static inline int PyFrame_FastToLocalsWithError(PyFrameObject *frame) {
    (void)frame;
    return 0;
}

static inline void PyFrame_FastToLocals(PyFrameObject *frame) {
    (void)frame;
}

#endif
