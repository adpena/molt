/* Shared declarations for the runtime-owned GC allocation/control family. */
#ifndef MOLT_GC_EXPORTS_H
#define MOLT_GC_EXPORTS_H

extern PyObject *_PyObject_GC_New(PyTypeObject *type);
extern PyVarObject *_PyObject_GC_NewVar(PyTypeObject *type, Py_ssize_t size);
extern void PyObject_GC_Track(void *obj);
extern void PyObject_GC_UnTrack(void *obj);
extern void PyObject_GC_Del(void *obj);
extern int PyObject_GC_IsTracked(PyObject *obj);
extern int PyObject_GC_IsFinalized(PyObject *obj);
extern Py_ssize_t PyGC_Collect(void);
extern int PyGC_Enable(void);
extern int PyGC_Disable(void);
extern int PyGC_IsEnabled(void);

#ifdef MOLT_EXTENSION_HOST_ABI
#define _PyObject_GC_New ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("_PyObject_GC_New"))
#define _PyObject_GC_NewVar ((PyVarObject *(*)(PyTypeObject *, Py_ssize_t))_molt_host_abi_symbol("_PyObject_GC_NewVar"))
#define PyObject_GC_Track ((void (*)(void *))_molt_host_abi_symbol("PyObject_GC_Track"))
#define PyObject_GC_UnTrack ((void (*)(void *))_molt_host_abi_symbol("PyObject_GC_UnTrack"))
#define PyObject_GC_Del ((void (*)(void *))_molt_host_abi_symbol("PyObject_GC_Del"))
#define PyObject_GC_IsTracked ((int (*)(PyObject *))_molt_host_abi_symbol("PyObject_GC_IsTracked"))
#define PyObject_GC_IsFinalized ((int (*)(PyObject *))_molt_host_abi_symbol("PyObject_GC_IsFinalized"))
#define PyGC_Collect ((Py_ssize_t (*)(void))_molt_host_abi_symbol("PyGC_Collect"))
#define PyGC_Enable ((int (*)(void))_molt_host_abi_symbol("PyGC_Enable"))
#define PyGC_Disable ((int (*)(void))_molt_host_abi_symbol("PyGC_Disable"))
#define PyGC_IsEnabled ((int (*)(void))_molt_host_abi_symbol("PyGC_IsEnabled"))
#endif

#define PyObject_GC_New(type, typeobj) ((type *)_PyObject_GC_New((PyTypeObject *)(typeobj)))
#define PyObject_GC_NewVar(type, typeobj, n) ((type *)_PyObject_GC_NewVar((PyTypeObject *)(typeobj), (n)))

#endif /* MOLT_GC_EXPORTS_H */
