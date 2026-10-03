/* Shared declarations for the compiled exception-state and attribute owners.
 * Both Python.h transports use the same pending/handled exception state and
 * keep normal attribute dispatch distinct from the generic descriptor path. */
#ifndef MOLT_EXCEPTION_ATTRIBUTE_EXPORTS_H
#define MOLT_EXCEPTION_ATTRIBUTE_EXPORTS_H

#include "_c_api_linkage.h"

/* Builtin exception objects have one process-stable ABI address. Evaluating a
 * PyExc_* name must not allocate, inspect pending errors, or cache a runtime
 * handle. Direct linkage and host lookup address the same object storage;
 * these data exports are objects, not pointer-valued variables. */
PyAPI_DATA(PyObject) PyExc_BaseException;
PyAPI_DATA(PyObject) PyExc_Exception;
PyAPI_DATA(PyObject) PyExc_BaseExceptionGroup;
PyAPI_DATA(PyObject) PyExc_StopAsyncIteration;
PyAPI_DATA(PyObject) PyExc_StopIteration;
PyAPI_DATA(PyObject) PyExc_GeneratorExit;
PyAPI_DATA(PyObject) PyExc_ArithmeticError;
PyAPI_DATA(PyObject) PyExc_LookupError;
PyAPI_DATA(PyObject) PyExc_AssertionError;
PyAPI_DATA(PyObject) PyExc_AttributeError;
PyAPI_DATA(PyObject) PyExc_BufferError;
PyAPI_DATA(PyObject) PyExc_EOFError;
PyAPI_DATA(PyObject) PyExc_FloatingPointError;
PyAPI_DATA(PyObject) PyExc_OSError;
PyAPI_DATA(PyObject) PyExc_ImportError;
PyAPI_DATA(PyObject) PyExc_ModuleNotFoundError;
PyAPI_DATA(PyObject) PyExc_IndexError;
PyAPI_DATA(PyObject) PyExc_KeyError;
PyAPI_DATA(PyObject) PyExc_KeyboardInterrupt;
PyAPI_DATA(PyObject) PyExc_MemoryError;
PyAPI_DATA(PyObject) PyExc_NameError;
PyAPI_DATA(PyObject) PyExc_OverflowError;
PyAPI_DATA(PyObject) PyExc_RuntimeError;
PyAPI_DATA(PyObject) PyExc_RecursionError;
PyAPI_DATA(PyObject) PyExc_NotImplementedError;
PyAPI_DATA(PyObject) PyExc_SyntaxError;
PyAPI_DATA(PyObject) PyExc_IndentationError;
PyAPI_DATA(PyObject) PyExc_TabError;
PyAPI_DATA(PyObject) PyExc_ReferenceError;
PyAPI_DATA(PyObject) PyExc_SystemError;
PyAPI_DATA(PyObject) PyExc_SystemExit;
PyAPI_DATA(PyObject) PyExc_TypeError;
PyAPI_DATA(PyObject) PyExc_UnboundLocalError;
PyAPI_DATA(PyObject) PyExc_UnicodeError;
PyAPI_DATA(PyObject) PyExc_UnicodeEncodeError;
PyAPI_DATA(PyObject) PyExc_UnicodeDecodeError;
PyAPI_DATA(PyObject) PyExc_UnicodeTranslateError;
PyAPI_DATA(PyObject) PyExc_ValueError;
PyAPI_DATA(PyObject) PyExc_ZeroDivisionError;
PyAPI_DATA(PyObject) PyExc_BlockingIOError;
PyAPI_DATA(PyObject) PyExc_BrokenPipeError;
PyAPI_DATA(PyObject) PyExc_ChildProcessError;
PyAPI_DATA(PyObject) PyExc_ConnectionError;
PyAPI_DATA(PyObject) PyExc_ConnectionAbortedError;
PyAPI_DATA(PyObject) PyExc_ConnectionRefusedError;
PyAPI_DATA(PyObject) PyExc_ConnectionResetError;
PyAPI_DATA(PyObject) PyExc_FileExistsError;
PyAPI_DATA(PyObject) PyExc_FileNotFoundError;
PyAPI_DATA(PyObject) PyExc_InterruptedError;
PyAPI_DATA(PyObject) PyExc_IsADirectoryError;
PyAPI_DATA(PyObject) PyExc_NotADirectoryError;
PyAPI_DATA(PyObject) PyExc_PermissionError;
PyAPI_DATA(PyObject) PyExc_ProcessLookupError;
PyAPI_DATA(PyObject) PyExc_TimeoutError;
PyAPI_DATA(PyObject) PyExc_Warning;
PyAPI_DATA(PyObject) PyExc_UserWarning;
PyAPI_DATA(PyObject) PyExc_DeprecationWarning;
PyAPI_DATA(PyObject) PyExc_PendingDeprecationWarning;
PyAPI_DATA(PyObject) PyExc_SyntaxWarning;
PyAPI_DATA(PyObject) PyExc_RuntimeWarning;
PyAPI_DATA(PyObject) PyExc_FutureWarning;
PyAPI_DATA(PyObject) PyExc_ImportWarning;
PyAPI_DATA(PyObject) PyExc_UnicodeWarning;
PyAPI_DATA(PyObject) PyExc_BytesWarning;
PyAPI_DATA(PyObject) PyExc_EncodingWarning;
PyAPI_DATA(PyObject) PyExc_ResourceWarning;

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyExc_BaseException ((PyObject *)_molt_host_abi_symbol("PyExc_BaseException"))
#define PyExc_Exception ((PyObject *)_molt_host_abi_symbol("PyExc_Exception"))
#define PyExc_BaseExceptionGroup ((PyObject *)_molt_host_abi_symbol("PyExc_BaseExceptionGroup"))
#define PyExc_StopAsyncIteration ((PyObject *)_molt_host_abi_symbol("PyExc_StopAsyncIteration"))
#define PyExc_StopIteration ((PyObject *)_molt_host_abi_symbol("PyExc_StopIteration"))
#define PyExc_GeneratorExit ((PyObject *)_molt_host_abi_symbol("PyExc_GeneratorExit"))
#define PyExc_ArithmeticError ((PyObject *)_molt_host_abi_symbol("PyExc_ArithmeticError"))
#define PyExc_LookupError ((PyObject *)_molt_host_abi_symbol("PyExc_LookupError"))
#define PyExc_AssertionError ((PyObject *)_molt_host_abi_symbol("PyExc_AssertionError"))
#define PyExc_AttributeError ((PyObject *)_molt_host_abi_symbol("PyExc_AttributeError"))
#define PyExc_BufferError ((PyObject *)_molt_host_abi_symbol("PyExc_BufferError"))
#define PyExc_EOFError ((PyObject *)_molt_host_abi_symbol("PyExc_EOFError"))
#define PyExc_FloatingPointError ((PyObject *)_molt_host_abi_symbol("PyExc_FloatingPointError"))
#define PyExc_OSError ((PyObject *)_molt_host_abi_symbol("PyExc_OSError"))
#define PyExc_ImportError ((PyObject *)_molt_host_abi_symbol("PyExc_ImportError"))
#define PyExc_ModuleNotFoundError ((PyObject *)_molt_host_abi_symbol("PyExc_ModuleNotFoundError"))
#define PyExc_IndexError ((PyObject *)_molt_host_abi_symbol("PyExc_IndexError"))
#define PyExc_KeyError ((PyObject *)_molt_host_abi_symbol("PyExc_KeyError"))
#define PyExc_KeyboardInterrupt ((PyObject *)_molt_host_abi_symbol("PyExc_KeyboardInterrupt"))
#define PyExc_MemoryError ((PyObject *)_molt_host_abi_symbol("PyExc_MemoryError"))
#define PyExc_NameError ((PyObject *)_molt_host_abi_symbol("PyExc_NameError"))
#define PyExc_OverflowError ((PyObject *)_molt_host_abi_symbol("PyExc_OverflowError"))
#define PyExc_RuntimeError ((PyObject *)_molt_host_abi_symbol("PyExc_RuntimeError"))
#define PyExc_RecursionError ((PyObject *)_molt_host_abi_symbol("PyExc_RecursionError"))
#define PyExc_NotImplementedError ((PyObject *)_molt_host_abi_symbol("PyExc_NotImplementedError"))
#define PyExc_SyntaxError ((PyObject *)_molt_host_abi_symbol("PyExc_SyntaxError"))
#define PyExc_IndentationError ((PyObject *)_molt_host_abi_symbol("PyExc_IndentationError"))
#define PyExc_TabError ((PyObject *)_molt_host_abi_symbol("PyExc_TabError"))
#define PyExc_ReferenceError ((PyObject *)_molt_host_abi_symbol("PyExc_ReferenceError"))
#define PyExc_SystemError ((PyObject *)_molt_host_abi_symbol("PyExc_SystemError"))
#define PyExc_SystemExit ((PyObject *)_molt_host_abi_symbol("PyExc_SystemExit"))
#define PyExc_TypeError ((PyObject *)_molt_host_abi_symbol("PyExc_TypeError"))
#define PyExc_UnboundLocalError ((PyObject *)_molt_host_abi_symbol("PyExc_UnboundLocalError"))
#define PyExc_UnicodeError ((PyObject *)_molt_host_abi_symbol("PyExc_UnicodeError"))
#define PyExc_UnicodeEncodeError ((PyObject *)_molt_host_abi_symbol("PyExc_UnicodeEncodeError"))
#define PyExc_UnicodeDecodeError ((PyObject *)_molt_host_abi_symbol("PyExc_UnicodeDecodeError"))
#define PyExc_UnicodeTranslateError ((PyObject *)_molt_host_abi_symbol("PyExc_UnicodeTranslateError"))
#define PyExc_ValueError ((PyObject *)_molt_host_abi_symbol("PyExc_ValueError"))
#define PyExc_ZeroDivisionError ((PyObject *)_molt_host_abi_symbol("PyExc_ZeroDivisionError"))
#define PyExc_BlockingIOError ((PyObject *)_molt_host_abi_symbol("PyExc_BlockingIOError"))
#define PyExc_BrokenPipeError ((PyObject *)_molt_host_abi_symbol("PyExc_BrokenPipeError"))
#define PyExc_ChildProcessError ((PyObject *)_molt_host_abi_symbol("PyExc_ChildProcessError"))
#define PyExc_ConnectionError ((PyObject *)_molt_host_abi_symbol("PyExc_ConnectionError"))
#define PyExc_ConnectionAbortedError ((PyObject *)_molt_host_abi_symbol("PyExc_ConnectionAbortedError"))
#define PyExc_ConnectionRefusedError ((PyObject *)_molt_host_abi_symbol("PyExc_ConnectionRefusedError"))
#define PyExc_ConnectionResetError ((PyObject *)_molt_host_abi_symbol("PyExc_ConnectionResetError"))
#define PyExc_FileExistsError ((PyObject *)_molt_host_abi_symbol("PyExc_FileExistsError"))
#define PyExc_FileNotFoundError ((PyObject *)_molt_host_abi_symbol("PyExc_FileNotFoundError"))
#define PyExc_InterruptedError ((PyObject *)_molt_host_abi_symbol("PyExc_InterruptedError"))
#define PyExc_IsADirectoryError ((PyObject *)_molt_host_abi_symbol("PyExc_IsADirectoryError"))
#define PyExc_NotADirectoryError ((PyObject *)_molt_host_abi_symbol("PyExc_NotADirectoryError"))
#define PyExc_PermissionError ((PyObject *)_molt_host_abi_symbol("PyExc_PermissionError"))
#define PyExc_ProcessLookupError ((PyObject *)_molt_host_abi_symbol("PyExc_ProcessLookupError"))
#define PyExc_TimeoutError ((PyObject *)_molt_host_abi_symbol("PyExc_TimeoutError"))
#define PyExc_Warning ((PyObject *)_molt_host_abi_symbol("PyExc_Warning"))
#define PyExc_UserWarning ((PyObject *)_molt_host_abi_symbol("PyExc_UserWarning"))
#define PyExc_DeprecationWarning ((PyObject *)_molt_host_abi_symbol("PyExc_DeprecationWarning"))
#define PyExc_PendingDeprecationWarning ((PyObject *)_molt_host_abi_symbol("PyExc_PendingDeprecationWarning"))
#define PyExc_SyntaxWarning ((PyObject *)_molt_host_abi_symbol("PyExc_SyntaxWarning"))
#define PyExc_RuntimeWarning ((PyObject *)_molt_host_abi_symbol("PyExc_RuntimeWarning"))
#define PyExc_FutureWarning ((PyObject *)_molt_host_abi_symbol("PyExc_FutureWarning"))
#define PyExc_ImportWarning ((PyObject *)_molt_host_abi_symbol("PyExc_ImportWarning"))
#define PyExc_UnicodeWarning ((PyObject *)_molt_host_abi_symbol("PyExc_UnicodeWarning"))
#define PyExc_BytesWarning ((PyObject *)_molt_host_abi_symbol("PyExc_BytesWarning"))
#define PyExc_EncodingWarning ((PyObject *)_molt_host_abi_symbol("PyExc_EncodingWarning"))
#define PyExc_ResourceWarning ((PyObject *)_molt_host_abi_symbol("PyExc_ResourceWarning"))
#else
#define PyExc_BaseException (&PyExc_BaseException)
#define PyExc_Exception (&PyExc_Exception)
#define PyExc_BaseExceptionGroup (&PyExc_BaseExceptionGroup)
#define PyExc_StopAsyncIteration (&PyExc_StopAsyncIteration)
#define PyExc_StopIteration (&PyExc_StopIteration)
#define PyExc_GeneratorExit (&PyExc_GeneratorExit)
#define PyExc_ArithmeticError (&PyExc_ArithmeticError)
#define PyExc_LookupError (&PyExc_LookupError)
#define PyExc_AssertionError (&PyExc_AssertionError)
#define PyExc_AttributeError (&PyExc_AttributeError)
#define PyExc_BufferError (&PyExc_BufferError)
#define PyExc_EOFError (&PyExc_EOFError)
#define PyExc_FloatingPointError (&PyExc_FloatingPointError)
#define PyExc_OSError (&PyExc_OSError)
#define PyExc_ImportError (&PyExc_ImportError)
#define PyExc_ModuleNotFoundError (&PyExc_ModuleNotFoundError)
#define PyExc_IndexError (&PyExc_IndexError)
#define PyExc_KeyError (&PyExc_KeyError)
#define PyExc_KeyboardInterrupt (&PyExc_KeyboardInterrupt)
#define PyExc_MemoryError (&PyExc_MemoryError)
#define PyExc_NameError (&PyExc_NameError)
#define PyExc_OverflowError (&PyExc_OverflowError)
#define PyExc_RuntimeError (&PyExc_RuntimeError)
#define PyExc_RecursionError (&PyExc_RecursionError)
#define PyExc_NotImplementedError (&PyExc_NotImplementedError)
#define PyExc_SyntaxError (&PyExc_SyntaxError)
#define PyExc_IndentationError (&PyExc_IndentationError)
#define PyExc_TabError (&PyExc_TabError)
#define PyExc_ReferenceError (&PyExc_ReferenceError)
#define PyExc_SystemError (&PyExc_SystemError)
#define PyExc_SystemExit (&PyExc_SystemExit)
#define PyExc_TypeError (&PyExc_TypeError)
#define PyExc_UnboundLocalError (&PyExc_UnboundLocalError)
#define PyExc_UnicodeError (&PyExc_UnicodeError)
#define PyExc_UnicodeEncodeError (&PyExc_UnicodeEncodeError)
#define PyExc_UnicodeDecodeError (&PyExc_UnicodeDecodeError)
#define PyExc_UnicodeTranslateError (&PyExc_UnicodeTranslateError)
#define PyExc_ValueError (&PyExc_ValueError)
#define PyExc_ZeroDivisionError (&PyExc_ZeroDivisionError)
#define PyExc_BlockingIOError (&PyExc_BlockingIOError)
#define PyExc_BrokenPipeError (&PyExc_BrokenPipeError)
#define PyExc_ChildProcessError (&PyExc_ChildProcessError)
#define PyExc_ConnectionError (&PyExc_ConnectionError)
#define PyExc_ConnectionAbortedError (&PyExc_ConnectionAbortedError)
#define PyExc_ConnectionRefusedError (&PyExc_ConnectionRefusedError)
#define PyExc_ConnectionResetError (&PyExc_ConnectionResetError)
#define PyExc_FileExistsError (&PyExc_FileExistsError)
#define PyExc_FileNotFoundError (&PyExc_FileNotFoundError)
#define PyExc_InterruptedError (&PyExc_InterruptedError)
#define PyExc_IsADirectoryError (&PyExc_IsADirectoryError)
#define PyExc_NotADirectoryError (&PyExc_NotADirectoryError)
#define PyExc_PermissionError (&PyExc_PermissionError)
#define PyExc_ProcessLookupError (&PyExc_ProcessLookupError)
#define PyExc_TimeoutError (&PyExc_TimeoutError)
#define PyExc_Warning (&PyExc_Warning)
#define PyExc_UserWarning (&PyExc_UserWarning)
#define PyExc_DeprecationWarning (&PyExc_DeprecationWarning)
#define PyExc_PendingDeprecationWarning (&PyExc_PendingDeprecationWarning)
#define PyExc_SyntaxWarning (&PyExc_SyntaxWarning)
#define PyExc_RuntimeWarning (&PyExc_RuntimeWarning)
#define PyExc_FutureWarning (&PyExc_FutureWarning)
#define PyExc_ImportWarning (&PyExc_ImportWarning)
#define PyExc_UnicodeWarning (&PyExc_UnicodeWarning)
#define PyExc_BytesWarning (&PyExc_BytesWarning)
#define PyExc_EncodingWarning (&PyExc_EncodingWarning)
#define PyExc_ResourceWarning (&PyExc_ResourceWarning)
#endif

