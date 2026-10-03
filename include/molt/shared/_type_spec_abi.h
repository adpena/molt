/* One public stable type-spec layout for both Molt header transports. */
#ifndef MOLT_TYPE_SPEC_ABI_H
#define MOLT_TYPE_SPEC_ABI_H

typedef struct PyType_Slot {
    int slot;
    void *pfunc;
} PyType_Slot;

typedef struct PyType_Spec {
    const char *name;
    int basicsize;
    int itemsize;
    unsigned int flags;
    PyType_Slot *slots;
} PyType_Spec;

#include "_molt_typeslots.generated.h"

#endif /* MOLT_TYPE_SPEC_ABI_H */
