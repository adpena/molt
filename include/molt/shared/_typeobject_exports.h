/* The linked ABI owns physical type construction, readiness and observation.
 * Include after PyObject, PyTypeObject, PyModuleDef and _type_spec_abi.h. */
#ifndef MOLT_TYPEOBJECT_EXPORTS_H
#define MOLT_TYPEOBJECT_EXPORTS_H

extern int PyType_Ready(PyTypeObject *type);
extern PyObject *PyType_FromSpec(PyType_Spec *spec);
extern PyObject *PyType_FromSpecWithBases(PyType_Spec *spec, PyObject *bases);
extern PyObject *PyType_FromModuleAndSpec(PyObject *module, PyType_Spec *spec, PyObject *bases);
extern PyObject *PyType_FromMetaclass(PyTypeObject *metaclass, PyObject *module, PyType_Spec *spec, PyObject *bases);
extern unsigned long PyType_GetFlags(PyTypeObject *type);
extern void *PyType_GetSlot(PyTypeObject *type, int slot);
extern int PyType_HasFeature(PyTypeObject *type, unsigned long feature);
/* Dictionary and name results are new references; module results are borrowed. */
extern PyObject *PyType_GetDict(PyTypeObject *type);
extern PyObject *PyType_GetName(PyTypeObject *type);
extern PyObject *PyType_GetQualName(PyTypeObject *type);
extern PyObject *PyType_GetModule(PyTypeObject *type);
extern void *PyType_GetModuleState(PyTypeObject *type);
extern PyObject *PyType_GetModuleByDef(PyTypeObject *type, PyModuleDef *def);

typedef int (*PyType_WatchCallback)(PyObject *type);
extern void PyType_Modified(PyTypeObject *type);
extern int PyType_AddWatcher(PyType_WatchCallback callback);
extern int PyType_ClearWatcher(int watcher_id);
extern int PyType_Watch(int watcher_id, PyObject *type);
extern int PyType_Unwatch(int watcher_id, PyObject *type);
extern int PyUnstable_Type_AssignVersionTag(PyTypeObject *type);

/* Host loading resolves this same ABI image. This is transport only; missing
 * symbols fail closed through the existing loader's symbol authority. */
#ifdef MOLT_EXTENSION_HOST_ABI
#define PyType_Ready ((int (*)(PyTypeObject *))_molt_host_abi_symbol("PyType_Ready"))
#define PyType_FromSpec ((PyObject *(*)(PyType_Spec *))_molt_host_abi_symbol("PyType_FromSpec"))
#define PyType_FromSpecWithBases ((PyObject *(*)(PyType_Spec *, PyObject *))_molt_host_abi_symbol("PyType_FromSpecWithBases"))
#define PyType_FromModuleAndSpec ((PyObject *(*)(PyObject *, PyType_Spec *, PyObject *))_molt_host_abi_symbol("PyType_FromModuleAndSpec"))
#define PyType_FromMetaclass ((PyObject *(*)(PyTypeObject *, PyObject *, PyType_Spec *, PyObject *))_molt_host_abi_symbol("PyType_FromMetaclass"))
#define PyType_GetFlags ((unsigned long (*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetFlags"))
#define PyType_GetSlot ((void *(*)(PyTypeObject *, int))_molt_host_abi_symbol("PyType_GetSlot"))
#define PyType_HasFeature ((int (*)(PyTypeObject *, unsigned long))_molt_host_abi_symbol("PyType_HasFeature"))
#define PyType_GetDict ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetDict"))
#define PyType_GetName ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetName"))
#define PyType_GetQualName ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetQualName"))
#define PyType_GetModule ((PyObject *(*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetModule"))
#define PyType_GetModuleState ((void *(*)(PyTypeObject *))_molt_host_abi_symbol("PyType_GetModuleState"))
#define PyType_GetModuleByDef ((PyObject *(*)(PyTypeObject *, PyModuleDef *))_molt_host_abi_symbol("PyType_GetModuleByDef"))
#define PyType_Modified ((void (*)(PyTypeObject *))_molt_host_abi_symbol("PyType_Modified"))
#define PyType_AddWatcher ((int (*)(PyType_WatchCallback))_molt_host_abi_symbol("PyType_AddWatcher"))
#define PyType_ClearWatcher ((int (*)(int))_molt_host_abi_symbol("PyType_ClearWatcher"))
#define PyType_Watch ((int (*)(int, PyObject *))_molt_host_abi_symbol("PyType_Watch"))
#define PyType_Unwatch ((int (*)(int, PyObject *))_molt_host_abi_symbol("PyType_Unwatch"))
#define PyUnstable_Type_AssignVersionTag ((int (*)(PyTypeObject *))_molt_host_abi_symbol("PyUnstable_Type_AssignVersionTag"))
#endif

#endif /* MOLT_TYPEOBJECT_EXPORTS_H */