#define PyExc_EnvironmentError PyExc_OSError
#define PyExc_IOError PyExc_OSError
#if defined(_WIN32) || defined(MS_WINDOWS)
#define PyExc_WindowsError PyExc_OSError
#endif

extern PyObject *PyErr_Occurred(void);
extern void PyErr_Clear(void);
extern void PyErr_SetString(PyObject *exc_type, const char *message);
extern void PyErr_SetObject(PyObject *exc_type, PyObject *value);
extern void PyErr_SetNone(PyObject *exc_type);
extern PyObject *PyErr_NoMemory(void);
extern void PyErr_Fetch(PyObject **type, PyObject **value, PyObject **traceback);
extern void PyErr_Restore(PyObject *type, PyObject *value, PyObject *traceback);
extern void PyErr_NormalizeException(PyObject **type, PyObject **value, PyObject **traceback);
extern PyObject *PyErr_GetRaisedException(void);
extern void PyErr_SetRaisedException(PyObject *exc);
extern PyObject *PyErr_GetHandledException(void);
extern void PyErr_SetHandledException(PyObject *exc);
extern void PyErr_GetExcInfo(PyObject **type, PyObject **value, PyObject **traceback);
extern void PyErr_SetExcInfo(PyObject *type, PyObject *value, PyObject *traceback);
extern int PyErr_GivenExceptionMatches(PyObject *given, PyObject *exc);
extern int PyErr_ExceptionMatches(PyObject *exc);
extern int PyErr_BadArgument(void);
extern void PyErr_BadInternalCall(void);
extern void PyErr_Print(void);
extern void PyErr_PrintEx(int set_sys_last_vars);
extern int PyErr_WarnEx(PyObject *category, const char *message, Py_ssize_t stack_level);
extern int PyErr_WarnFormat(PyObject *category, Py_ssize_t stack_level, const char *format, ...);

