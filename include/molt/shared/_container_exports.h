/* Container APIs preserve physical PyObject elements through the linked ABI. */
#ifndef MOLT_CONTAINER_EXPORTS_H
#define MOLT_CONTAINER_EXPORTS_H

extern PyObject *PyDictProxy_New (PyObject *mapping);
extern int PyDict_Check (PyObject *op);
extern int PyDict_CheckExact(PyObject *op);
extern void PyDict_Clear (PyObject *op);
extern int PyDict_Contains (PyObject *op, PyObject *key);
extern int PyDict_ContainsString(PyObject *op, const char *key);
extern PyObject *PyDict_Copy (PyObject *op);
extern int PyDict_DelItem (PyObject *op, PyObject *key);
extern int PyDict_DelItemString (PyObject *op, const char *key);
extern PyObject *PyDict_GetItem (PyObject *op, PyObject *key);
extern int PyDict_GetItemRef (PyObject *op, PyObject *key, PyObject **result);
extern PyObject *PyDict_GetItemString (PyObject *op, const char *key);
extern int PyDict_GetItemStringRef(PyObject *op, const char *key, PyObject **result);
extern PyObject *PyDict_GetItemWithError(PyObject *op, PyObject *key);
extern PyObject *PyDict_Items(PyObject *obj);
extern PyObject *PyDict_Keys (PyObject *op);
extern int PyDict_Merge (PyObject *op, PyObject *other, int override);
extern int PyDict_MergeFromSeq2 (PyObject *op, PyObject *seq2, int override);
extern PyObject *PyDict_New (void);
extern int PyDict_Next (PyObject *op, Py_ssize_t *pos, PyObject **key, PyObject **value);
extern int PyDict_Pop(PyObject *obj, PyObject *key, PyObject **result);
extern PyObject *PyDict_SetDefault (PyObject *op, PyObject *key, PyObject *default_value);
extern int PyDict_SetDefaultRef (PyObject *op, PyObject *key, PyObject *default_value, PyObject **result);
extern int PyDict_SetItem (PyObject *op, PyObject *key, PyObject *val);
extern int PyDict_SetItemString (PyObject *op, const char *key, PyObject *val);
extern Py_ssize_t PyDict_Size (PyObject *op);
extern int PyDict_Update (PyObject *op, PyObject *other);
extern PyObject *PyDict_Values (PyObject *op);
extern int PyFrozenSet_Check(PyObject *op);
extern int PyFrozenSet_CheckExact(PyObject *op);
extern PyObject *PyFrozenSet_New(PyObject *iterable);
extern int PyMapping_Check(PyObject *obj);
extern int PyMapping_DelItemString(PyObject *obj, const char *key);
extern PyObject *PyMapping_GetItemString(PyObject *obj, const char *key);
extern int PyMapping_GetOptionalItem(PyObject *obj, PyObject *key, PyObject **result);
extern int PyMapping_HasKey(PyObject *obj, PyObject *key);
extern int PyMapping_HasKeyString(PyObject *obj, const char *key);
extern int PyMapping_HasKeyStringWithError(PyObject *obj, const char *key);
extern int PyMapping_HasKeyWithError(PyObject *obj, PyObject *key);
extern PyObject *PyMapping_Items(PyObject *obj);
extern PyObject *PyMapping_Keys(PyObject *obj);
extern Py_ssize_t PyMapping_Length(PyObject *obj);
extern int PyMapping_SetItemString(PyObject *obj, const char *key, PyObject *value);
extern Py_ssize_t PyMapping_Size(PyObject *obj);
extern PyObject *PyMapping_Values(PyObject *obj);
extern int PyObject_DelItem(PyObject *obj, PyObject *key);
extern PyObject *PyObject_GetItem (PyObject *op, PyObject *key);
extern int PyObject_SetItem (PyObject *op, PyObject *key, PyObject *value);
extern int PySet_Add (PyObject *anyset, PyObject *key);
extern int PySet_Check (PyObject *op);
extern int PySet_CheckExact(PyObject *op);
extern int PySet_Clear (PyObject *anyset);
extern int PySet_Contains (PyObject *anyset, PyObject *key);
extern int PySet_Discard (PyObject *anyset, PyObject *key);
extern PyObject *PySet_New (PyObject *iterable);
extern PyObject *PySet_Pop (PyObject *anyset);
extern Py_ssize_t PySet_Size (PyObject *anyset);
extern Py_ssize_t PySlice_AdjustIndices(Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t step);
extern int PySlice_Check(PyObject *op);
extern int PySlice_GetIndices(PyObject *slice, Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step);
extern int PySlice_GetIndicesEx(PyObject *slice, Py_ssize_t length, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step, Py_ssize_t *slicelength);
extern PyObject *PySlice_New(PyObject *start, PyObject *stop, PyObject *step);
extern int PySlice_Unpack(PyObject *slice, Py_ssize_t *start, Py_ssize_t *stop, Py_ssize_t *step);
extern PyObject *_PyDict_GetItem_KnownHash(PyObject *op, PyObject *key, Py_hash_t hash);
extern PyObject *_PyDict_NewPresized (Py_ssize_t minused);
PyAPI_DATA(PyTypeObject) PyDict_Type;
PyAPI_DATA(PyTypeObject) PySet_Type;
PyAPI_DATA(PyTypeObject) PyFrozenSet_Type;
PyAPI_DATA(PyTypeObject) PySlice_Type;

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyDictProxy_New ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyDictProxy_New"))
#define PyDict_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyDict_Check"))
#define PyDict_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyDict_CheckExact"))
#define PyDict_Clear ((void (*)(PyObject *))_molt_host_abi_symbol("PyDict_Clear"))
#define PyDict_Contains ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_Contains"))
#define PyDict_ContainsString ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyDict_ContainsString"))
#define PyDict_Copy ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyDict_Copy"))
#define PyDict_DelItem ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_DelItem"))
#define PyDict_DelItemString ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyDict_DelItemString"))
#define PyDict_GetItem ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_GetItem"))
#define PyDict_GetItemRef ((int (*)(PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyDict_GetItemRef"))
#define PyDict_GetItemString ((PyObject * (*)(PyObject *, const char *))_molt_host_abi_symbol("PyDict_GetItemString"))
#define PyDict_GetItemStringRef ((int (*)(PyObject *, const char *, PyObject **))_molt_host_abi_symbol("PyDict_GetItemStringRef"))
#define PyDict_GetItemWithError ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_GetItemWithError"))
#define PyDict_Items ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyDict_Items"))
#define PyDict_Keys ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyDict_Keys"))
#define PyDict_Merge ((int (*)(PyObject *, PyObject *, int))_molt_host_abi_symbol("PyDict_Merge"))
#define PyDict_MergeFromSeq2 ((int (*)(PyObject *, PyObject *, int))_molt_host_abi_symbol("PyDict_MergeFromSeq2"))
#define PyDict_New ((PyObject * (*)(void))_molt_host_abi_symbol("PyDict_New"))
#define PyDict_Next ((int (*)(PyObject *, Py_ssize_t *, PyObject **, PyObject **))_molt_host_abi_symbol("PyDict_Next"))
#define PyDict_Pop ((int (*)(PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyDict_Pop"))
#define PyDict_SetDefault ((PyObject * (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_SetDefault"))
#define PyDict_SetDefaultRef ((int (*)(PyObject *, PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyDict_SetDefaultRef"))
#define PyDict_SetItem ((int (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_SetItem"))
#define PyDict_SetItemString ((int (*)(PyObject *, const char *, PyObject *))_molt_host_abi_symbol("PyDict_SetItemString"))
#define PyDict_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyDict_Size"))
#define PyDict_Update ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyDict_Update"))
#define PyDict_Values ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyDict_Values"))
#define PyFrozenSet_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyFrozenSet_Check"))
#define PyFrozenSet_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PyFrozenSet_CheckExact"))
#define PyFrozenSet_New ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyFrozenSet_New"))
#define PyMapping_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Check"))
#define PyMapping_DelItemString ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyMapping_DelItemString"))
#define PyMapping_GetItemString ((PyObject * (*)(PyObject *, const char *))_molt_host_abi_symbol("PyMapping_GetItemString"))
#define PyMapping_GetOptionalItem ((int (*)(PyObject *, PyObject *, PyObject **))_molt_host_abi_symbol("PyMapping_GetOptionalItem"))
#define PyMapping_HasKey ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyMapping_HasKey"))
#define PyMapping_HasKeyString ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyMapping_HasKeyString"))
#define PyMapping_HasKeyStringWithError ((int (*)(PyObject *, const char *))_molt_host_abi_symbol("PyMapping_HasKeyStringWithError"))
#define PyMapping_HasKeyWithError ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyMapping_HasKeyWithError"))
#define PyMapping_Items ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Items"))
#define PyMapping_Keys ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Keys"))
#define PyMapping_Length ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Length"))
#define PyMapping_SetItemString ((int (*)(PyObject *, const char *, PyObject *))_molt_host_abi_symbol("PyMapping_SetItemString"))
#define PyMapping_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Size"))
#define PyMapping_Values ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyMapping_Values"))
#define PyObject_DelItem ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_DelItem"))
#define PyObject_GetItem ((PyObject * (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_GetItem"))
#define PyObject_SetItem ((int (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PyObject_SetItem"))
#define PySet_Add ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySet_Add"))
#define PySet_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PySet_Check"))
#define PySet_CheckExact ((int (*)(PyObject *))_molt_host_abi_symbol("PySet_CheckExact"))
#define PySet_Clear ((int (*)(PyObject *))_molt_host_abi_symbol("PySet_Clear"))
#define PySet_Contains ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySet_Contains"))
#define PySet_Discard ((int (*)(PyObject *, PyObject *))_molt_host_abi_symbol("PySet_Discard"))
#define PySet_New ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PySet_New"))
#define PySet_Pop ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PySet_Pop"))
#define PySet_Size ((Py_ssize_t (*)(PyObject *))_molt_host_abi_symbol("PySet_Size"))
#define PySlice_AdjustIndices ((Py_ssize_t (*)(Py_ssize_t, Py_ssize_t *, Py_ssize_t *, Py_ssize_t))_molt_host_abi_symbol("PySlice_AdjustIndices"))
#define PySlice_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PySlice_Check"))
#define PySlice_GetIndices ((int (*)(PyObject *, Py_ssize_t, Py_ssize_t *, Py_ssize_t *, Py_ssize_t *))_molt_host_abi_symbol("PySlice_GetIndices"))
#define PySlice_GetIndicesEx ((int (*)(PyObject *, Py_ssize_t, Py_ssize_t *, Py_ssize_t *, Py_ssize_t *, Py_ssize_t *))_molt_host_abi_symbol("PySlice_GetIndicesEx"))
#define PySlice_New ((PyObject * (*)(PyObject *, PyObject *, PyObject *))_molt_host_abi_symbol("PySlice_New"))
#define PySlice_Unpack ((int (*)(PyObject *, Py_ssize_t *, Py_ssize_t *, Py_ssize_t *))_molt_host_abi_symbol("PySlice_Unpack"))
#define _PyDict_GetItem_KnownHash ((PyObject * (*)(PyObject *, PyObject *, Py_hash_t))_molt_host_abi_symbol("_PyDict_GetItem_KnownHash"))
#define _PyDict_NewPresized ((PyObject * (*)(Py_ssize_t))_molt_host_abi_symbol("_PyDict_NewPresized"))
#define PyDict_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyDict_Type"))
#define PySet_Type (*(PyTypeObject *)_molt_host_abi_symbol("PySet_Type"))
#define PyFrozenSet_Type (*(PyTypeObject *)_molt_host_abi_symbol("PyFrozenSet_Type"))
#define PySlice_Type (*(PyTypeObject *)_molt_host_abi_symbol("PySlice_Type"))
#endif

#define PyDict_GET_SIZE(op) PyDict_Size((PyObject *)(op))
#define PySet_GET_SIZE(op) PySet_Size((PyObject *)(op))
#define PyAnySet_Check(op) (PySet_Check((PyObject *)(op)) || PyFrozenSet_Check((PyObject *)(op)))
#define PyMapping_DelItem(op, key) PyObject_DelItem((PyObject *)(op), (PyObject *)(key))

#endif /* MOLT_CONTAINER_EXPORTS_H */
