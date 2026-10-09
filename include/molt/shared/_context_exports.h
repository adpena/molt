/* Context APIs share one runtime owner across both installed transports. */
#ifndef MOLT_CONTEXT_EXPORTS_H
#define MOLT_CONTEXT_EXPORTS_H

PyAPI_DATA(PyTypeObject) PyContext_Type;
PyAPI_DATA(PyTypeObject) PyContextVar_Type;
PyAPI_DATA(PyTypeObject) PyContextToken_Type;
extern PyObject * PyContext_New(void);
extern PyObject * PyContext_Copy(PyObject *);
extern PyObject * PyContext_CopyCurrent(void);
extern int PyContext_Enter(PyObject *);
extern int PyContext_Exit(PyObject *);
extern int PyContext_CheckExact(PyObject *);
extern int PyContextVar_CheckExact(PyObject *);
extern int PyContextToken_CheckExact(PyObject *);
extern PyObject * PyContextVar_New(const char *, PyObject *);
extern int PyContextVar_Get(PyObject *, PyObject *, PyObject **);
extern PyObject * PyContextVar_Set(PyObject *, PyObject *);
extern int PyContextVar_Reset(PyObject *, PyObject *);

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyContext_New ((PyObject * (*)(void))_molt_host_abi_symbol("PyContext_New"))
#define PyContext_Copy ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyContext_Copy"))
#define PyContext_CopyCurrent ((PyObject * (*)(void))_molt_host_abi_symbol("PyContext_CopyCurrent"))
#define PyContext_Enter ((int (*)(PyObject *))_molt_host_abi_symbol("PyContext_Enter"))
#define PyContext_Exit ((int (*)(PyObject *))_molt_host_abi_symbol("PyContext_Exit"))
#define PyContext_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyContext_CheckExact"))
#define PyContextVar_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyContextVar_CheckExact"))
#define PyContextToken_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyContextToken_CheckExact"))
#define PyContextVar_New ((PyObject * (*)(const char *, PyObject *))_molt_host_abi_symbol("PyContextVar_New"))
#define PyContextVar_Get ((int (*)(PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyContextVar_Get"))
#define PyContextVar_Set ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyContextVar_Set"))
#define PyContextVar_Reset ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyContextVar_Reset"))
#define PyContext_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyContext_Type"))
#define PyContextVar_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyContextVar_Type"))
#define PyContextToken_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyContextToken_Type"))
#endif

#endif /* MOLT_CONTEXT_EXPORTS_H */
