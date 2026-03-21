# Molt Performance Handoff Status - 2026-03-21

> **For the next friendly agent:** thank you for picking this up. This note is the current handoff/status document for the performance program. Use it as the execution starting point. The older [2026-03-20-extreme-performance-optimization.md](/Users/adpena/Projects/molt/docs/superpowers/plans/2026-03-20-extreme-performance-optimization.md) remains useful as backlog/reference, but it is not the authoritative execution ledger anymore.

**Scope:** compiler throughput, native runtime/backend performance, benchmark coverage, and current blockers.

**Current local `main`:** `dbfafa1f`

**Important note:** at the time of writing, local `main` is ahead of the last fetched `origin/main` in this workspace. Treat the local checkout as the immediate source of truth for handoff, and re-fetch before doing any integration/push decisions.

---

## Executive Summary

The project is in a materially better state than the earlier raw optimization plan suggests, but it is not “done.”

What is meaningfully better:
- several compiler/CLI orchestration bottlenecks were removed or flattened
- backend feature isolation and target-specific backend builds are much cleaner
- native backend compile graph has been reduced in several real ways
- major native regression work around intrinsic bootstrap and backend lowering was fixed and greened
- warm native build behavior is much healthier than the worst historical samples
- runtime hot-path work has shifted into real low-level wins such as iteration specialization, integer fast paths, and SIMD string/list work

What is still open:
- native benchmark coverage/performance is still incomplete
- WASM benchmark lane is still effectively red
- binary sizes are still too large for the stated north star
- there is active, uncommitted local work in [ops.rs](/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops.rs) that should be treated as in-flight and verified before commit

---

## Most Important Current Facts

### 1. Native regression from earlier handoff is closed

This was the highest-priority blocker before optimization could safely continue.

Closed items:
- intrinsic bootstrap contract restored
- direct native rebuild of `tmp/hello_regress.py` succeeded
- rebuilt native binary executed successfully and printed `ok`
- CLI smoke `test_cli_run_json` was green again

Relevant commit from that closure:
- `bb723337` `fix: restore intrinsic bootstrap contract`

Files involved in the closure:
- [src/_intrinsics.py](/Users/adpena/Projects/molt/src/_intrinsics.py)
- [src/molt/stdlib/_intrinsics.py](/Users/adpena/Projects/molt/src/molt/stdlib/_intrinsics.py)
- [runtime/molt-backend/src/native_backend/function_compiler.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs)
- [runtime/molt-backend/src/lib.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/lib.rs)
- [tests/test_intrinsics_bootstrap_contract.py](/Users/adpena/Projects/molt/tests/test_intrinsics_bootstrap_contract.py)

### 2. Current local dirty file

There is one tracked, uncommitted local modification:
- [ops.rs](/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops.rs)

Current diff summary:
- introduces Neumaier compensated float summation behavior in `molt_sum_builtin`
- also routes some float-sum helpers away from the previous SIMD reduction path

Why this matters:
- it may be a correctness/parity improvement for CPython 3.12+ `sum()` on floats
- it may also be a performance regression on float-heavy paths if landed naively
- this should be treated as active, not-yet-blessed work

Do not hand-wave this in:
- it needs correctness verification
- it needs performance verification
- it should not be mixed with unrelated perf commits

### 3. Latest benchmark state from checked-in artifacts

#### Native suite

Artifact:
- [full_native_20260321_post_fixes.json](/Users/adpena/Projects/molt/bench/results/full_native_20260321_post_fixes.json)

Observed from the artifact:
- `21 / 54` native benchmarks passing
- of the `21` passing native benchmarks, `5` are faster than CPython

This means:
- native execution is partially green, not broadly green
- the benchmark suite is useful but still exposes a lot of unsupported/broken lanes

#### WASM suite

Artifact:
- [full_wasm_20260321.json](/Users/adpena/Projects/molt/bench/results/full_wasm_20260321.json)

Observed from the artifact:
- `0 / 51` WASM benchmarks passing
- failure classes:
  - `build_setup_error`: `2`
  - `runner_error`: `49`

Dominant current WASM failures:
- backend panic in [wasm.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs) at `runtime/molt-backend/src/wasm.rs:10430`
- runtime intrinsic failure in [sys.py](/Users/adpena/Projects/molt/src/molt/stdlib/sys.py): `_as_callable -> RuntimeError: intrinsic unavailable`

This means:
- WASM is still a major red lane
- do not let the existence of recent WASM performance commits create a false sense that the WASM benchmark program is healthy

---

## Recent Landed Work Worth Knowing

Recent local `main` commits at handoff time:
- `dbfafa1f` `perf: pre-build const map for O(1) peephole lookups`
- `e1c4e0d2` `fix: defensive sdiv docs + MachTaskBasicInfo size guard + CallArgs byte tracking`
- `9c57d002` `perf: remove redundant from_utf8 validation before SIMD codepoint count`
- `3a636c3d` `perf: truly inline list/range/tuple iteration in molt_iter_next_unboxed`
- `b10b3d6c` `security: use AtomicPtr for BUILTINS_MODULE_PTR thread safety`
- `259befc2` `fix: address review findings — restore exception_pending + verifier`
- `ff4915d7` `perf: expand fast_int inline paths for neg/shift/div/mod/abs/bool`
- `1c78a1fd` `perf: enable WASM native exception handling by default`
- `df1590d1` `perf: specialize iteration protocol with unboxed iter_next`
- `f438e198` `feat: add allocation byte tracking and RSS profiling`
- `382a043e` `perf: inline integer division and modulo in native backend`
- `908abf7a` `perf: add WASM SIMD (v128) for string and list operations`

