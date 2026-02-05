# 90-Day Roadmap

This plan sequences near-term work described in `ROADMAP.md` and prioritizes doc alignment, runtime hardening, and measurable correctness/perf gates.

## Version Policy
Molt targets **Python 3.12+** semantics only. When 3.12/3.13/3.14 diverge,
document the chosen target in specs/tests.

## Month 1: Spec and tooling alignment
- Finalize core specs: `docs/spec/areas/core/0000-vision.md` and `docs/spec/areas/compiler/0100_MOLT_IR.md`.
- Align testing and CI docs with the current workflow and repo layout.
- Define determinism/security enforcement checklists (lockfiles, SBOM, capability gating).
- Establish a minimum “must-pass” test matrix for Tier 0/1 semantics and molt-diff parity.

## Month 2: Runtime + compiler hardening
- Implement or scaffold RC + incremental cycle detection per `docs/spec/areas/runtime/0003-runtime.md`.
- Add a minimal tasks/channels runtime skeleton and gated API in `molt`.
- Promote MsgPack/CBOR parsing as the default structured codec; keep JSON only for compatibility/debug.
- Wire guard/deopt instrumentation to emit `molt_runtime_feedback.json` (MPA loop).
- Keep `molt run` compiled-by-default; use `molt compare` (or a dedicated parity runner) for CPython parity testing.

## Month 3: Packaging + validation gates
- Add benchmark regression checks and publish results in CI.
- Add load-time signature enforcement (CycloneDX/SPDX + signing hooks + publish/verify checks now ship).
- Add portable WASM ABI smoke tests (native + wasm32 targets).
- Draft native WebSocket + streaming I/O plan aligned with tasks/channels and multicore scaling tests.
- Kick off DataFrame Phase 1 Plan IR scaffolding for Polars/DuckDB delegation.
