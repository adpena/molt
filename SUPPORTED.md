# Molt Supported Surface (Operator Snapshot)

Last updated: 2026-03-11

This file is the operator-facing support contract for Molt.
It is intentionally shorter and more decision-oriented than `docs/spec/STATUS.md`.

Canonical deep status: [docs/spec/STATUS.md](docs/spec/STATUS.md)
Canonical roadmap: [ROADMAP.md](ROADMAP.md)
Canonical proof workflow: [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
Canonical corpus manifest: [docs/COMPATIBILITY_CORPUS_MANIFEST.md](docs/COMPATIBILITY_CORPUS_MANIFEST.md)

## Product Doctrine
- Target practical **CPython 3.12+** ecosystem parity.
- Ship **standalone binaries** with **no host Python fallback**.
- Treat **`libmolt` as the primary extension path**.
- Keep dynamic execution and bridge escape hatches **explicit, capability-gated, and non-default**.
- Require **native and WASM same-contract proof**, not separate hand-wavy promises.
- Use Rust and Luau as supporting convergence lanes in service of the core product, not as side quests.

## What Molt currently supports

### Language/runtime baseline
- CPython **3.12+** semantics are the target.
- Native AOT compilation is real and active.
- WASM remains a first-class target, with linked-runner and host-intrinsic support under active verification.
- Differential testing against CPython is a core release gate, not a nice-to-have.

### Packaging and deployment
- Compiled Molt binaries are intended to be **self-contained**.
- Molt does **not** rely on a local host Python installation at runtime.
- `molt package`, `molt verify`, and `molt publish` enforce ABI/capability/trust-policy checks for Molt-packaged artifacts.

### Stdlib direction
- The standard library program is explicitly **Rust-first**.
- Python stdlib files may exist as **thin intrinsic-forwarding wrappers**, but compiled execution is not allowed to depend on Python-only host behavior.
- Current deep coverage and gaps are tracked in `docs/spec/STATUS.md` and the generated stdlib audit docs.

### Extension strategy
- **Primary:** recompiled extensions against `libmolt`.
- **Not supported as product default:** loading CPython wheels/ABI artifacts as-is.
- **Escape hatch only:** explicit bridge policy, capability-gated, non-default, and never a silent fallback.

## What is not a supported promise
- Silent fallback to host CPython.
- “Works because Python is installed on the machine.”
- Binary compatibility with arbitrary CPython extension wheels.
- Dynamic execution as a default production compatibility mechanism.
- Separate native vs WASM semantics promises.

## Operator release gates
A change is not ready to present as supported unless it satisfies all of:
- status/docs updated in the same change when support semantics shift
- differential evidence against CPython where applicable
- standalone proof workflow evidence for native binaries
- no-host-Python fallback preserved
- WASM contract reviewed when the feature is claimed cross-target
- `libmolt`/bridge posture unchanged or explicitly documented

## Source-of-truth map
- Current detailed state: [docs/spec/STATUS.md](docs/spec/STATUS.md)
- Current roadmap/backlog: [ROADMAP.md](ROADMAP.md)
- 90-day execution slice: [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md)
- `libmolt` ABI contract: [docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md](docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md)
- Dynamic execution policy: [docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
- CPython bridge policy: [docs/spec/areas/compat/contracts/cpython_bridge_policy.md](docs/spec/areas/compat/contracts/cpython_bridge_policy.md)
- Standalone proof workflow: [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
- Compatibility corpus manifest: [docs/COMPATIBILITY_CORPUS_MANIFEST.md](docs/COMPATIBILITY_CORPUS_MANIFEST.md)
