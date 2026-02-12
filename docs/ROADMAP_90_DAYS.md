# 90-Day Roadmap

This plan sequences near-term work described in `ROADMAP.md` and prioritizes doc alignment, runtime hardening, and measurable correctness/perf gates.

Document role:
- Canonical state lives in `docs/spec/STATUS.md`.
- Active long-horizon plan lives in `ROADMAP.md`.
- This document is the rolling 90-day execution slice and must stay aligned with both.

## Execution Tracker (2026-02-11)
- [x] Month 1: define determinism/security enforcement checklists.
  - Delivered: `docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`.
- [x] Month 1: define minimum must-pass test matrix for Tier 0/1 + diff parity.
  - Delivered: `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`.
- [ ] Month 1: finalize core specs (`0000-vision`, `0100_MOLT_IR`) with explicit sign-off.
  - Partial delivered: sign-off readiness sections and implementation-status alignment added in `docs/spec/areas/core/0000-vision.md` and `docs/spec/areas/compiler/0100_MOLT_IR.md`; explicit owner approval still pending.
- [x] Month 1: align testing + CI docs with current workflow and gate sequence.
  - Delivered: `docs/spec/areas/testing/0007-testing.md`, `docs/spec/areas/tooling/0011-ci.md`.
- [ ] Month 2 and Month 3 deliverables pending.
- [x] IR inventory gate now asserts frontend emit/lowering inventory, required
  native/wasm dedicated-lane coverage, and behavior-level semantic invariants
  (`tools/check_molt_ir_ops.py`).
- [x] IR probe execution/failure-queue linkage is now mandatory in CI after
  `diff-basic` via
  `tools/check_molt_ir_ops.py --require-probe-execution`.

## Version Policy
Molt targets **Python 3.12+** semantics only. When 3.12/3.13/3.14 diverge,
document the chosen target in specs/tests.

## Month 1: Spec and tooling alignment
- Finalize core specs: `docs/spec/areas/core/0000-vision.md` and `docs/spec/areas/compiler/0100_MOLT_IR.md`.
- Testing/CI docs alignment: updated in `docs/spec/areas/testing/0007-testing.md` and `docs/spec/areas/tooling/0011-ci.md` to reference and enforce the must-pass matrix.
- Determinism/security enforcement checklists (lockfiles, SBOM, capability gating): delivered in `docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`.
- Minimum “must-pass” test matrix for Tier 0/1 semantics and molt-diff parity: delivered in `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`.

## Month 2: Runtime + compiler hardening
- Implement or scaffold RC + incremental cycle detection per `docs/spec/areas/runtime/0003-runtime.md`.
- Add a minimal tasks/channels runtime skeleton and gated API in `molt`.
- Promote MsgPack/CBOR parsing as the default structured codec; keep JSON only for compatibility/debug.
- Wire guard/deopt instrumentation to emit `molt_runtime_feedback.json` (MPA loop).
  - Partial delivered: runtime feedback emission + schema validation gate (`MOLT_RUNTIME_FEEDBACK`, `MOLT_RUNTIME_FEEDBACK_FILE`, `tools/check_runtime_feedback.py`) are wired; broader MPA consumption loop remains pending.
- Keep `molt run` compiled-by-default; use `molt compare` (or a dedicated parity runner) for CPython parity testing.

## Month 3: Packaging + validation gates
- Add benchmark regression checks and publish results in CI.
- Add load-time signature enforcement (CycloneDX/SPDX + signing hooks + publish/verify checks now ship).
- Add portable WASM ABI smoke tests (native + wasm32 targets).
- Draft native WebSocket + streaming I/O plan aligned with tasks/channels and multicore scaling tests.
- Kick off DataFrame Phase 1 Plan IR scaffolding for Polars/DuckDB delegation.
