# Compatibility Diagnostics Contract

**Spec ID:** 0211
**Status:** Active
**Owner:** frontend + runtime + tooling

Molt targets full CPython `>=3.12` parity within the
[verified subset](verified_subset_contract.md). Unsupported implementations and
unqualified cells remain release work. The
[dynamic-semantics policy](dynamic_execution_policy_contract.md) owns justified
restrictions on excessive dynamism.

**No host CPython in binaries:** compiled outputs execute without a host or
fallback interpreter. The [Python execution boundary](cpython_bridge_policy.md)
also applies to explicitly selected compatibility policies.

## Policy and outcomes

`molt build --fallback {error|bridge}` and `[tool.molt.build].fallback` select
how existing frontend compatibility sites handle unavailable implementations:

| Policy | Unsupported operation |
| --- | --- |
| `error` (default) | Fail compilation with the feature, location and available guidance. |
| `bridge` | Permit the existing frontend's deferred compatibility paths with warnings. A path lowered to `bridge_unavailable` fails when reached; other unsupported sites can still reject compilation. |

The option does not enable execution through CPython. Native/WASM lower the
unavailable marker to their runtime boundary; native currently prints
`Molt bridge unavailable: ...` and exits with status 1. Other shipped backend
implementations and target qualification remain governed by their owning cells.

Supported generic calls and guarded specialization retain Molt runtime dispatch
and the required Python effects. A guard does not imply a CPython deoptimization
path, and `error` does not prohibit supported guarded operations.

## Diagnostics

`molt.compat.CompatibilityReporter` owns compatibility warnings and errors.
Warnings identify the tier, impact, feature and source location; available detail
and replacement guidance follow. Errors include the verified-subset boundary
and a specific replacement or default workaround. Running reference CPython
separately is development guidance, never an emitted fallback path.

`MOLT_COMPAT_WARNINGS=0` suppresses warnings. It grants no capability and changes
neither compilation refusal nor an unavailable operation's runtime behavior.

## Authorities

- CLI selection: `molt.cli.entrypoint_parser` and `entrypoint_dispatch`.
- Diagnostics: `molt.compat`.
- Frontend unavailable operation: `frontend/lowering/emission_core.py`.
- Native/WASM runtime boundary: `molt_bridge_unavailable` in
  `runtime/molt-runtime/src/builtins/platform.rs` and the generated WASM ABI.
- Extension support: [libmolt ABI contract](libmolt_extension_abi_contract.md).
