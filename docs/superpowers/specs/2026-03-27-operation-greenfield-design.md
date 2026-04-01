# Operation Greenfield — Sprint Design

## Goal

Kill every P0/P1 correctness blocker, promote WASM to first-class semantic
parity with native, then unlock the third-party ecosystem pipeline — in that
strict order, parallelized within each wave for maximum throughput.

## Current Grounded Status (updated 2026-04-01)

**Working:**
- Zero build errors, zero warnings, fully pushed to `origin/main`
- Conformance baseline: 385 tests, 272 compile (59%), 197 runtime pass (78% parity), 0 SIGSEGVs
- 494/736 tests tagged MOLT_SKIP or xfail for unsupported features
- TIR default-ON with 6+ type specializations, structured CondBranch, nested loop emission
- Cranelift 0.130.0 pinned; sieve correctness verified (9592 primes)
- Split-runtime WASM 1.7MB gzipped fits Cloudflare Workers
- RuntimeVtable (58+ fn ptrs), 20+ extracted crates, thin LTO
- Generators yield all elements; CSE alias resolution fixed

**Blocker status (updated 2026-04-01):**

| ID | Blocker | Severity | Status | Resolution |
|----|---------|----------|--------|------------|
| B1 | stdlib `import sys/os` AttributeError | P0 | **FIXED** | Attribute lookup + stdlib cache invalidation (`ef6e8f540`, `2951c165f`) |
| B2 | Nested indexed loops miscompiled | P0 | **FIXED** | Loop IR restructured, TIR nested loop emission (`d6b3692ac`, `e8f0c7c42`) |
| B3 | Backend daemon lock contention | P1 | **FIXED** | Lock/state ownership resolved |
| B4 | `importlib.machinery` missing in WASM | P1 | **FIXED** | Import resolution boundary fixed |
| B5 | TIR globally disabled | P1 | **FIXED (but new issue)** | TIR default-ON (`d6b3692ac`). **NEW:** TIR strips exception labels → try/except broken (WIP `a2c6be8e0`) |
| B6 | `six`/`click` compilation failures | P2 | **PARTIAL** | `six` test exists but MOLT_SKIP'd (runtime crash); `click` test absent |
| B7 | Tuple subclass MRO | P2 | **FIXED** | MRO lookup corrected |
| B8 | Genexpr enumerate tuple unpacking | P2 | **FIXED** | Generator state machine + tuple unpacking fixed |
| B9 | TIR exception handling | P0 | **MITIGATED** | Functions with check_exception bypass TIR (guard at lib.rs:2974). Exception handler type eval restored (`76cf5a071`). |

**In flight (uncommitted):** ~1,562 lines across 20 files — CLI enhancements,
WASM artifact validation tests, importlib machinery tests, wasm link validation,
bench tooling.

**Existing plan debt:** Three overlapping meta-plans (stabilization,
gap-closure, roadmap continuation) all start with the same "re-establish ground
truth" and "close blockers" sequences. This sprint supersedes all three as the
single canonical execution program.

## Design Principles

1. **Zero code smell.** No workarounds, no silent bypasses, no `SKIP`/`DISABLE`
   flags, no technical debt accepted. Every fix improves the architecture.

2. **Blocker-first.** No optimization wave lands while correctness blockers
   remain unresolved.

3. **Evidence-first.** Every correctness or performance claim cites fresh
   command output under canonical artifact roots.

4. **Upgrade-then-restructure.** For dependency bugs: upgrade to latest first
   (capture upstream fixes), then restructure internal code for durability.

5. **Primitive-first.** Reusable semantics belong in Rust runtime primitives
   exposed by intrinsics. Python wrappers limited to argument normalization and
   error mapping.

6. **Canonical-doc sync mandatory.** When semantics or status change, update
   `docs/spec/STATUS.md`, `ROADMAP.md`, and relevant compat/perf docs in the
   same commit.

## Architecture: Three Waves, Parallelized Within Each

### Wave A: Correctness Fortress

**Objective:** Every P0/P1 blocker dead. Real programs run end-to-end. TIR
re-enabled with proper SSA roundtrip.

#### Track A1: Cranelift Upgrade + Loop IR Restructuring

**Blockers addressed:** B2 (nested loops), partially B5 (TIR)

