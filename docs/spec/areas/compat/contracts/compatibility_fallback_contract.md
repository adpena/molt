# Compatibility Diagnostics Contract

**Spec ID:** 0211
**Status:** Active
**Owner:** frontend + tooling

Molt targets full CPython `>=3.12` parity within the
[verified subset](verified_subset_contract.md). Unsupported implementations and
unqualified cells remain release work. The
[dynamic-semantics policy](dynamic_execution_policy_contract.md) owns justified
restrictions on excessive dynamism.

**No host CPython in binaries:** compiled outputs execute without a host or
fallback interpreter. The [Python execution boundary](cpython_bridge_policy.md)
applies to every compiled output.

## Compile-time legality

An operation without an implementation for the selected compilation cell fails
compilation with its feature, source location and available guidance. The former
`--fallback` option and `[tool.molt.build].fallback` setting are removed; supplying
either is an error. There is no executable unavailable-operation marker or
runtime exit helper. A failed lowering cannot produce a successful artifact.

This follows [MLIR's full-conversion legality model](https://mlir.llvm.org/docs/DialectConversion/#modes-of-conversion):
successful conversion leaves no illegal operations. Molt's generated diagnostic
catalog and closed operation registry make this boundary explicit across
frontends and backends. This does not change the verified subset: an admitted
Python construct that Molt cannot yet implement remains release-blocking work.

Supported generic calls and guarded specialization retain Molt runtime dispatch
and the required Python effects. A guard does not imply a CPython deoptimization
path. Supported guarded operations remain executable within Molt.

Python errors that belong to execution, such as an invalid call's `TypeError`
or a missing import's `ModuleNotFoundError`,
remain runtime behavior. An implementation gap is not permission to reject
valid Python or change when a Python exception occurs.

## Diagnostics

The declarative `frontend_diagnostics.toml` catalog owns stable diagnostic codes
and metadata. Typed `FrontendRejection` values convert through one authority into
`molt.compat.CompatibilityReporter` errors.
Errors identify the impact, feature and source location; available detail
and replacement guidance follow. They include the verified-subset boundary
and a specific replacement or default workaround. Running reference CPython
separately is development guidance, never an emitted fallback path.

The deferred-path warning emitter and its `MOLT_COMPAT_WARNINGS` switch are
retired with their last producer. Supported guarded operations use the runtime's
ordinary dispatch and Python exception contracts.

## Cost and persisted inputs

The compiler rejects at the existing lowering site, without another source walk,
backend launch or emitted dispatch. Compiled programs pay no diagnostic-policy
check, message allocation, runtime import or helper for an unavailable operation.
No speedup for successfully compiled programs is claimed from removing a fatal
placeholder.

CLI requests, frontend worker payloads and lowering contexts carry no selectable
fallback policy. Removing that field changes the lowering-context digest;
semantic compiler fingerprints bind the changed producers and generated
operation registry. Old module records cannot satisfy the new context. Wire IR
containing the removed marker fails the existing checked-operation boundary.
Rebuild old objects and modules against the current compiler/runtime generation;
there is no compatibility shim. Release receipts also omit the redundant fixed
policy field and reject receipts that still contain it. Reference interpreter
execution and conformance remain development-side responsibilities.

## Authorities

- CLI/configuration refusal: `molt.cli.entrypoint_parser` and `config_resolution`.
- Diagnostics: `frontend/frontend_diagnostics.toml`, `frontend/diagnostics.py`
  and `molt.compat`.
- Operation admission: `runtime/molt-ir/src/tir/op_kinds.toml` and its generated
  closed types; frontend census proves registered producer custody.
- Runtime imports: `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml` and its
  generated projections.
- Extension support: [libmolt ABI contract](libmolt_extension_abi_contract.md).
