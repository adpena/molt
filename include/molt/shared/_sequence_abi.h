/* Shared physical sequence prefixes for both C header transports. */
#ifndef MOLT_SEQUENCE_ABI_H
#define MOLT_SEQUENCE_ABI_H

typedef struct {
    PyObject_VAR_HEAD
    PyObject *ob_item[1];
} PyTupleObject;

typedef struct {
    PyObject_VAR_HEAD
    PyObject **ob_item;
    Py_ssize_t allocated;
} PyListObject;

#endif /* MOLT_SEQUENCE_ABI_H */