Strategy:
1. Upgrade Cranelift from 0.130 to latest stable (0.132+).
2. Verify whether the nested loop miscompilation is fixed upstream.
3. Regardless of upstream fix status, restructure loop IR lowering so the
   emitted IR is structurally immune to egraph optimizer reachability analysis
   bugs. The emitted IR for nested loops must be correct by construction, not
   dependent on optimizer behavior.

Files:
- `Cargo.toml` (workspace dependency)
- `runtime/molt-backend/Cargo.toml`
- `runtime/molt-backend/src/native_backend/function_compiler.rs`
- `runtime/molt-backend/src/lib.rs`

Verification:
- `for i in range(3): for j in range(3): print(i, j)` produces correct output
- All 2,617 differential tests remain green
- `cargo test -p molt-backend --features native-backend`

#### Track A2: Stdlib AttributeError Fix

**Blockers addressed:** B1 (stdlib imports crash at runtime)

Strategy:
1. Reproduce with minimal case: `import sys; print(sys.platform)`
2. Trace attribute lookup through compiled stdlib class hierarchy path
3. Fix the attribute resolution so compiled stdlib modules expose attributes
   correctly through the same mechanism that works for user code

Files:
- `runtime/molt-runtime/src/object/ops.rs` (attribute lookup)
- `runtime/molt-runtime/src/builtins/attr.rs`
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (if codegen issue)
- `src/molt/frontend/__init__.py` (if IR emission issue)

Verification:
- `import sys; print(sys.platform)` prints `darwin`
- `import os; print(os.getcwd())` prints working directory
- All 17/18 stdlib imports still pass; target 18/18

#### Track A3: Backend Daemon Lock Contention

**Blockers addressed:** B3 (benchmarks unreliable without `DAEMON=0`)

Strategy:
1. Reproduce the stall with a minimal reproducer
2. Fix lock/state ownership in the daemon/build-state path — not a workaround,
   not "disable daemon for benchmarks"
3. Daemon must be the default for all lanes including benchmarks

Files:
- `runtime/molt-backend/src/main.rs` (daemon entry)
- `runtime/molt-backend/src/lib.rs` (build state)
- `src/molt/cli.py` (daemon client)

Verification:
- `python3 tools/bench.py --bench tests/benchmarks/bench_sum.py` completes
  with daemon ON (no `MOLT_BACKEND_DAEMON=0`)
- No stale-socket artifacts after repeated runs

#### Track A4: TIR Re-enablement (depends on A1)

**Blockers addressed:** B5 (20-30% perf recovery)

Strategy:
1. Fix SSA roundtrip in `lower_to_simple.rs` — operand connections must survive
   the TIR→SSA lowering pass
2. Re-enable TIR globally (remove `MOLT_TIR_OPT=0` default)
3. Run full differential suite with TIR enabled

Files:
- `runtime/molt-backend/src/tir/lower_to_simple.rs`
- `runtime/molt-backend/src/tir/mod.rs`
- `src/molt/frontend/__init__.py` (TIR toggle removal)

Verification:
- `MOLT_TIR_OPT=1` is the default (or env var removed entirely)
- All 2,617 differential tests pass with TIR enabled
- fib(30) benchmark shows measurable improvement over TIR-disabled baseline

#### Track A5: Tuple MRO + Genexpr Enumerate

**Blockers addressed:** B7, B8

Strategy:
1. Fix MRO lookup to find `tuple.__new__` before `object.__new__` for tuple
   subclasses
2. Fix compiled generator expression body to handle tuple unpacking from
   enumerate results

Files:
- `runtime/molt-runtime/src/object/ops.rs` (MRO resolution)
- `src/molt/frontend/__init__.py` (genexpr unpacking emission)
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (if codegen)

Verification:
- `class MyTuple(tuple): pass; MyTuple((42,))` succeeds
- `{k: v for k, v in enumerate(("a", "b"))}` produces `{0: "a", 1: "b"}`

#### Wave A Exit Gate

All of these must pass before Wave C tracks that depend on Wave A can begin:

