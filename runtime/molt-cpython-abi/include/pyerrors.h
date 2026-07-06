#ifndef MOLT_CPYTHON_ABI_PYERRORS_H
#define MOLT_CPYTHON_ABI_PYERRORS_H

/*
 * pyerrors.h — CPython public C-API compatibility header for the standalone
 * CPython-ABI tier.
 *
 * The stock CPython ``Include/pyerrors.h`` is pulled in transitively by
 * ``Python.h``; a few source-recompiled extensions (numpy _core ``refcount.c``)
 * include it directly. This tier defines the full ``PyErr_*`` / ``PyExc_*``
 * surface inline in ``Python.h``, so this header only needs to forward to it
 * and resolve to this tier's own ``Python.h``.
 */

#include <Python.h>

#endif /* MOLT_CPYTHON_ABI_PYERRORS_H */
