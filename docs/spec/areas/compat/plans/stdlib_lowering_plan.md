# Stdlib Lowering Plan (Intrinsic-First, CPython 3.12+)

**Status:** Active
**Owner:** stdlib + runtime + tooling + frontend + tests
**Scope:** CPython 3.12/3.13/3.14 stdlib union, top-level + submodule coverage, intrinsic-first lowering for native and wasm.

## Mission
Drive Molt to a strict stdlib posture where:
1. every required stdlib module/submodule is canonically present,
2. every required behavior is intrinsic-backed (or explicit fail-fast for unsupported dynamism),
3. CI enforces no silent fallback paths and no coverage regressions,
4. native + wasm behavior moves in lockstep for covered semantics.

## Non-Negotiable Gates
1. `probe-only == 0` and `python-only == 0` hard fail.
2. top-level union coverage hard fail.
3. submodule union coverage hard fail.
4. module/package collision hard fail.
5. forbidden fallback patterns hard fail (strict roots + intrinsic-backed + all-stdlib mode).
6. intrinsic runtime pass-fallback hard fail.
7. intrinsic-partial ratchet hard fail (`tools/stdlib_intrinsics_ratchet.json`).
8. memory regressions in differential runs are blockers (`MOLT_DIFF_MEASURE_RSS=1`, process cap 10GB).

## Program Board
| Phase | Scope | Status | Entry Gate | Exit Gate |
| --- | --- | --- | --- | --- |
| 0 | Enforcement spine | Completed | Intrinsics audit in CI | `tools/check_stdlib_intrinsics.py` + `tools/check_core_lane_lowering.py` green |
| 1 | Core-lane closure lowering | Completed | Phase 0 complete | `tests/differential/basic/CORE_TESTS.txt` green + closure intrinsic-implemented |
| 2 | Concurrency substrate | In progress | Phase 1 complete | `socket` + `threading` + `asyncio` clusters green in native and wasm |
| 3 | Core-adjacent stdlib | Planned | Phase 2 complete | target families promoted to `intrinsic-backed` |
| 4 | Capability-gated long tail | Planned | Phase 3 complete | shipped compiled surface has no `python-only` modules |

Specialized long-tail plan:
- Tkinter family (`_tkinter`, `tkinter`, `ttk`, dialogs) is tracked in
  `docs/spec/areas/compat/plans/tkinter_lowering_plan.md`.

## Acceptance Template (Required For Each Module Conversion)
1. Rust intrinsic implementation.
2. manifest entry in `runtime/molt-runtime/src/intrinsics/manifest.pyi`.
3. regenerated bindings: `src/molt/_intrinsics.pyi` and `runtime/molt-runtime/src/intrinsics/generated.rs`.
4. thin Python shim wiring only (argument normalization/error mapping/capability gating).
5. targeted differential tests for edge semantics.
6. `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only` green for touched lane.
7. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist` green for touched lane.
8. native + wasm parity for touched module(s).

## Blocker-First Execution Order
1. Import system blockers and resolver correctness.
2. Concurrency substrate in strict order:
   - `socket` + `select` + `selectors`
   - `threading`
   - `asyncio`
3. Core-adjacent performance/semantic modules:
   - `builtins`, `math`, `re`, `struct`, `time`, `inspect`, `functools`, `itertools`, `operator`, `contextlib`
4. Capability-gated long tail:
   - import/filesystem cluster
   - data/codecs cluster
   - networking/process cluster
5. Final hardening:
   - ratchet to `intrinsic-partial == 0`

## Differential Execution Policy
Per cluster:
1. targeted native differential lane first,
2. targeted wasm parity lane second,
3. full sweep only after targeted lanes are green.

Required constraints:
- `MOLT_DIFF_MEASURE_RSS=1`
- `MOLT_DIFF_RLIMIT_GB=10`

## Weekly Scoreboard (Required)
1. intrinsic-backed count
2. intrinsic-partial count
3. probe-only count
4. python-only count
5. missing required top-level entries
6. missing required submodule entries
7. native differential pass %
8. wasm parity pass %
9. memory regressions (count + top offenders)

Any PR that worsens the scoreboard must include explicit exception sign-off and rollback plan.

## Update Workflow (Python Release Advance or Major Sweep)
1. `python3 tools/gen_stdlib_module_union.py`
2. `python3 tools/sync_stdlib_top_level_stubs.py --write`
3. `python3 tools/sync_stdlib_submodule_stubs.py --write`
4. `python3 tools/check_stdlib_intrinsics.py --update-doc`
5. `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
6. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
7. sync rollup docs:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/OPERATIONS.md`
