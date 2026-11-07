/* Both C header transports use the runtime's single marshal format owner. */
#ifndef MOLT_MARSHAL_EXPORTS_H
#define MOLT_MARSHAL_EXPORTS_H

extern PyObject *PyMarshal_WriteObjectToString(PyObject *, int);
extern PyObject *PyMarshal_ReadObjectFromString(const char *, Py_ssize_t);

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyMarshal_WriteObjectToString ((PyObject *(*)(PyObject *, int))_molt_host_abi_symbol("PyMarshal_WriteObjectToString"))
#define PyMarshal_ReadObjectFromString ((PyObject *(*)(const char *, Py_ssize_t))_molt_host_abi_symbol("PyMarshal_ReadObjectFromString"))
#endif

#endif /* MOLT_MARSHAL_EXPORTS_H */
