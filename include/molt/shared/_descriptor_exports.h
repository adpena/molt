/* Both header transports call the same descriptor and wrapper constructors.
 * Include after _descriptor_abi.h. This surface owns declarations only. */
#ifndef MOLT_DESCRIPTOR_EXPORTS_H
#define MOLT_DESCRIPTOR_EXPORTS_H

#include "_c_api_linkage.h"

PyAPI_DATA(PyTypeObject) PyMethodDescr_Type;
PyAPI_DATA(PyTypeObject) PyClassMethodDescr_Type;
PyAPI_DATA(PyTypeObject) PyMemberDescr_Type;
PyAPI_DATA(PyTypeObject) PyGetSetDescr_Type;
PyAPI_DATA(PyTypeObject) PyWrapperDescr_Type;
PyAPI_DATA(PyTypeObject) _PyMethodWrapper_Type;
PyAPI_DATA(PyTypeObject) PyClassMethod_Type;
PyAPI_DATA(PyTypeObject) PyStaticMethod_Type;

extern int PyDescr_IsData(PyObject *descr);
extern PyObject *PyDescr_NAME(PyObject *descr);
extern PyObject *PyDescr_NewMethod(PyTypeObject *type, PyMethodDef *method);
extern PyObject *PyDescr_NewClassMethod(PyTypeObject *type, PyMethodDef *method);
extern PyObject *PyDescr_NewMember(PyTypeObject *type, PyMemberDef *member);
extern PyObject *PyDescr_NewGetSet(PyTypeObject *type, PyGetSetDef *getset);
extern PyObject *PyDescr_NewWrapper(PyTypeObject *type, struct wrapperbase *base, void *wrapped);
extern PyObject *PyWrapper_New(PyObject *descr, PyObject *self);
extern PyObject *PyMember_GetOne(const char *addr, PyMemberDef *member);
extern int PyMember_SetOne(char *addr, PyMemberDef *member, PyObject *value);
extern PyObject *PyClassMethod_New(PyObject *callable);
extern PyObject *PyStaticMethod_New(PyObject *callable);

#endif /* MOLT_DESCRIPTOR_EXPORTS_H */
