/* One linked authority for iteration, sequence admission and materialization. */
#ifndef MOLT_SEQUENCE_EXPORTS_H
#define MOLT_SEQUENCE_EXPORTS_H

typedef enum {
    PYGEN_RETURN = 0,
    PYGEN_ERROR = -1,
    PYGEN_NEXT = 1
} PySendResult;

extern PyObject *PyObject_GetIter(PyObject *obj);
extern int PyIter_Check(PyObject *obj);
extern PyObject *PyIter_Next(PyObject *obj);
extern PyObject *PyObject_Next(PyObject *obj);
extern PyObject *PyObject_SelfIter(PyObject *obj);
extern PyObject *PySeqIter_New(PyObject *obj);
extern PySendResult PyIter_Send(PyObject *iter, PyObject *arg, PyObject **result);

extern Py_ssize_t PyObject_LengthHint(PyObject *obj, Py_ssize_t defaultvalue);
extern int PySequence_Check(PyObject *obj);
extern PyObject *PySequence_Fast(PyObject *obj, const char *message);
extern PyObject *PySequence_List(PyObject *obj);
extern PyObject *PySequence_Tuple(PyObject *obj);
extern Py_ssize_t PySequence_Fast_GET_SIZE(PyObject *obj);
extern PyObject *PySequence_Fast_GET_ITEM(PyObject *obj, Py_ssize_t index);
extern PyObject **PySequence_Fast_ITEMS(PyObject *obj);

extern int PyList_Append (PyObject *list, PyObject *item);
extern PyObject *PyList_AsTuple (PyObject *op);
extern int PyList_Check (PyObject *op);
extern int PyList_CheckExact(PyObject *op);
extern PyObject *PyList_GetItem (PyObject *op, Py_ssize_t i);
extern PyObject *PyList_GetItemRef(PyObject *op, Py_ssize_t i);
extern PyObject *PyList_GetSlice(PyObject *op, Py_ssize_t low, Py_ssize_t high);
extern int PyList_Insert (PyObject *op, Py_ssize_t where, PyObject *v);
extern PyObject *PyList_New (Py_ssize_t size);
extern int PyList_Reverse (PyObject *op);
extern void PyList_SET_ITEM(PyObject *op, Py_ssize_t index, PyObject *value);
extern int PyList_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern int PyList_SetSlice(PyObject *op, Py_ssize_t low, Py_ssize_t high, PyObject *itemlist);
extern Py_ssize_t PyList_Size (PyObject *op);
extern int PyList_Sort (PyObject *op);
extern PyObject *PySequence_Concat (PyObject *op, PyObject *other);
extern int PySequence_Contains (PyObject *op, PyObject *value);
extern Py_ssize_t PySequence_Count (PyObject *op, PyObject *value);
extern int PySequence_DelItem(PyObject *obj, Py_ssize_t index);
extern PyObject *PySequence_GetItem (PyObject *op, Py_ssize_t i);
extern PyObject *PySequence_GetSlice(PyObject *obj, Py_ssize_t low, Py_ssize_t high);
extern PyObject *PySequence_InPlaceConcat(PyObject *op, PyObject *other);
extern PyObject *PySequence_InPlaceRepeat(PyObject *op, Py_ssize_t count);
extern Py_ssize_t PySequence_Index (PyObject *op, PyObject *value);
extern Py_ssize_t PySequence_Length (PyObject *op);
extern PyObject *PySequence_Repeat (PyObject *op, Py_ssize_t count);
extern int PySequence_SetItem (PyObject *op, Py_ssize_t i, PyObject *value);
extern int PySequence_SetSlice(PyObject *obj, Py_ssize_t low, Py_ssize_t high, PyObject *value);
extern Py_ssize_t PySequence_Size (PyObject *op);
extern int PyTuple_Check (PyObject *op);
extern int PyTuple_CheckExact(PyObject *op);
extern PyObject *PyTuple_FromArray(PyObject *const *array, Py_ssize_t size);
extern PyObject *PyTuple_GetItem (PyObject *op, Py_ssize_t i);
extern PyObject *PyTuple_GetSlice(PyObject *op, Py_ssize_t start, Py_ssize_t end);
extern PyObject *PyTuple_New (Py_ssize_t size);
extern PyObject *PyTuple_Pack (Py_ssize_t n, ...);
extern void PyTuple_SET_ITEM(PyObject *op, Py_ssize_t index, PyObject *value);
extern int PyTuple_SetItem (PyObject *op, Py_ssize_t i, PyObject *v);
extern Py_ssize_t PyTuple_Size (PyObject *op);
extern PyObject *_PyList_Extend(PyListObject *self, PyObject *iterable);
extern int _PyTuple_Resize(PyObject **op, Py_ssize_t newsize);

