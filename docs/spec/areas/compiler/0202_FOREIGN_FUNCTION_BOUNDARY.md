# Molt Foreign Function Boundary

**Status:** Active contract; implementation and target qualification tracked separately
**Owner:** compiler + runtime + tooling

Molt compiles admitted package source and rebuilds native extensions against
[`libmolt`](../compat/contracts/libmolt_extension_abi_contract.md). Rust/WASM
components and target host imports retain their explicit ABI, ownership and
capability contracts. The [Python execution boundary](../compat/contracts/cpython_bridge_policy.md)
excludes embedded CPython and Python-worker fallback from emitted programs.

## Effects and ownership

Foreign calls declare allocation, I/O, mutation, exceptions and nondeterminism.
Unknown effects remain conservative: optimizations cannot reorder, eliminate or
specialize through them without a valid proof. The boundary records allocation
and deallocation ownership, retained references, callback/reentry behavior,
buffer extent and lifetime, and error transfer. Capability checks precede the
protected operation.

Shared compiler/runtime and C-API primitives provide ecosystem compatibility.
Keep upstream native kernels and Python APIs; do not substitute a Molt-specific
NumPy, pandas or other package implementation. Missing source or ABI support is
an explicit compatibility obligation.

## Qualification

Use the owning ABI and capability authorities for each target. Validate actual
source-recompiled extensions against the selected CPython reference, including
callback order, failure transfer, cleanup and concurrency. Fuzz malformed ABI
inputs and measure boundary costs before selecting specialization or batching.
Development-side reference execution adds no interpreter dependency to guests.
