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

typedef PyTupleObject PyStructSequence;

typedef struct PyStructSequence_Field {
    const char *name;
    const char *doc;
} PyStructSequence_Field;

typedef struct PyStructSequence_Desc {
    const char *name;
    const char *doc;
    PyStructSequence_Field *fields;
    int n_in_sequence;
} PyStructSequence_Desc;

#endif /* MOLT_SEQUENCE_ABI_H */