extern PyObject *PyObject_GetAttr(PyObject *obj, PyObject *name);
extern PyObject *PyObject_GetAttrString(PyObject *obj, const char *name);
extern int PyObject_SetAttr(PyObject *obj, PyObject *name, PyObject *value);
extern int PyObject_SetAttrString(PyObject *obj, const char *name, PyObject *value);
extern int PyObject_HasAttr(PyObject *obj, PyObject *name);
extern int PyObject_HasAttrString(PyObject *obj, const char *name);
extern int PyObject_HasAttrWithError(PyObject *obj, PyObject *name);
extern int PyObject_HasAttrStringWithError(PyObject *obj, const char *name);
extern int PyObject_GetOptionalAttr(PyObject *obj, PyObject *name, PyObject **result);
extern int PyObject_GetOptionalAttrString(PyObject *obj, const char *name, PyObject **result);
extern PyObject *PyObject_GenericGetAttr(PyObject *obj, PyObject *name);
extern int PyObject_GenericSetAttr(PyObject *obj, PyObject *name, PyObject *value);
extern PyObject *_PyObject_GenericGetAttrWithDict(PyObject *obj, PyObject *name, PyObject *dict, int suppress);
extern PyObject *PyObject_GenericGetDict(PyObject *obj, void *context);
extern int PyObject_GenericSetDict(PyObject *obj, PyObject *value, void *context);

/* CPython deletion aliases use the same setters with a NULL value. */
#define PyObject_DelAttr(obj, name) PyObject_SetAttr((obj), (name), NULL)
#define PyObject_DelAttrString(obj, name) PyObject_SetAttrString((obj), (name), NULL)

#endif /* MOLT_EXCEPTION_ATTRIBUTE_EXPORTS_H */
