/* One physical C-callable layout and calling-convention authority for both
 * Molt Python.h transports. Include after PyObject, PyTypeObject,
 * Py_ssize_t, and vectorcallfunc are defined. */
#ifndef MOLT_CFUNCTION_ABI_H
#define MOLT_CFUNCTION_ABI_H

typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef PyObject *(*PyCFunctionWithKeywords)(PyObject *, PyObject *, PyObject *);
typedef PyObject *(*_PyCFunctionFast)(PyObject *, PyObject *const *, Py_ssize_t);
typedef PyObject *(*_PyCFunctionFastWithKeywords)(PyObject *, PyObject *const *, Py_ssize_t, PyObject *);
typedef PyObject *(*PyCMethod)(PyObject *, PyTypeObject *, PyObject *const *, size_t, PyObject *);

#define METH_VARARGS  0x0001
#define METH_KEYWORDS 0x0002
#define METH_NOARGS   0x0004
#define METH_O        0x0008
#define METH_CLASS    0x0010
#define METH_STATIC   0x0020
#define METH_COEXIST  0x0040
#define METH_FASTCALL 0x0080
#define METH_METHOD   0x0200

typedef struct PyMethodDef {
    const char *ml_name;
    PyCFunction ml_meth;
    int ml_flags;
    const char *ml_doc;
} PyMethodDef;

#define PY_METHODDEF_SENTINEL { NULL, NULL, 0, NULL }

typedef struct {
    PyObject_HEAD
    PyMethodDef *m_ml;
    PyObject *m_self;
    PyObject *m_module;
    PyObject *m_weakreflist;
    vectorcallfunc vectorcall;
} PyCFunctionObject;

typedef struct {
    PyCFunctionObject func;
    PyTypeObject *mm_class;
} PyCMethodObject;

#endif /* MOLT_CFUNCTION_ABI_H */
