/* Object inquiry and comparison semantics belong to the linked ABI.
 * Both header transports expose the same functions; host symbol lookup only
 * selects their address. Include after PyObject, PyTypeObject, Py_hash_t
 * and the PyType_Type declaration/projection. */
#ifndef MOLT_OBJECT_OBSERVATION_EXPORTS_H
#define MOLT_OBJECT_OBSERVATION_EXPORTS_H

extern int PyType_Check(PyObject *object);
extern int PyType_IsSubtype(PyTypeObject *derived, PyTypeObject *base);
extern PyObject * PyObject_Type(PyObject *object);
extern int PyObject_TypeCheck(PyObject *object, PyTypeObject *type);
extern int PyObject_IsInstance(PyObject *instance, PyObject *classinfo);
extern int PyObject_IsSubclass(PyObject *derived, PyObject *classinfo);
extern int PyCallable_Check(PyObject *object);
extern Py_hash_t PyObject_Hash(PyObject *object);
extern Py_hash_t PyObject_HashNotImplemented(PyObject *object);
extern int PyObject_IsTrue(PyObject *object);
extern int PyObject_Not(PyObject *object);
extern PyObject * PyObject_RichCompare(PyObject *left, PyObject *right, int op);
extern int PyObject_RichCompareBool(PyObject *left, PyObject *right, int op);
extern PyObject * PyObject_Str(PyObject *object);
extern PyObject * PyObject_Repr(PyObject *object);
extern Py_ssize_t PyObject_Length(PyObject *object);
extern Py_ssize_t PyObject_Size(PyObject *object);
extern PyObject * PyObject_Bytes(PyObject *object);
extern PyObject * PyObject_Format(PyObject *object, PyObject *spec);
extern PyObject * PyObject_Dir(PyObject *object);

extern PyTypeObject *_Py_TYPE(PyObject *object);
#ifdef MOLT_EXTENSION_HOST_ABI
#define _Py_TYPE ((PyTypeObject *(*)(PyObject *))_molt_host_abi_symbol("_Py_TYPE"))
#endif

/* CPython exact-type inquiry is identity, without metaclass protocols. Py_TYPE
 * layout admission belongs to the linked owner and evaluates once. */
static inline int PyType_CheckExact(PyObject *object) {
    PyTypeObject *type = _Py_TYPE(object);
    return type != NULL && type == &PyType_Type;
}
#define PyType_CheckExact(object) PyType_CheckExact((PyObject *)(object))

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyType_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyType_Check"))
#define PyType_IsSubtype ((int (*)(PyTypeObject *, PyTypeObject *))_molt_host_abi_symbol("PyType_IsSubtype"))
#define PyObject_Type ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_Type"))
#define PyObject_TypeCheck ((int (*)(PyObject *, PyTypeObject *))_molt_host_abi_symbol("PyObject_TypeCheck"))
#define PyObject_IsInstance ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_IsInstance"))
#define PyObject_IsSubclass ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_IsSubclass"))
#define PyCallable_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyCallable_Check"))
#define PyObject_Hash ((Py_hash_t (*)(PyObject *))_molt_host_abi_symbol("PyObject_Hash"))
#define PyObject_HashNotImplemented ((Py_hash_t (*)(PyObject *))_molt_host_abi_symbol("PyObject_HashNotImplemented"))
#define PyObject_IsTrue ((int (*)(PyObject *))_molt_host_abi_symbol("PyObject_IsTrue"))
#define PyObject_Not ((int (*)(PyObject *))_molt_host_abi_symbol("PyObject_Not"))
#define PyObject_RichCompare ((PyObject * (*)(PyObject *, PyObject *, int))_molt_host_abi_symbol("PyObject_RichCompare"))
#define PyObject_RichCompareBool ((int (*)(PyObject *, PyObject *, int))_molt_host_abi_symbol("PyObject_RichCompareBool"))
#define PyObject_Str ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_Str"))
#define PyObject_Repr ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_Repr"))
#define PyObject_Length ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyObject_Length"))
#define PyObject_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyObject_Size"))
#define PyObject_Bytes ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_Bytes"))
#define PyObject_Format ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_Format"))
#define PyObject_Dir ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_Dir"))

#endif

#endif /* MOLT_OBJECT_OBSERVATION_EXPORTS_H */
