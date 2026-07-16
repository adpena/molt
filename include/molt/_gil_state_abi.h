/* Canonical PyGILState_STATE ABI values shared by both Molt Python.h surfaces. */
#ifndef MOLT_GIL_STATE_ABI_H
#define MOLT_GIL_STATE_ABI_H

typedef enum {
    PyGILState_LOCKED = 0,
    PyGILState_UNLOCKED = 1
} PyGILState_STATE;

#endif /* MOLT_GIL_STATE_ABI_H */