PyAPI_DATA(PyTypeObject) PyList_Type;
PyAPI_DATA(PyTypeObject) PyTuple_Type;

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyObject_GetIter ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyObject_GetIter"))
#define PyIter_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyIter_Check"))
#define PyIter_Next ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyIter_Next"))
#define PyObject_Next ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyObject_Next"))
#define PyObject_SelfIter ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PyObject_SelfIter"))
#define PySeqIter_New ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PySeqIter_New"))
#define PyIter_Send ((PySendResult (*)(PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyIter_Send"))
#define PyObject_LengthHint ((Py_ssize_t (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PyObject_LengthHint"))
#define PySequence_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PySequence_Check"))
#define PySequence_Fast ((PyObject *(*)(PyObject *, const char *))_molt_host_abi_symbol("PySequence_Fast"))
#define PySequence_List ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PySequence_List"))
#define PySequence_Tuple ((PyObject *(*)(PyObject *))_molt_host_abi_symbol("PySequence_Tuple"))
#define PySequence_Fast_GET_SIZE ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PySequence_Fast_GET_SIZE"))
#define PySequence_Fast_GET_ITEM ((PyObject *(*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PySequence_Fast_GET_ITEM"))
#define PySequence_Fast_ITEMS ((PyObject **(*)(PyObject *))_molt_host_abi_symbol("PySequence_Fast_ITEMS"))
#define PyList_Append ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyList_Append"))
#define PyList_AsTuple ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyList_AsTuple"))
#define PyList_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyList_Check"))
#define PyList_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyList_CheckExact"))
#define PyList_GetItem ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PyList_GetItem"))
#define PyList_GetItemRef ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PyList_GetItemRef"))
#define PyList_GetSlice ((PyObject * (*)(PyObject *, Py_ssize_t, Py_ssize_t))_molt_host_abi_symbol("PyList_GetSlice"))
#define PyList_Insert ((int (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyList_Insert"))
#define PyList_New ((PyObject * (*)(Py_ssize_t))_molt_host_abi_symbol("PyList_New"))
#define PyList_Reverse ((int (*)(PyObject *))_molt_host_abi_symbol("PyList_Reverse"))
#define PyList_SetItem ((int (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyList_SetItem"))
#define PyList_SetSlice ((int (*)(PyObject *, Py_ssize_t, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyList_SetSlice"))
#define PyList_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyList_Size"))
#define PyList_Sort ((int (*)(PyObject *))_molt_host_abi_symbol("PyList_Sort"))
#define PySequence_Concat ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySequence_Concat"))
#define PySequence_Contains ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySequence_Contains"))
#define PySequence_Count ((Py_ssize_t (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySequence_Count"))
#define PySequence_DelItem ((int (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PySequence_DelItem"))
#define PySequence_GetItem ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PySequence_GetItem"))
#define PySequence_GetSlice ((PyObject * (*)(PyObject *, Py_ssize_t, Py_ssize_t))_molt_host_abi_symbol("PySequence_GetSlice"))
#define PySequence_InPlaceConcat ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySequence_InPlaceConcat"))
#define PySequence_InPlaceRepeat ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PySequence_InPlaceRepeat"))
#define PySequence_Index ((Py_ssize_t (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySequence_Index"))
#define PySequence_Length ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PySequence_Length"))
#define PySequence_Repeat ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PySequence_Repeat"))
#define PySequence_SetItem ((int (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PySequence_SetItem"))
#define PySequence_SetSlice ((int (*)(PyObject *, Py_ssize_t, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PySequence_SetSlice"))
#define PySequence_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PySequence_Size"))
#define PyTuple_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyTuple_Check"))
#define PyTuple_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyTuple_CheckExact"))
#define PyTuple_FromArray ((PyObject * (*)(PyObject *const *, Py_ssize_t))_molt_host_abi_symbol("PyTuple_FromArray"))
#define PyTuple_GetItem ((PyObject * (*)(PyObject *, Py_ssize_t))_molt_host_abi_symbol("PyTuple_GetItem"))
#define PyTuple_GetSlice ((PyObject * (*)(PyObject *, Py_ssize_t, Py_ssize_t))_molt_host_abi_symbol("PyTuple_GetSlice"))
#define PyTuple_New ((PyObject * (*)(Py_ssize_t))_molt_host_abi_symbol("PyTuple_New"))
#define PyTuple_Pack ((PyObject * (*)(Py_ssize_t, ...))_molt_host_abi_symbol("PyTuple_Pack"))
#define PyTuple_SetItem ((int (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyTuple_SetItem"))
#define PyTuple_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyTuple_Size"))
#define _PyList_Extend ((PyObject * (*)(PyListObject *, PyObject *))_molt_host_abi_symbol("_PyList_Extend"))
#define _PyTuple_Resize ((int (*)(PyObject **, Py_ssize_t))_molt_host_abi_symbol("_PyTuple_Resize"))
#define PyList_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyList_Type"))
#define PyTuple_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyTuple_Type"))
#endif

#define PyTuple_GET_SIZE(op) (((PyVarObject *)(op))->ob_size)
#define PyTuple_GET_ITEM(op, index) (((PyTupleObject *)(op))->ob_item[(index)])
#define PyList_GET_SIZE(op) (((PyVarObject *)(op))->ob_size)
#define PyList_GET_ITEM(op, index) (((PyListObject *)(op))->ob_item[(index)])
#define PySequence_ITEM(op, index) PySequence_GetItem((PyObject *)(op), (index))
#define PySequence_FAST_GET_SIZE(op) PySequence_Fast_GET_SIZE((PyObject *)(op))

/* Unsafe construction stores steal the incoming reference without releasing
 * a displaced one. Both transports use the same physical/runtime transaction. */
#ifdef MOLT_EXTENSION_HOST_ABI
#define PyTuple_SET_ITEM(op, index, value) (((void (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyTuple_SET_ITEM"))((PyObject *)(op), (index), (PyObject *)(value)))
#define PyList_SET_ITEM(op, index, value) (((void (*)(PyObject *, Py_ssize_t, PyObject *))_molt_host_abi_symbol("PyList_SET_ITEM"))((PyObject *)(op), (index), (PyObject *)(value)))
#else
#define PyTuple_SET_ITEM(op, index, value) (PyTuple_SET_ITEM)((PyObject *)(op), (index), (PyObject *)(value))
#define PyList_SET_ITEM(op, index, value) (PyList_SET_ITEM)((PyObject *)(op), (index), (PyObject *)(value))
#endif

#endif /* MOLT_SEQUENCE_EXPORTS_H */
