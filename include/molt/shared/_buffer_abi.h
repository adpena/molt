/* CPython 3.12 Py_buffer prefix shared by both C transports.
 * Export metadata and leases belong to the linked implementation, never a
 * private tail in caller-owned storage. Include after PyObject/Py_ssize_t. */
#ifndef MOLT_BUFFER_ABI_H
#define MOLT_BUFFER_ABI_H
typedef struct bufferinfo {
    void *buf;
    PyObject *obj;
    Py_ssize_t len;
    Py_ssize_t itemsize;
    int readonly;
    int ndim;
    char *format;
    Py_ssize_t *shape;
    Py_ssize_t *strides;
    Py_ssize_t *suboffsets;
    void *internal;
} Py_buffer;
#endif /* MOLT_BUFFER_ABI_H */
