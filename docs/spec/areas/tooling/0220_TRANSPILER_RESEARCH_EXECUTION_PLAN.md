# 0220 Transpiler Research Execution Plan

Status: Draft (active execution)  
Owners: Backend + Compiler lanes  
Last updated: 2026-03-08

## Purpose

Turn external transpiler/compiler research into production changes for Molt `--target rust` and `--target luau` with measurable parity, performance, determinism, and maintainability gains.

## Scope

- In scope:
  - Rust source transpiler backend (`runtime/molt-backend/src/rust.rs`)
  - Luau transpiler backend (`runtime/molt-backend/src/luau.rs`)
  - Build/test harness reliability for transpiler correctness suites
  - Validation and benchmark automation for transpiler lanes
- Out of scope:
  - Any CPython runtime fallback for compiled outputs
  - Dynamic policy expansion (`eval`/`exec`/unrestricted reflection)

## Research Inputs (Primary Sources)

- Rust incremental/query model:
  - https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation.html
  - https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html
- Rust optimization ordering and monomorphization:
  - https://rustc-dev-guide.rust-lang.org/mir/optimizations.html
  - https://rustc-dev-guide.rust-lang.org/backend/monomorph.html
- Rule-based lowering inspiration:
  - https://github.com/bytecodealliance/wasmtime/tree/main/cranelift/isle
- Luau performance/type system:
  - https://luau.org/performance
  - https://luau.org/types
  - https://luau.org/lint/
  - https://rfcs.luau.org/syntax-attribute-functions-native.html
- Cross-language transpilation research:
  - https://arxiv.org/abs/2006.03511 (TransCoder)
  - https://arxiv.org/abs/2110.06773 (unit-test guided translation)
- Open-source transpiler/tooling references:
  - https://github.com/py2many/py2many
  - https://github.com/immunant/c2rust
  - https://github.com/roblox-ts/roblox-ts

## Learning-to-Implementation Loop (Non-Optional)

1. Ingest
- Add source notes to a short ADR-style entry in this file (appendix log).
- Extract only actionable techniques that fit Molt constraints.

2. Pilot
- Implement smallest production-shaped experiment behind explicit flags where needed.
- Add parity and perf checks in the same change.

3. Validate
- Run transpiler correctness suites + targeted differential checks.
- Record reproducible command output and baseline deltas.

4. Promote
- Remove temporary gates once metrics clear promotion bars.
- Update docs/spec and test matrix in same change.

## Workstreams

### WS1: Transpiler Harness Determinism And Throughput

Target: remove infra-induced false negatives and lock contention in transpiler tests.

Plan:
- Prefer direct interpreter invocation (`sys.executable -m molt.cli`) inside transpiler tests.
- Keep build artifacts and tmp on external volume only.
- Make build timeouts configurable (`MOLT_RUST_BUILD_TIMEOUT`, `MOLT_LUAU_BUILD_TIMEOUT`).

Promotion bar:
- No spurious timeout failures across 3 consecutive targeted runs.

### WS2: Rust Backend Incremental Rebuild Scope

Target: reduce rebuild blast radius and warm-build latency.

Plan:
- Introduce fingerprinted intermediate artifacts for rust transpiler stages.
- Partition stable helper emission from volatile function emission.

Promotion bar:
- Warm rebuild after 1-function edit improves >=25%.
- No parity regressions on `tests/rust/test_molt_rust_correctness.py`.

### WS3: Rust Lowering Safety Invariants

Target: prevent alias/writeback regressions and mutation-propagation bugs.

Plan:
- Add explicit regression tests for alias chains, swap-heavy loops, nested mutation.
- Harden alias propagation logic against cycles and stale parent links.

Promotion bar:
- Targeted algorithm set (bubble sort/matrix multiply/collatz + added alias tests) always green.

### WS4: Declarative Lowering Pilot

Target: reduce ad-hoc lowering drift by introducing table/rule-driven lowering for selected op families.

Plan:
- Start with arithmetic/comparison ops only.
- Keep legacy lowering path for equivalence checks during rollout.

Promotion bar:
- Behavior-equivalent output across pilot corpus.
- No measurable compile-time regression >5%.

### WS5: Luau Emission Performance Passes

Target: improve runtime performance of emitted Luau while preserving semantics.

Plan:
- Expand safe global localization and temporary sinking.
- Add opt-level aware emission hooks and controlled `@native` use on hot numeric kernels.

Promotion bar:
- >=10% improvement on Luau hotpath benchmark subset.
- Zero parity diffs on maintained Luau correctness corpus.

### WS6: Luau Static Analysis Gate

Target: catch translational quality issues before runtime.