```bash
cargo test -p molt-backend --features native-backend -- --nocapture
cargo test -p molt-runtime -- --nocapture
PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

### Wave C: WASM First-Class Target

**Objective:** WASM has semantic parity with native. Deploy pipeline is
bulletproof. Size and startup optimized.

#### Track C1: Fix `importlib.machinery` in WASM (depends on Wave A gate)

**Blockers addressed:** B4

Strategy:
1. Fix the import resolution boundary so `importlib.machinery` resolves in the
   wasm lane — real resolution at the importlib boundary, not a caller shim
2. Ensure version-gated absence behavior stays centralized in
   `src/molt/stdlib/importlib/__init__.py`

Files:
- `src/molt/stdlib/importlib/__init__.py`
- `src/molt/stdlib/importlib/machinery.py`
- `runtime/molt-backend/src/wasm.rs`
- `tests/test_wasm_importlib_machinery.py`

Verification:
- WASM linked runner successfully executes `import importlib.machinery`
- `bench_sum.py` and `bench_bytes_find.py` pass on WASM linked runner

#### Track C2: Finish Stdlib Partition (can start during Wave A)

**Blockers addressed:** stdlib-object-partition Tasks 1-2 remaining

Strategy:
1. Implement cache-mode versioning — partition mode/version encoded in cache
   identity so monolithic objects can't be reused under split pipeline
2. Implement explicit native link fingerprinting — link fingerprints hash all
   linked stdlib partition artifacts

Files:
- `src/molt/cli.py`
- `runtime/molt-backend/src/main.rs`
- `tests/cli/test_cli_import_collection.py`

Verification:
- Cache identity changes when stdlib partition mode is enabled vs monolithic
- Link fingerprint changes when any linked stdlib partition artifact changes
- `emit=obj` partial-link contract preserved

#### Track C3: WASM Link/Artifact Pipeline Hardening (depends on C1)

Strategy:
1. Land the uncommitted WASM artifact validation tests (~565 new lines)
2. Land wasm link validation tests (~80 new lines)
3. Add negative-path coverage for malformed artifacts

Files:
- `tests/cli/test_cli_wasm_artifact_validation.py`
- `tests/test_wasm_link_validation.py`
- `tests/wasm_linked_runner.py`
- `tools/wasm_link.py`
- `tools/bench_wasm.py`

Verification:
- All new WASM validation tests pass
- `pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py`

#### Track C4: Cloudflare Split-Runtime Hardening (depends on C3)

Strategy:
1. Test missing `bundle_root` error path
2. Test missing `wrangler_config` error path
3. Test `wrangler_config` outside `bundle_root` rejection
4. Test split-runtime explicit `--output` / `--out-dir`

Files:
- `src/molt/cli.py`
- `tests/cli/test_cli_import_collection.py`

Verification:
- All four negative-path cases produce deterministic errors (not silent failures)
- `pytest -q tests/cli/test_cli_import_collection.py -k deploy_cloudflare`

#### Track C5: WASM Size/Startup Optimization (depends on C1, C2)

Strategy (only after semantic parity is green):
1. Shared trampoline helpers to reduce code duplication
2. Zero-copy string passing between host and wasm
3. Function-table/reference-type improvements
4. Measure and record: wasm binary size, cold start latency, steady-state
   throughput

Files:
- `runtime/molt-backend/src/wasm.rs`
- `runtime/molt-runtime/src/`

Verification:
- Binary size delta recorded
- Cold start measurement recorded
- All WASM tests still pass

#### Wave C Exit Gate

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/test_wasm_importlib_machinery.py
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_partition or stdlib_link_fingerprint or deploy_cloudflare'
```

### Wave B: Ecosystem Unlock

**Objective:** `six -> attrs -> click` pipeline works. Stdlib intrinsic
coverage deepened. IR semantic debt closed.

#### Track B1: Fix `six` Compilation (depends on Wave A stdlib fix)

**Blockers addressed:** B6 (partially)

Strategy:
1. Fix module-scope variable scoping in `_moved_attributes` iteration
2. Ensure compiled module-level loops correctly read/write module dict

Files:
- `src/molt/frontend/__init__.py`
- `runtime/molt-backend/src/native_backend/function_compiler.rs`

Verification:
- `import six` succeeds
- `six.moves.urllib` resolves

#### Track B2: Fix `click` Compilation (depends on B1)

Strategy:
1. Identify and fix backend complexity limits that reject click's module graph
2. No artificial complexity caps — if the IR is valid, it compiles

Files:
- `runtime/molt-backend/src/native_backend/function_compiler.rs`
- `runtime/molt-backend/src/lib.rs`

Verification:
- `import click` succeeds
- Basic click CLI app runs

#### Track B3: `attrs` End-to-End (depends on B1)

