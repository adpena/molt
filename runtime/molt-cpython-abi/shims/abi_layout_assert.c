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

/* Keep the TU non-empty and warning-clean. */
int molt_abi_layout_asserted(void);
int molt_abi_layout_asserted(void) { return 0; }