Interpretation:
- current work is no longer only “CLI cleanup”
- there is active low-level runtime/backend optimization happening in hot paths
- there is also some safety and verifier hardening mixed in, which is good and should continue

---

## Most Useful Analysis Artifacts

These are the highest-value checked-in artifacts for the next agent:

### Profiling / hotspot analysis
- [profile_analysis_20260320.md](/Users/adpena/Projects/molt/bench/results/profile_analysis_20260320.md)
- [ic_miss_analysis.md](/Users/adpena/Projects/molt/bench/results/ic_miss_analysis.md)
- [string_alloc_analysis.md](/Users/adpena/Projects/molt/bench/results/string_alloc_analysis.md)
- [tuple_boxing_analysis.md](/Users/adpena/Projects/molt/bench/results/tuple_boxing_analysis.md)

### Benchmark datasets
- [full_native_20260321_post_fixes.json](/Users/adpena/Projects/molt/bench/results/full_native_20260321_post_fixes.json)
- [full_wasm_20260321.json](/Users/adpena/Projects/molt/bench/results/full_wasm_20260321.json)
- [full_native_baseline_20260320.json](/Users/adpena/Projects/molt/bench/results/full_native_baseline_20260320.json)

### Backend timing snapshots
- [molt_backend_timings_directdeps_after/build.log](/Users/adpena/Projects/molt/bench/results/molt_backend_timings_directdeps_after/build.log)
- [molt_backend_timings_directdeps_baseline/build.log](/Users/adpena/Projects/molt/bench/results/molt_backend_timings_directdeps_baseline/build.log)
- [molt_backend_timings_native_graph_pruned/build.log](/Users/adpena/Projects/molt/bench/results/molt_backend_timings_native_graph_pruned/build.log)

---

## What The Old Plan Still Gets Right

The older plan remains useful for backlog shape:
- benchmark infrastructure hardening
- backend feature/build isolation
- benchmark suite completeness
- binary size focus
- WASM pipeline recovery
- systematic hotspot optimization

What it gets wrong as a day-to-day status document:
- it is still mostly unchecked even though many parts of that work have already been attempted or partially landed
- it does not reflect later regressions/fixes
- it does not reflect current benchmark artifact reality
- it predates several lower-level runtime/backend optimization tranches now on `main`

So:
- keep it as a backlog/reference document
- do not use it as the authoritative “what is done” record

---

## Recommended Next Steps

### Priority 0: Resolve the live dirty file before branching into new work

File:
- [ops.rs](/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops.rs)

Why first:
- it is already dirty
- it is performance-sensitive
- it changes floating-point sum semantics
- it can easily become a silent perf/correctness footgun if ignored

Recommended verification before committing:
1. targeted runtime/unit tests covering float summation correctness
2. differential parity for CPython 3.12+ float-sum behavior
3. benchmark comparison on float-heavy cases
4. only keep it if both correctness and performance tradeoffs are defensible

### Priority 1: Recover the WASM red lane

Current evidence says WASM is the worst strategic gap.

Immediate subproblems:
- [wasm.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs) panic at `10430`
- [sys.py](/Users/adpena/Projects/molt/src/molt/stdlib/sys.py) intrinsic availability failure under WASM runner

Recommended approach:
1. fix the backend panic first
2. rerun a single focused WASM benchmark
3. then fix the `sys.py` intrinsic failure lane
4. only after that rerun the broader WASM suite

### Priority 2: Improve native benchmark pass rate, not just hot-path speed

Current state is still only `21 / 54` passing in the native benchmark suite.

That means the next agent should not focus only on micro-optimizations. They should also:
- classify failing native benchmarks
- decide whether each failure is:
  - compiler bug
  - runtime bug
  - missing intrinsic
  - unsupported benchmark assumption

### Priority 3: Revisit IC and allocation hotspot analyses

The checked-in analysis documents already point at likely wins:
- call bind IC miss rate
- string allocation pressure
- tuple boxing / iteration shape

These should guide optimization work, rather than random “fast-looking” changes.

---

## Suggested First Commands For The Incoming Agent

### Re-orient and verify current status

```bash
git status --short
git log --oneline --decorate -n 20
```

### Inspect the live dirty file

```bash
git diff -- runtime/molt-runtime/src/object/ops.rs
```

### Re-run the closed native regression checks

```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
export PYTHONPATH=src
export UV_NO_SYNC=1

cargo test -q -p molt-backend --lib --no-default-features --features native-backend
.venv/bin/python -m pytest -q tests/test_intrinsics_bootstrap_contract.py
.venv/bin/python -m pytest -q tests/cli/test_cli_smoke.py -k 'test_cli_run_json'
```

### Re-check benchmark facts before changing direction

```bash
python3 - <<'PY'
import json
from pathlib import Path
for name in [
    'bench/results/full_native_20260321_post_fixes.json',
    'bench/results/full_wasm_20260321.json',
]:
    path = Path(name)
    data = json.loads(path.read_text())
    print(path)
    print('benchmarks', len(data.get('benchmarks', {})))
PY
```

---

## Handoff Warnings

- Do not assume the old optimization program checklist reflects current reality.
- Do not blindly commit [ops.rs](/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops.rs) without verification.
- Do not assume WASM is “close” just because there are recent WASM optimization commits.
- Do not lose the native regression fix context: intrinsic bootstrap and backend native lowering were recently real blockers and must stay green.
- Re-fetch before any merge/push decisions if remote state matters.

---

## Short Closing Note

If you are the next agent: you are not starting from chaos, but you are also not inheriting a perfectly synchronized program doc. The best current truth is:
- recent `main` history
- checked-in benchmark/profiling artifacts
- the live dirty file
- this handoff note

Start by verifying the dirty runtime sum work, then decide whether the next highest-value push is WASM recovery or native benchmark pass-rate expansion.
