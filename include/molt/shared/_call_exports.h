/* One C-ABI owner for object calls, argument parsing and owned value packing. */
#ifndef MOLT_CALL_EXPORTS_H
#define MOLT_CALL_EXPORTS_H

extern int PyArg_Parse(PyObject *arg, const char *format, ...);
extern int PyArg_ParseTuple (PyObject *args, const char *format, ...);
extern int PyArg_ParseTupleAndKeywords (PyObject *args, PyObject *kwds, const char *format, char **kwlist, ...);
extern int PyArg_UnpackTuple (PyObject *args, const char *name, Py_ssize_t min, Py_ssize_t max, ...);
extern int PyArg_VaParseTupleAndKeywords(PyObject *args, PyObject *kwds, const char *format, char **kwlist, va_list vargs);
extern int PyArg_ValidateKeywordArguments(PyObject *kwargs);
extern PyObject *PyObject_Call (PyObject *callable, PyObject *args, PyObject *kwargs);
extern PyObject *PyObject_CallFunction(PyObject *callable, const char *format, ...);
extern PyObject *PyObject_CallFunctionObjArgs(PyObject *callable, ...);
extern PyObject *PyObject_CallMethod(PyObject *callable, const char *name, const char *format, ...);
extern PyObject *PyObject_CallMethodNoArgs(PyObject *obj, PyObject *name);
extern PyObject *PyObject_CallMethodObjArgs(PyObject *callable, PyObject *name, ...);
extern PyObject *PyObject_CallMethodOneArg(PyObject *obj, PyObject *name, PyObject *arg);
extern PyObject *PyObject_CallNoArgs (PyObject *callable);
extern PyObject *PyObject_CallObject (PyObject *callable, PyObject *args);
extern PyObject *PyObject_CallOneArg (PyObject *callable, PyObject *arg);
extern PyObject *PyObject_Vectorcall (PyObject *callable, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject *PyObject_VectorcallDict(PyObject *callable, PyObject *const *args, size_t nargs, PyObject *kwargs);
extern PyObject *PyObject_VectorcallMethod(PyObject *name, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject *PyVectorcall_Call (PyObject *callable, PyObject *args, PyObject *kwargs);
extern vectorcallfunc PyVectorcall_Function(PyObject *callable);
extern PyObject *Py_BuildValue(const char *format, ...);
extern PyObject *Py_VaBuildValue(const char *format, va_list vargs);
extern int _PyArg_ParseTupleAndKeywords_SizeT(PyObject *args, PyObject *kwargs, const char *format, char **kwlist, ...);
extern int _PyArg_ParseTuple_SizeT(PyObject *args, const char *format, ...);
extern int _PyArg_VaParseTupleAndKeywords_SizeT(PyObject *args, PyObject *kwargs, const char *format, char **kwlist, va_list vargs);
extern int _PyArg_VaParse_SizeT(PyObject *args, const char *format, va_list vargs);
extern PyObject *_PyObject_CallFunction_SizeT(PyObject *callable, const char *format, ...);
extern PyObject *_PyObject_CallMethod_SizeT(PyObject *callable, const char *name, const char *format, ...);
extern PyObject *_PyObject_Vectorcall(PyObject *callable, PyObject *const *args, size_t nargsf, PyObject *kwnames);
extern PyObject *_Py_BuildValue_SizeT(const char *format, ...);


#ifdef MOLT_EXTENSION_HOST_ABI
#define PyArg_Parse ((int (*)(PyObject *, const char *, ...))_molt_host_abi_symbol("PyArg_Parse"))
#define PyArg_ParseTuple ((int (*)(PyObject *, const char *, ...))_molt_host_abi_symbol("PyArg_ParseTuple"))
#define PyArg_ParseTupleAndKeywords ((int (*)(PyObject *, PyObject *, const char *, char **, ...))_molt_host_abi_symbol("PyArg_ParseTupleAndKeywords"))
#define PyArg_UnpackTuple ((int (*)(PyObject *, const char *, Py_ssize_t, Py_ssize_t, ...))_molt_host_abi_symbol("PyArg_UnpackTuple"))
#define PyArg_VaParseTupleAndKeywords ((int (*)(PyObject *, PyObject *, const char *, char **, va_list))_molt_host_abi_symbol("PyArg_VaParseTupleAndKeywords"))
#define PyArg_ValidateKeywordArguments ((int (*)(PyObject *))_molt_host_abi_symbol("PyArg_ValidateKeywordArguments"))
#define PyObject_Call ((PyObject * (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_Call"))
#define PyObject_CallFunction ((PyObject * (*)(PyObject *, const char *, ...))_molt_host_abi_symbol("PyObject_CallFunction"))
#define PyObject_CallFunctionObjArgs ((PyObject * (*)(PyObject *, ...))_molt_host_abi_symbol("PyObject_CallFunctionObjArgs"))
#define PyObject_CallMethod ((PyObject * (*)(PyObject *, const char *, const char *, ...))_molt_host_abi_symbol("PyObject_CallMethod"))
#define PyObject_CallMethodNoArgs ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_CallMethodNoArgs"))
#define PyObject_CallMethodObjArgs ((PyObject * (*)(PyObject *, PyObject *, ...))_molt_host_abi_symbol("PyObject_CallMethodObjArgs"))
#define PyObject_CallMethodOneArg ((PyObject * (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_CallMethodOneArg"))
#define PyObject_CallNoArgs ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyObject_CallNoArgs"))
#define PyObject_CallObject ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_CallObject"))
#define PyObject_CallOneArg ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_CallOneArg"))
#define PyObject_Vectorcall ((PyObject * (*)(PyObject *, PyObject *const *, size_t, PyObject *))_molt_host_abi_symbol("PyObject_Vectorcall"))
#define PyObject_VectorcallDict ((PyObject * (*)(PyObject *, PyObject *const *, size_t, PyObject *))_molt_host_abi_symbol("PyObject_VectorcallDict"))
#define PyObject_VectorcallMethod ((PyObject * (*)(PyObject *, PyObject *const *, size_t, PyObject *))_molt_host_abi_symbol("PyObject_VectorcallMethod"))
#define PyVectorcall_Call ((PyObject * (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyVectorcall_Call"))
#define PyVectorcall_Function ((vectorcallfunc (*)(PyObject *))_molt_host_abi_symbol("PyVectorcall_Function"))
#define Py_BuildValue ((PyObject * (*)(const char *, ...))_molt_host_abi_symbol("Py_BuildValue"))
#define Py_VaBuildValue ((PyObject * (*)(const char *, va_list))_molt_host_abi_symbol("Py_VaBuildValue"))
#define _PyArg_ParseTupleAndKeywords_SizeT ((int (*)(PyObject *, PyObject *, const char *, char **, ...))_molt_host_abi_symbol("_PyArg_ParseTupleAndKeywords_SizeT"))
#define _PyArg_ParseTuple_SizeT ((int (*)(PyObject *, const char *, ...))_molt_host_abi_symbol("_PyArg_ParseTuple_SizeT"))
#define _PyArg_VaParseTupleAndKeywords_SizeT ((int (*)(PyObject *, PyObject *, const char *, char **, va_list))_molt_host_abi_symbol("_PyArg_VaParseTupleAndKeywords_SizeT"))
#define _PyArg_VaParse_SizeT ((int (*)(PyObject *, const char *, va_list))_molt_host_abi_symbol("_PyArg_VaParse_SizeT"))
#define _PyObject_CallFunction_SizeT ((PyObject * (*)(PyObject *, const char *, ...))_molt_host_abi_symbol("_PyObject_CallFunction_SizeT"))
#define _PyObject_CallMethod_SizeT ((PyObject * (*)(PyObject *, const char *, const char *, ...))_molt_host_abi_symbol("_PyObject_CallMethod_SizeT"))
#define _PyObject_Vectorcall ((PyObject * (*)(PyObject *, PyObject *const *, size_t, PyObject *))_molt_host_abi_symbol("_PyObject_Vectorcall"))
#define _Py_BuildValue_SizeT ((PyObject * (*)(const char *, ...))_molt_host_abi_symbol("_Py_BuildValue_SizeT"))

#endif


#endif /* MOLT_CALL_EXPORTS_H */
