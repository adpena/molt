# Compatibility Corpus Manifest

Last updated: 2026-03-11

This document defines the operator-facing proof corpus Molt should maintain while pursuing practical CPython 3.12+ parity.
It is not a replacement for the full test suite. It is the minimal high-signal corpus that should answer:
- what claims are we making now?
- what artifacts prove those claims?
- what must stay green before we expand the supported surface?

## Corpus Goals
- Prove **standalone binary** behavior with **no host Python fallback**.
- Prove **native and WASM same-contract** behavior for claimed cross-target features.
- Prove **CPython 3.12+ compatibility** on the highest-value operator paths.
- Prove **libmolt-first** extension strategy rather than accidental CPython ABI dependence.
- Catch regressions in anti-dynamic guardrails that would blur Molt's compiler/runtime identity.

## Corpus Families

### 1. Core language parity corpus
Purpose: prove the compiler/runtime still behaves like CPython on the core language surface Molt claims today.

Primary evidence sources:
- `tests/differential/basic/`
- `tests/differential/INDEX.md`
- `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`

Required properties:
- runs against CPython oracle
- covers core control flow, functions, containers, exceptions, async/generator behavior, and high-value builtin semantics
- produces machine-readable results suitable for CI gating

### 2. Strict stdlib lowering corpus
Purpose: prove compiled execution uses intrinsic/runtime-owned behavior rather than host fallback.

Primary evidence sources:
- `tools/check_stdlib_intrinsics.py`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
- `docs/spec/areas/compat/plans/stdlib_lowering_plan.md`

Required properties:
- rejects `_py_*` and equivalent fallback patterns
- enforces transitive closure rules for strict/import-critical lanes
- keeps generated audit outputs synchronized with code

### 3. Standalone binary proof corpus
Purpose: prove native output is genuinely self-contained.

Primary evidence sources:
- workflow in [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
- representative binaries built from `examples/`, `tests/differential/`, and selected demo/operator targets

Required properties:
- binary runs without host Python
- runtime behavior does not depend on `python`, `python3`, `PYTHONPATH`, or host stdlib discovery
- failure modes are explicit when capabilities/resources are absent

### 4. Native/WASM same-contract corpus
Purpose: prove that features claimed on both targets obey the same contract.

Primary evidence sources:
- `tests/test_wasm_*`
- `tools/bench_wasm.py`
- linked-runner workflow in the standalone proof document
- WASM specs under `docs/spec/areas/wasm/`

Required properties:
- same input/output and error-shape expectations where cross-target support is claimed
- explicit exceptions documented where a capability model or platform constraint differs
- no silent divergence

### 5. libmolt extension corpus
Purpose: prove extension support is advancing through `libmolt`, not through accidental CPython ABI coupling.

Primary evidence sources:
- `include/molt/molt.h`
- `include/Python.h`
- `docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md`
- `docs/spec/areas/compat/surfaces/c_api/`
- extension build/scan/verify/publish tooling

Required properties:
- source-compat and stable-ABI tiers are kept distinct
- representative extension probes compile against shipped headers
- unsupported CPython ABI dependencies fail clearly

### 6. Anti-dynamic guardrail corpus
Purpose: preserve Molt's identity as a compiler/runtime rather than a hidden CPython launcher.

Primary evidence sources:
- `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`
- `docs/spec/areas/compat/contracts/cpython_bridge_policy.md`
- negative/expected-failure dynamic-execution probes

Required properties:
- unrestricted `eval`/`exec` are not silently enabled
- bridge paths remain explicit and non-default
- compiled binaries do not widen trust boundaries without documentation and gating

## Promotion rule
A feature should be treated as promoted operator surface only when the relevant corpus family exists and is wired into repeatable validation.

At minimum, promotion requires:
- canonical spec/status update
- at least one named proof corpus family
- reproducible commands or CI lane
- documented pass/fail contract

## Immediate highest-value additions
These are the operator artifacts worth maintaining right now:
1. `SUPPORTED.md`
2. this compatibility corpus manifest
3. standalone binary proof workflow doc
4. continued `libmolt` ABI contract refinement and extension proof probes

## Non-goals
This manifest does not try to enumerate every single test file.
It defines the proof buckets that release decisions should speak in.
