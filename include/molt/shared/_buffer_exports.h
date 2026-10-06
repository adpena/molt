/* Buffer acquisition, release and memoryview identity have one linked owner.
 * Host lookup selects transport only; no header-local descriptors or caches. */
#ifndef MOLT_BUFFER_EXPORTS_H
#define MOLT_BUFFER_EXPORTS_H
extern PyObject * PyMemoryView_FromMemory(char *mem, Py_ssize_t size, int flags);
extern PyObject * PyMemoryView_FromBuffer(Py_buffer *info);
extern PyObject * PyMemoryView_FromObject(PyObject *object);
extern int PyMemoryView_Check(PyObject *object);
extern PyObject * PyMemoryView_GET_BASE(PyObject *object);
extern Py_buffer * PyMemoryView_GET_BUFFER(PyObject *object);
extern int PyObject_GetBuffer(PyObject *object, Py_buffer *view, int flags);
extern int PyObject_CheckBuffer(PyObject *object);
extern void PyBuffer_Release(Py_buffer *view);
extern int PyBuffer_IsContiguous(const Py_buffer *view, char order);
extern int PyBuffer_FillInfo(Py_buffer *view, PyObject *object, void *buf, Py_ssize_t len, int readonly, int flags);

#ifdef MOLT_EXTENSION_HOST_ABI
#define PyMemoryView_FromMemory ((PyObject * (*)(char *, Py_ssize_t, int))_molt_host_abi_symbol("PyMemoryView_FromMemory"))
#define PyMemoryView_FromBuffer ((PyObject * (*)(Py_buffer *))_molt_host_abi_symbol("PyMemoryView_FromBuffer"))
#define PyMemoryView_FromObject ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyMemoryView_FromObject"))
#define PyMemoryView_Check ((int (*)(PyObject *))_molt_host_abi_symbol("PyMemoryView_Check"))
#define PyMemoryView_GET_BASE ((PyObject * (*)(PyObject *))_molt_host_abi_symbol("PyMemoryView_GET_BASE"))
#define PyMemoryView_GET_BUFFER ((Py_buffer * (*)(PyObject *))_molt_host_abi_symbol("PyMemoryView_GET_BUFFER"))
#define PyObject_GetBuffer ((int (*)(PyObject *, Py_buffer *, int))_molt_host_abi_symbol("PyObject_GetBuffer"))
#define PyObject_CheckBuffer ((int (*)(PyObject *))_molt_host_abi_symbol("PyObject_CheckBuffer"))
#define PyBuffer_Release ((void (*)(Py_buffer *))_molt_host_abi_symbol("PyBuffer_Release"))
#define PyBuffer_IsContiguous ((int (*)(const Py_buffer *, char))_molt_host_abi_symbol("PyBuffer_IsContiguous"))
#define PyBuffer_FillInfo ((int (*)(Py_buffer *, PyObject *, void *, Py_ssize_t, int, int))_molt_host_abi_symbol("PyBuffer_FillInfo"))
#endif

/* Pure shape arithmetic has no exported Rust owner. */
static inline int _molt_pyssize_mul_nonnegative(Py_ssize_t lhs, Py_ssize_t rhs, Py_ssize_t *out) {
    Py_ssize_t max_value = (Py_ssize_t)(((size_t)-1) >> 1);
    if (lhs < 0 || rhs < 0 || out == NULL) {
        return 0;
    }
    if (rhs != 0 && lhs > max_value / rhs) {
        return 0;
    }
    *out = lhs * rhs;
    return 1;
}

static inline void PyBuffer_FillContiguousStrides(int ndim, Py_ssize_t *shape,
                                                    Py_ssize_t *strides,
                                                    int itemsize, char order) {
    int i;
    Py_ssize_t next;
    if (ndim <= 0 || shape == NULL || strides == NULL || itemsize <= 0) return;
    if (order == 'F' || order == 'f') {
        strides[0] = itemsize;
        for (i = 1; i < ndim; i++) {
            if (!_molt_pyssize_mul_nonnegative(strides[i - 1], shape[i - 1], &next)) {
                strides[i] = 0;
                return;
            }
            strides[i] = next;
        }
        return;
    }
    strides[ndim - 1] = itemsize;
    for (i = ndim - 2; i >= 0; i--) {
        if (!_molt_pyssize_mul_nonnegative(strides[i + 1], shape[i + 1], &next)) {
            strides[i] = 0;
            return;
        }
        strides[i] = next;
    }
}

#endif /* MOLT_BUFFER_EXPORTS_H */
