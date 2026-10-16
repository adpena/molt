# Stdlib Lowering Plan (Intrinsic-First, CPython 3.12+)

**Status:** Active
**Owner:** stdlib + runtime + tooling + frontend + tests
**Scope:** CPython 3.12/3.13/3.14 stdlib union, top-level + submodule coverage, intrinsic-first lowering for native and wasm.

## Mission
Drive Molt to a strict stdlib posture where:
1. every required stdlib module/submodule is canonically present,
2. every required behavior is implemented: intrinsic-backed where the runtime must own it, or compiled from Python source (`python-compiled`, spec 0016), with explicit fail-fast for unsupported dynamism,
3. CI enforces no silent fallback paths and no coverage regressions,
4. native + wasm behavior moves in lockstep for covered semantics.

## Non-Negotiable Gates
1. a required intrinsic that nothing reads hard fails (no anchors, spec 0016).
2. top-level union coverage hard fail.
3. submodule union coverage hard fail.
4. module/package collision hard fail.
5. selected/bootstrap roots must have implemented source owners; inventory does not require every optional import to succeed.
6. required runtime symbols and module custody must be admitted; ordinary Python exception behavior is qualified through API differential tests.
7. `intrinsic-partial` and `stub` ratchets hard fail (`tools/stdlib_intrinsics_ratchet.json`).
8. memory regressions in differential runs are blockers (`MOLT_DIFF_MEASURE_RSS=1`, process cap 10GB).

## Program Board
| Phase | Scope | Status | Entry Gate | Exit Gate |
| --- | --- | --- | --- | --- |
| 0 | Enforcement spine | Completed | Intrinsics audit in CI | `tools/check_stdlib_intrinsics.py` + `tools/check_core_lane_lowering.py` green |
| 1 | Core-lane closure lowering | Completed | Phase 0 complete | `tests/differential/basic/CORE_TESTS.txt` green + closure implemented |
| 2 | Concurrency substrate | In progress | Phase 1 complete | `socket` + `threading` + `asyncio` clusters green in native and wasm |
| 3 | Core-adjacent stdlib | Planned | Phase 2 complete | target families promoted to `intrinsic-backed` |
| 4 | Capability-gated long tail | Planned | Phase 3 complete | shipped compiled surface has no `stub` modules |

Specialized long-tail plan:
- Tkinter family (`_tkinter`, `tkinter`, `ttk`, dialogs) is tracked in
  `docs/spec/areas/compat/plans/tkinter_lowering_plan.md`.

## Acceptance Template (Required For Each Module Conversion)
1. Choose the semantic owner: compiled Python algorithm or runtime primitive.
2. For a runtime primitive, update its manifest and generated bindings together.
3. Keep one implementation owner; argument/error/capability handling follows its real API.
4. Targeted differential tests prove supported semantics, including failure propagation.
5. Run the complete audit once with `--critical-allowlist`; combine `--allowlist-modules` for additional selected roots.
6. Qualify native and WASM behavior and the relevant performance/size/compile-cost cells.

Source classification is an inventory, not evidence of runtime parity. Optional
accelerators may be absent when the admitted compiled algorithm has the correct
Python behavior. No host interpreter is consulted. The
[source authority](../../../../internals/source_scan_authority.md#what-source-inventory-establishes)
owns this distinction; coverage and performance require actual target receipts.

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
   - ratchet to `intrinsic-partial == 0` and `stub == 0`

## Differential Execution Policy
Per cluster:
1. targeted native differential lane first,
2. targeted wasm parity lane second,
3. full sweep only after targeted lanes are green.

Required constraints:
- `MOLT_DIFF_MEASURE_RSS=1`
- default adaptive process/tree/global RSS guard plus adaptive child rlimit

## Weekly Scoreboard (Required)
1. intrinsic-backed count
2. intrinsic-partial count
3. python-compiled count
4. stub count
5. missing required top-level entries
6. missing required submodule entries
7. native differential pass %
8. wasm parity pass %
9. memory regressions (count + top offenders)

Any PR that worsens the scoreboard must include explicit exception sign-off and rollback plan.

## Update Workflow (Python Release Advance or Major Sweep)
1. Pin the new CPython tag and commit in `config/cpython_regrtest_sources.toml`,
   then `python3 tools/gen_stdlib_module_union.py --refresh-sources`
2. `python3 tools/gen_stdlib_module_union.py --write`
3. `python3 tools/gen_stdlib_stubs.py --write`
4. `python3 tools/check_stdlib_intrinsics.py --update-doc`
5. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
6. sync rollup docs:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/OPERATIONS.md`
