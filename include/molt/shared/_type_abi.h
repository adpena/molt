/* Canonical native C type shells for both distributed header transports.
 * Physical layout follows abi_types.rs (CPython 3.12 layout contract). Runtime
 * values remain behind the existing C-ABI bridge; this is not a Molt object
 * header. Static callers allocate this complete shell, and heap callers see
 * the complete native allocation including its owned protocol tables.
 */
#ifndef MOLT_TYPE_ABI_H
#define MOLT_TYPE_ABI_H

#include "_buffer_abi.h"
#include "_sequence_abi.h"
#include "_sequence_exports.h"

typedef PyObject *(*unaryfunc)   (PyObject *);
typedef PyObject *(*binaryfunc)  (PyObject *, PyObject *);
typedef PyObject *(*ternaryfunc) (PyObject *, PyObject *, PyObject *);
typedef int       (*inquiry)     (PyObject *);
typedef Py_ssize_t(*lenfunc)     (PyObject *);
typedef PyObject *(*ssizeargfunc)(PyObject *, Py_ssize_t);
typedef PyObject *(*ssizessizeargfunc)(PyObject *, Py_ssize_t, Py_ssize_t);
typedef int (*ssizeobjargproc)   (PyObject *, Py_ssize_t, PyObject *);
typedef int (*ssizessizeobjargproc)(PyObject *, Py_ssize_t, Py_ssize_t, PyObject *);
typedef int (*objobjargproc)     (PyObject *, PyObject *, PyObject *);
typedef int (*objobjproc)        (PyObject *, PyObject *);
typedef void(*destructor)        (PyObject *);
typedef PyObject *(*reprfunc)    (PyObject *);
typedef PyObject *(*richcmpfunc) (PyObject *, PyObject *, int);
typedef PyObject *(*getattrofunc)(PyObject *, PyObject *);
typedef int (*setattrofunc)      (PyObject *, PyObject *, PyObject *);
typedef PyObject *(*descrgetfunc)(PyObject *, PyObject *, PyObject *);
typedef int (*descrsetfunc)      (PyObject *, PyObject *, PyObject *);
typedef Py_hash_t (*hashfunc)    (PyObject *);
typedef int (*visitproc)         (PyObject *, void *);
typedef int (*traverseproc)      (PyObject *, visitproc, void *);
typedef PyObject *(*newfunc)     (PyTypeObject *, PyObject *, PyObject *);
typedef PyObject *(*allocfunc)   (PyTypeObject *, Py_ssize_t);
typedef int (*initproc)          (PyObject *, PyObject *, PyObject *);
typedef void (*freefunc)         (void *);
typedef PyObject *(*getiterfunc) (PyObject *);
typedef PyObject *(*iternextfunc)(PyObject *);
typedef PyObject *(*getter)      (PyObject *, void *);
typedef int (*setter)            (PyObject *, PyObject *, void *);
typedef PySendResult (*sendfunc) (PyObject *, PyObject *, PyObject **);
typedef PyObject *(*vectorcallfunc)(PyObject *, PyObject *const *, size_t, PyObject *);

typedef struct PyNumberMethods {
    binaryfunc  nb_add;
    binaryfunc  nb_subtract;
    binaryfunc  nb_multiply;
    binaryfunc  nb_remainder;
    binaryfunc  nb_divmod;
    ternaryfunc nb_power;
    unaryfunc   nb_negative;
    unaryfunc   nb_positive;
    unaryfunc   nb_absolute;
    inquiry     nb_bool;
    unaryfunc   nb_invert;
    binaryfunc  nb_lshift;
    binaryfunc  nb_rshift;
    binaryfunc  nb_and;
    binaryfunc  nb_xor;
    binaryfunc  nb_or;
    unaryfunc   nb_int;
    void       *nb_reserved;
    unaryfunc   nb_float;
    binaryfunc  nb_inplace_add;
    binaryfunc  nb_inplace_subtract;
    binaryfunc  nb_inplace_multiply;
    binaryfunc  nb_inplace_remainder;
    ternaryfunc nb_inplace_power;
    binaryfunc  nb_inplace_lshift;
    binaryfunc  nb_inplace_rshift;
    binaryfunc  nb_inplace_and;
    binaryfunc  nb_inplace_xor;
    binaryfunc  nb_inplace_or;
    binaryfunc  nb_floor_divide;
    binaryfunc  nb_true_divide;
    binaryfunc  nb_inplace_floor_divide;
    binaryfunc  nb_inplace_true_divide;
    unaryfunc   nb_index;
    binaryfunc  nb_matrix_multiply;
    binaryfunc  nb_inplace_matrix_multiply;
} PyNumberMethods;