Strategy:
1. Run attrs test suite on Molt
2. Fix failures — attrs depends on six, so B1 must be green first

Verification:
- attrs test suite pass rate documented
- Regressions filed and tracked

#### Track B4: Stdlib Intrinsic Closure Tranche 1 (depends on Wave A gate)

Strategy:
1. `functools` — full intrinsic mapping (wraps, partial, reduce, lru_cache)
2. `itertools` — full intrinsic mapping (chain, islice, product, permutations)
3. `operator` — full intrinsic mapping (itemgetter, attrgetter, methodcaller)
4. `math` — full intrinsic mapping (floor, ceil, sqrt, log, sin, cos, pi, e)
5. `json` — full intrinsic mapping (dumps, loads, JSONEncoder, JSONDecoder)

Each module change lands the full intrinsic mapping in the same commit.
Regeneration commands run after each:

```bash
python3 tools/gen_intrinsics.py
python3 tools/gen_stdlib_module_union.py
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --update-doc
```

#### Track B5: IR Semantic Hardening (depends on Wave A gate)

Strategy — priority order:
1. `CALL_INDIRECT` — indirect call resolution
2. `INVOKE_FFI` — foreign function interface invocation
3. `GUARD_TAG` — type tag guards
4. `GUARD_DICT_SHAPE` — dictionary shape guards
5. `INC_REF` / `DEC_REF` — ownership primitives
6. `BORROW` / `RELEASE` — borrow tracking
7. `BOX` / `UNBOX` / `CAST` / `WIDEN` — type conversion

Replace backend panics on malformed IR edge cases with deterministic compile
errors where reachable from user programs.

#### Wave B Exit Gate

```bash
PYTHONPATH=src uv run --python 3.12 python3 -c "import six; import attrs; import click; print('ecosystem green')"
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
UV_NO_SYNC=1 uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib --jobs 1
```

## Parallelization Strategy

```
Timeline:
  Wave A: [A1] [A2] [A3] [A5]  ──→  [A4]
  Wave C:           [C2] ──────────→ [C1] ──→ [C3] ──→ [C4]
                                                   ──→ [C5]
  Wave B:                             [B1] ──→ [B2]
                                      [B4]      [B3]
                                      [B5]
```

A1, A2, A3, A5 run in parallel (no dependencies between them).
C2 starts during Wave A (independent of correctness blockers).
A4 starts after A1 completes (needs correct loop IR).
C1, B1, B4, B5 start after Wave A exit gate.
B2, B3 start after B1.
C3 starts after C1. C4 after C3. C5 after C1+C2.

## Cross-Cutting: Documentation and Linear Discipline

1. **Consolidate plan debt.** The three overlapping meta-plans
   (`molt-stabilization-and-roadmap-continuation`,
   `repo-gap-closure-program`, and the branch integration plan) are superseded
   by this sprint. Mark them as superseded in their headers.

2. **Canonical doc sync.** Every commit that changes behavior or status must
   update `docs/spec/STATUS.md` and `ROADMAP.md` in the same commit.

3. **Linear workspace alignment.** After each wave completion, refresh grouped
   Linear manifests:
   ```bash
   python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
   ```

4. **Benchmark discipline.** Every optimization landing requires:
   - `tools/bench.py` or `tools/bench_wasm.py` artifact
   - Differential probe artifact
   - Binary size delta when relevant
   - Compile-time delta when relevant

## Superseded Plans

This sprint superseded and consolidated:
- `docs/superpowers/plans/2026-03-27-molt-stabilization-and-roadmap-continuation.md` (removed after the 2026-03-30 audit)
- `docs/superpowers/plans/2026-03-27-repo-gap-closure-program.md` (removed after the 2026-03-30 audit)
- `docs/superpowers/plans/2026-03-27-branch-integration-into-main.md` (removed after the 2026-03-30 audit)

The following plans remain active for their specific scope:
- `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md` (separate Linear/workspace ops lane)
- `docs/superpowers/plans/2026-03-29-consolidated-monty-buffa-and-waves.md` (canonical engineering burndown plan)

## Non-Goals

- Dynamic execution bridges (exec/eval/compile)
- Runtime monkeypatching support
- Unrestricted reflection
- async/threading substrate (deferred to post-sprint)
- Formal verification / fuzz gate promotion (deferred to post-sprint)
