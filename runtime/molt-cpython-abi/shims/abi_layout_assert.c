/*
 * abi_layout_assert.c — compile-time layout parity enforcement.
 *
 * This translation unit exists solely to force the single-authority layout
 * guard (runtime/molt-cpython-abi/include/_molt_abi_layout.generated.h, generated
 * from the Rust repr(C) authority in src/abi_types.rs) to be evaluated whenever
 * the crate itself builds — not only when an external C extension is compiled.
 *
 * <Python.h> pulls in the generated header, whose _Static_assert block pins the
 * sizeof/offsetof of every traditional-representation struct to the value the Rust
 * authority dictates. If the hand-written C structs in <Python.h> drift from the
 * Rust structs the dylib actually operates on, this file fails to compile, so the
 * standalone libmolt_cpython_abi dylib cannot be built with a memory-unsafe layout
 * mismatch. See tools/gen_cpython_abi_layout.py.
 */
#include <Python.h>

/* The macros must be valid before C expressions exist (NumPy/Cython use #if). */
#if SIZEOF_VOID_P != 4 && SIZEOF_VOID_P != 8
#error "invalid pointer model"
#endif
#if SIZEOF_INT != 4 || SIZEOF_LONG_LONG != 8 || SIZEOF_SIZE_T != SIZEOF_VOID_P
#error "invalid scalar model"
#endif
#if LONG_BIT != 32 && LONG_BIT != 64
#error "invalid long model"
#endif

typedef char molt_gil_state_locked_abi[(PyGILState_LOCKED == 0) ? 1 : -1];
typedef char molt_gil_state_unlocked_abi[(PyGILState_UNLOCKED == 1) ? 1 : -1];
typedef char molt_sizeof_void_p_abi[(SIZEOF_VOID_P == sizeof(void *)) ? 1 : -1];
typedef char molt_sizeof_int_abi[(SIZEOF_INT == sizeof(int)) ? 1 : -1];
typedef char molt_sizeof_long_abi[(SIZEOF_LONG == sizeof(long)) ? 1 : -1];
typedef char molt_sizeof_long_long_abi[(SIZEOF_LONG_LONG == sizeof(long long)) ? 1 : -1];
typedef char molt_sizeof_size_t_abi[(SIZEOF_SIZE_T == sizeof(size_t)) ? 1 : -1];
typedef char molt_long_bit_abi[(LONG_BIT == sizeof(long) * CHAR_BIT) ? 1 : -1];

/* Keep the TU non-empty and warning-clean. */
int molt_abi_layout_asserted(void);
int molt_abi_layout_asserted(void) { return 0; }
