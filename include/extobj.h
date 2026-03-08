#ifndef MOLT_NUMPY_COMPAT_EXTOBJ_H
#define MOLT_NUMPY_COMPAT_EXTOBJ_H

#include <Python.h>
#include <numpy/ndarraytypes.h>

typedef struct {
    int errmask;
    npy_intp bufsize;
    PyObject *pyfunc;
} npy_extobj;

static inline void npy_extobj_clear(npy_extobj *extobj) {
    if (extobj == NULL) {
        return;
    }
    Py_XDECREF(extobj->pyfunc);
}

NPY_NO_EXPORT int _check_ufunc_fperr(int errmask, const char *ufunc_name);
NPY_NO_EXPORT int _get_bufsize_errmask(int *buffersize, int *errormask);
NPY_NO_EXPORT int init_extobj(void);
NPY_NO_EXPORT PyObject *extobj_make_extobj(
    PyObject *mod,
    PyObject *const *args,
    Py_ssize_t len_args,
    PyObject *kwnames
);
NPY_NO_EXPORT PyObject *extobj_get_extobj_dict(PyObject *mod, PyObject *noarg);

#endif
