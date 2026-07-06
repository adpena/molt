#ifndef MOLT_CPYTHON_ABI_PYMEM_H
#define MOLT_CPYTHON_ABI_PYMEM_H

/*
 * pymem.h — CPython public C-API compatibility header for the standalone
 * CPython-ABI tier.
 *
 * The stock CPython ``Include/pymem.h`` is pulled in transitively by
 * ``Python.h``; a few source-recompiled extensions (numpy _core ``alloc.c``)
 * include it directly. This tier defines the full ``PyMem_*`` surface inline in
 * ``Python.h``, so this header only needs to forward to it and resolve to this
 * tier's own ``Python.h``.
 */

#include <Python.h>

#endif /* MOLT_CPYTHON_ABI_PYMEM_H */