Plan:
- Add a `luau-analyze`/lint check over transpiled fixtures in warn mode.
- Trend warning classes and ratchet down over time.

Promotion bar:
- UnknownGlobal + LocalShadow classes reduced >=50% from baseline.

### WS7: Translation-Validation Corpus Expansion

Target: keep research-inspired changes honest against real program shapes.

Plan:
- Build a curated corpus containing loops, mutation aliasing, nested functions, closures, dict/list mix.
- Run nightly for both `--target rust` and `--target luau`.

Promotion bar:
- No new behavioral regressions in nightly corpus for 2 weeks.

### WS8: Documentation And Decision Hygiene

Target: keep architecture and validation guarantees explicit.

Plan:
- For each promoted technique: document invariants, failure modes, rollback path.
- Keep `docs/spec/README.md`, `docs/INDEX.md`, `docs/spec/STATUS.md`, and `ROADMAP.md` synchronized when scope moves.

Promotion bar:
- No untracked architecture change lands without spec update.

## 30 / 60 / 90 Day Delivery Plan

## Day 0-30

- Land WS1 across Rust and Luau test harnesses.
- Establish baseline metrics:
  - transpiler compile latency (cold/warm)
  - targeted correctness suite pass rates
  - lock-contention and timeout incidence
- Start WS3 targeted regression additions.

Gate:
- `tests/rust/test_molt_rust_correctness.py -k "bubble_sort or matrix_multiply or collatz"` green.
- `tests/luau/test_molt_luau_correctness.py -k "bubble_sort or matrix_multiply or collatz"` green.

## Day 31-60

- Ship WS2 pilot (incremental rebuild scope reduction).
- Ship WS4 pilot subset (declarative lowering for arithmetic/comparison).
- Ship first WS5 performance pass bundle.

Gate:
- Warm Rust transpiler rebuild >=25% faster on pilot benchmark.
- No regression in targeted + nightly translation-validation corpus.

## Day 61-90

- Expand WS4 coverage beyond pilot ops.
- Enable WS6 warning trend gating for curated corpus.
- Promote successful WS5 optimizations to default emission paths.

Gate:
- Full transpiler correctness suites stable.
- Luau hotpath p95 runtime >=15% better vs baseline.
- Deterministic output hash stability for repeated identical builds.

## Required Validation Commands

Always run with external-volume env defaults.

- Rust targeted correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/rust/test_molt_rust_correctness.py -k "bubble_sort or matrix_multiply or collatz"`
- Luau targeted correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/luau/test_molt_luau_correctness.py -k "bubble_sort or matrix_multiply or collatz"`
- Rust full suite:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/rust/test_molt_rust_correctness.py`
- Luau full suite:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/luau/test_molt_luau_correctness.py`

## Risk Register

- Risk: backend/cargo lock contention creates false timeout failures.
  - Mitigation: direct interpreter subprocesses, stale-job cleanup, configurable timeouts.
- Risk: optimization pass introduces semantic drift.
  - Mitigation: targeted algorithm regressions + nightly translation-validation corpus.
- Risk: local perf wins regress determinism.
  - Mitigation: repeated build hash checks and cache-invalidation audits.

## Appendix A: Research Assimilation Log

- 2026-03-08
  - Added research-driven workstreams for Rust incremental/query patterns, declarative lowering, and Luau optimization/lint integration.
  - Added explicit promotion bars and a 30/60/90 day delivery model for transpiler lanes.

## Appendix B: Execution Snapshot (2026-03-08)

Baseline command results captured in this turn:

- Rust targeted correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/rust/test_molt_rust_correctness.py -k \"bubble_sort or matrix_multiply or collatz\"`
  - Result: `3 passed, 61 deselected in 254.25s`
- Rust full correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/rust/test_molt_rust_correctness.py`
  - Result: `64 passed, 0 failed, 0 skipped in 1899.39s`
- Luau targeted correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/luau/test_molt_luau_correctness.py -k \"bubble_sort or matrix_multiply or collatz\"`
  - Result: `2 passed, 57 deselected in 82.16s`
- Luau full correctness:
  - `/Users/adpena/PycharmProjects/molt/.venv/bin/python -m pytest -q tests/luau/test_molt_luau_correctness.py`
  - Result: `59 passed in 1146.83s`

Execution notes:

- Transpiler harness reliability depends on avoiding nested `uv run` subprocess contention inside tests.
- For transpiler subprocesses, prefer direct interpreter invocation (`sys.executable -m molt.cli`) with external-volume env defaults.
- Keep build timeout knobs explicit (`MOLT_RUST_BUILD_TIMEOUT`, `MOLT_LUAU_BUILD_TIMEOUT`) and treat timeout spikes as infra signals before semantic regressions.
