#ifndef MOLT_NUMPY_RANDOM_BITGEN_H
#define MOLT_NUMPY_RANDOM_BITGEN_H

/*
 * Source-compat overlay derived from NumPy 2.4.2 public random/bitgen.h.
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct bitgen {
    void *state;
    uint64_t (*next_uint64)(void *st);
    uint32_t (*next_uint32)(void *st);
    double (*next_double)(void *st);
    uint64_t (*next_raw)(void *st);
} bitgen_t;

#endif
