# 90-Day Roadmap

This plan sequences near-term work described in `ROADMAP.md` and prioritizes doc alignment, runtime hardening, and measurable correctness/perf gates.

## Month 1: Spec and tooling alignment
- Finalize core specs: `docs/spec/0000-vision.md` and `docs/spec/0100_MOLT_IR.md`.
- Align testing and CI docs with the current workflow and repo layout.
- Define determinism/security enforcement checklists (lockfiles, SBOM, capability gating).
- Establish a minimum “must-pass” test matrix for Tier 0/1 semantics and molt-diff parity.

## Month 2: Runtime + compiler hardening
- Implement or scaffold RC + incremental cycle detection per `docs/spec/0003-runtime.md`.
- Add a minimal tasks/channels runtime skeleton and gated API in `molt`.
- Wire guard/deopt instrumentation to emit `molt_runtime_feedback.json` (MPA loop).
- Add `molt run` slow-path execution for parity testing.

## Month 3: Packaging + validation gates
- Add benchmark regression checks and publish results in CI.
- Implement SBOM generation and signing hooks in the CLI.
- Add portable WASM ABI smoke tests (native + wasm32 targets).
- Kick off DataFrame Phase 1 Plan IR scaffolding for Polars/DuckDB delegation.
