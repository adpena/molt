/* One linkable CPython module/C-callable entry-point surface for the source
 * and linked Molt headers. Include after PyModuleDef and _cfunction_abi.h.
 * These declarations never install a source-header implementation fallback. */
#ifndef MOLT_MODULE_CALLABLE_EXPORTS_H
#define MOLT_MODULE_CALLABLE_EXPORTS_H

#include "_c_api_linkage.h"

PyAPI_DATA(PyTypeObject) PyModule_Type;
PyAPI_DATA(PyTypeObject) PyModuleDef_Type;
PyAPI_DATA(PyTypeObject) PyCFunction_Type;
PyAPI_DATA(PyTypeObject) PyCMethod_Type;

extern PyObject *PyModule_New(const char *name);
extern PyObject *PyModule_NewObject(PyObject *name);
extern PyObject *PyModule_Create2(PyModuleDef *def, int module_api_version);
extern PyObject *PyModuleDef_Init(PyModuleDef *def);
extern PyObject *PyModule_FromDefAndSpec2(PyModuleDef *def, PyObject *spec, int module_api_version);
extern PyObject *PyModule_FromDefAndSpec(PyModuleDef *def, PyObject *spec);
extern int PyModule_ExecDef(PyObject *module, PyModuleDef *def);
extern int PyUnstable_Module_SetGIL(PyObject *module, void *gil);

extern int PyModule_Check(PyObject *module);
extern int PyModule_CheckExact(PyObject *module);
extern PyObject *PyModule_GetDict(PyObject *module);
extern PyModuleDef *PyModule_GetDef(PyObject *module);
extern void *PyModule_GetState(PyObject *module);
extern const char *PyModule_GetName(PyObject *module);
extern PyObject *PyModule_GetNameObject(PyObject *module);
extern const char *PyModule_GetFilename(PyObject *module);
extern PyObject *PyModule_GetFilenameObject(PyObject *module);
extern int PyModule_SetDocString(PyObject *module, const char *docstring);
extern PyObject *PyModule_GetObject(PyObject *module, const char *name);
extern int PyModule_AddFunctions(PyObject *module, PyMethodDef *functions);
extern int PyModule_AddObjectRef(PyObject *module, const char *name, PyObject *value);
extern int PyModule_AddObject(PyObject *module, const char *name, PyObject *value);
extern int PyModule_Add(PyObject *module, const char *name, PyObject *value);
extern int PyModule_AddType(PyObject *module, PyTypeObject *type);
extern int PyModule_AddIntConstant(PyObject *module, const char *name, long value);
extern int PyModule_AddStringConstant(PyObject *module, const char *name, const char *value);
extern int PyState_AddModule(PyObject *module, PyModuleDef *def);
extern PyObject *PyState_FindModule(PyModuleDef *def);
extern int PyState_RemoveModule(PyModuleDef *def);

extern PyObject *PyCFunction_New(PyMethodDef *ml, PyObject *self);
extern PyObject *PyCFunction_NewEx(PyMethodDef *ml, PyObject *self, PyObject *module);
extern PyObject *PyCMethod_New(PyMethodDef *ml, PyObject *self, PyObject *module, PyTypeObject *cls);
extern int PyCFunction_Check(PyObject *op);
extern PyCFunction PyCFunction_GetFunction(PyObject *op);
extern PyObject *PyCFunction_GetSelf(PyObject *op);
extern int PyCFunction_GetFlags(PyObject *op);

#define PyModule_Create(def) PyModule_Create2((def), 1013)
#define PyCFunction_GET_FUNCTION(op) PyCFunction_GetFunction((PyObject *)(op))
#define PyCFunction_GET_SELF(op) PyCFunction_GetSelf((PyObject *)(op))
#define PyCFunction_GET_FLAGS(op) PyCFunction_GetFlags((PyObject *)(op))

#endif /* MOLT_MODULE_CALLABLE_EXPORTS_H */