typedef struct PySequenceMethods {
    lenfunc      sq_length;
    binaryfunc   sq_concat;
    ssizeargfunc sq_repeat;
    ssizeargfunc sq_item;
    void        *was_sq_slice;
    ssizeobjargproc sq_ass_item;
    void        *was_sq_ass_slice;
    objobjproc  sq_contains;
    binaryfunc  sq_inplace_concat;
    ssizeargfunc sq_inplace_repeat;
} PySequenceMethods;

typedef struct PyMappingMethods {
    lenfunc     mp_length;
    binaryfunc  mp_subscript;
    objobjargproc mp_ass_subscript;
} PyMappingMethods;

typedef int (*getbufferproc)(PyObject *, Py_buffer *, int);
typedef void (*releasebufferproc)(PyObject *, Py_buffer *);

typedef struct PyBufferProcs {
    getbufferproc bf_getbuffer;
    releasebufferproc bf_releasebuffer;
} PyBufferProcs;

typedef struct {
    unaryfunc am_await;
    unaryfunc am_aiter;
    unaryfunc am_anext;
    sendfunc am_send;
} PyAsyncMethods;

struct _typeobject {
    /* ob_base (PyVarObject) */
    Py_ssize_t          ob_refcnt;
    PyTypeObject       *ob_type;
    Py_ssize_t          ob_size;

    const char         *tp_name;
    Py_ssize_t          tp_basicsize;
    Py_ssize_t          tp_itemsize;
    destructor          tp_dealloc;
    Py_ssize_t          tp_vectorcall_offset;
    void               *tp_getattr;
    void               *tp_setattr;
    void               *tp_as_async;
    reprfunc            tp_repr;
    PyNumberMethods    *tp_as_number;
    PySequenceMethods  *tp_as_sequence;
    PyMappingMethods   *tp_as_mapping;
    hashfunc            tp_hash;
    ternaryfunc         tp_call;
    reprfunc            tp_str;
    getattrofunc        tp_getattro;
    setattrofunc        tp_setattro;
    PyBufferProcs      *tp_as_buffer;
    unsigned long       tp_flags;
    const char         *tp_doc;
    traverseproc        tp_traverse;
    inquiry             tp_clear;
    richcmpfunc         tp_richcompare;
    Py_ssize_t          tp_weaklistoffset;
    unaryfunc           tp_iter;
    unaryfunc           tp_iternext;
    struct PyMethodDef *tp_methods;
    struct PyMemberDef *tp_members;
    struct PyGetSetDef *tp_getset;
    PyTypeObject       *tp_base;
    PyObject           *tp_dict;
    descrgetfunc        tp_descr_get;
    descrsetfunc        tp_descr_set;
    Py_ssize_t          tp_dictoffset;
    initproc            tp_init;
    allocfunc           tp_alloc;
    newfunc             tp_new;
    freefunc            tp_free;
    inquiry             tp_is_gc;
    PyObject           *tp_bases;
    PyObject           *tp_mro;
    PyObject           *tp_cache;
    void               *tp_subclasses;
    PyObject           *tp_weaklist;
    destructor          tp_del;
    unsigned int        tp_version_tag;
    destructor          tp_finalize;
    void               *tp_vectorcall;
    unsigned char       tp_watched;
};

struct _dictkeysobject;
struct _specialization_cache {
    PyObject *getitem;
    uint32_t getitem_version;
};

typedef struct _heaptypeobject {
    PyTypeObject ht_type;
    PyAsyncMethods as_async;
    PyNumberMethods as_number;
    PyMappingMethods as_mapping;
    PySequenceMethods as_sequence;
    PyBufferProcs as_buffer;
    PyObject *ht_name;
    PyObject *ht_slots;
    PyObject *ht_qualname;
    struct _dictkeysobject *ht_cached_keys;
    PyObject *ht_module;
    char *_ht_tpname;
    struct _specialization_cache _spec_cache;
} PyHeapTypeObject;

#include "_molt_type_layout.generated.h"

#endif /* MOLT_TYPE_ABI_H */
