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

## Closed-world stdlib (architecture)
Molt compiles the stdlib with the user program as one closed world. This section owns how; the
gates below enforce coverage.

1. **One source of semantics.** A module CPython ships in Python compiles from CPython's pinned
   `.py` source (one pin per supported minor, 3.12-3.15), patched only where the verified subset
   forbids a construct; each patch is minimal, provenance-pinned and rebased per release. A module
   CPython ships in C (`_asyncio`, `_json`, `_struct`, `array`, `_sre`) is a Rust intrinsic with
   thin Python glue matching CPython's surface. A Molt-authored Python rewrite of a Python module
   is a defect: a second semantics that drifts (2026-11-09 audit: 142 rewrites and 261 stubs;
   `asyncio/__init__.py` is 2,153 Molt lines where CPython ships 47).
2. **Part of the program.** Whole-program analysis spans user code and the stdlib: dead code
   is removed at function, class and attribute granularity, stdlib functions are specialized to
   their call sites' types, and inlining crosses the boundary.
3. **Summaries, not reanalysis.** Each module gets a content-addressed interprocedural summary
   (effects, return types, written globals and attributes, init purity). Whole-program analysis
   reads summaries, so a no-change rebuild stays at the Cargo, Go and uv bar.
4. **Build-time initialization.** A pure module body runs at compile time; its objects (constant
   tables, enum members, compiled regexes, codec maps) become immortal read-only data, the image
   heap (GraalVM Native Image's build-time class initialization). Runtime module init disappears.
   The whole-program literal pool is the first step.
5. **Target facts are constants.** `sys.platform`, `os.name`, `sys.version_info` and capability
   flags are fixed per target plan; their branches fold, and code for other targets never compiles.
6. **Static binding.** An intrinsic is bound by typed id, never by name. A simple operation (len
   of an exact list, isinstance on an exact type) is a late-TIR expansion every backend inlines; a
   runtime call is kept for real work.
7. **Two tiers, one source.** Development builds link prebuilt, content-addressed generic stdlib
   objects; release builds specialize the whole program. Both read the same summaries.
8. **Imports resolve at compile time.** A module object exists at run time only where the program
   reflects on it; module namespaces are static cells.

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
1. Choose the semantic owner: CPython's own Python source for that target version (compiled,
   patched only where the verified subset excludes a construct, each patch naming its rule), or a
   Rust runtime primitive for a C module or a measured hot path. Never a Molt rewrite of CPython's
   Python (AGENTS.md; HF-820 measures the current rewrites and stubs).
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
