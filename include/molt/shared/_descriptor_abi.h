/* Physical descriptor declarations shared by both Python.h transports.
 * The repr(C) definitions in abi_types.rs own the layout; the generated ABI
 * assertions check this projection. Include after _cfunction_abi.h, getter
 * and setter are defined. */
#ifndef MOLT_DESCRIPTOR_ABI_H
#define MOLT_DESCRIPTOR_ABI_H

typedef struct PyMemberDef {
    const char *name;
    int type;
    Py_ssize_t offset;
    int flags;
    const char *doc;
} PyMemberDef;

typedef struct PyGetSetDef {
    const char *name;
    getter get;
    setter set;
    const char *doc;
    void *closure;
} PyGetSetDef;

typedef struct {
    PyObject_HEAD
    PyTypeObject *d_type;
    PyObject *d_name;
    PyObject *d_qualname;
} PyDescrObject;

typedef struct {
    PyDescrObject d_common;
    PyMethodDef *d_method;
    vectorcallfunc vectorcall;
} PyMethodDescrObject;

typedef struct {
    PyDescrObject d_common;
    PyMemberDef *d_member;
} PyMemberDescrObject;

typedef struct {
    PyDescrObject d_common;
    PyGetSetDef *d_getset;
} PyGetSetDescrObject;

typedef PyObject *(*wrapperfunc)(PyObject *, PyObject *, void *);
typedef PyObject *(*wrapperfunc_kwds)(PyObject *, PyObject *, void *, PyObject *);

typedef struct wrapperbase {
    const char *name;
    int offset;
    void *function;
    wrapperfunc wrapper;
    const char *doc;
    int flags;
    PyObject *name_strobj;
} PyWrapperBase;
#define PyWrapperFlag_KEYWORDS 1

typedef struct {
    PyDescrObject d_common;
    struct wrapperbase *d_base;
    void *d_wrapped;
} PyWrapperDescrObject;

typedef struct {
    PyObject_HEAD
    PyWrapperDescrObject *descr;
    PyObject *self_;
} PyMethodWrapperObject;

#endif /* MOLT_DESCRIPTOR_ABI_H */
