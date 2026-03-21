# Molt Extreme Performance Optimization Program

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve Jeff Dean/Gabe Newell/Chris Lattner-grade performance: native compilation faster than CPython on all benchmarks, WASM within 2x of native, Luau/Rust transpilers benchmarked and optimized, with zero regressions and full observability.

**Architecture:** Four-wave attack: (1) Fix infrastructure bugs blocking measurement, (2) Close benchmark coverage gaps across all 4 backends, (3) Systematic hot-path optimization using profile-guided evidence, (4) Architecture-level wins (SIMD, PGO, LTO, memory layout).

**Tech Stack:** Rust (Cranelift 0.128, wasm-encoder), Python (frontend compiler), Node.js (WASM runner), Lune (Luau VM)

**North Star Metrics:**
- Native: median 5x+ CPython, no benchmark slower than CPython
- WASM: median 2x+ CPython, wasm/native ratio < 2.5x
- Build: dev warm < 3s, release warm < 8s
- Binary size: < 3MB for simple programs (native), < 500KB (WASM)
- Startup: < 5ms cold start for hello world

---

## Phase 0: Fix Infrastructure (Unblock Measurement)

### Task 0.1: Harden Backend Feature Isolation

**Problem:** Running WASM and native builds concurrently overwrites `target/<profile>/molt-backend` with incompatible features. Cargo's incremental cache doesn't reliably recompile when only features change.

**Files:**
- Modify: `src/molt/cli.py:16459-16570` (`_ensure_backend_binary`)
- Modify: `src/molt/cli.py:7597-7640` (`_backend_bin_path_cached`)
- Test: `tests/cli/test_cli_backend_isolation.py` (create)

- [ ] **Step 1: Write failing test for feature collision**

```python
# tests/cli/test_cli_backend_isolation.py
"""Verify native and wasm backend builds don't stomp each other."""
import subprocess, sys, os

def test_native_after_wasm_build():
    """Build for wasm, then native — native must still work."""
    env = {**os.environ, "PYTHONPATH": "src", "UV_NO_SYNC": "1"}
    # Build wasm first
    subprocess.run(
        [sys.executable, "-m", "molt.cli", "build", "--target", "wasm",
         "--json", "tests/benchmarks/bench_sum.py"],
        env=env, capture_output=True, timeout=120)
    # Now build native — this MUST succeed
    r = subprocess.run(
        [sys.executable, "-m", "molt.cli", "build", "--trusted", "--json",
         "tests/benchmarks/bench_sum.py"],
        env=env, capture_output=True, text=True, timeout=120)
    assert r.returncode == 0, f"Native build failed after wasm: {r.stderr}"
```

- [ ] **Step 2: Run test — expect FAIL (feature collision)**

Run: `UV_NO_SYNC=1 uv run --python 3.12 pytest tests/cli/test_cli_backend_isolation.py -v`

- [ ] **Step 3: Add post-build verification to `_ensure_backend_binary`**

After `cargo build` succeeds, probe the built binary to verify it supports the requested feature set before accepting it. If the binary doesn't match (Cargo cache confusion), force a clean rebuild of just the backend crate:

In `_ensure_backend_binary` after the cargo build succeeds (around line 16557):

```python
# After successful cargo build, verify the binary actually has the right features.
# Cargo's incremental cache can silently skip recompilation when only features change.
_cargo_bin = backend_bin.parent / "molt-backend"
if os.name == "nt":
    _cargo_bin = _cargo_bin.with_suffix(".exe")
if _cargo_bin.exists():
    _probe_target = "native" if "native-backend" in backend_features else "wasm"
    _probe_input = json.dumps({
        "functions": [], "module": "__probe__", "entry": "main",
        "metadata": {"target": _probe_target, "deterministic": True}
    }).encode()
    _probe = subprocess.run(
        [str(_cargo_bin)], input=_probe_input,
        capture_output=True, timeout=10)
    if _probe.returncode != 0 and b"without" in _probe.stderr and b"support" in _probe.stderr:
        # Binary has wrong features — Cargo cache lied. Force clean rebuild.
        if not json_output:
            print("Backend feature mismatch detected; forcing clean rebuild...")
        subprocess.run(
            ["cargo", "clean", "-p", "molt-backend", "--profile", cargo_profile],
            cwd=project_root, capture_output=True, timeout=60)
        build = _run_cargo_with_sccache_retry(
            cmd, cwd=project_root, env=build_env,
            timeout=cargo_timeout, json_output=json_output,
            label="Backend rebuild (feature fix)")
        if build.returncode != 0:
            return False
```

Also: when `backend_features != _DEFAULT_BACKEND_FEATURES`, copy the cargo output to the feature-tagged path:

```python
if backend_features != _DEFAULT_BACKEND_FEATURES:
    cargo_output = backend_bin.parent / ("molt-backend" + (".exe" if os.name == "nt" else ""))
    if cargo_output.exists() and cargo_output != backend_bin:
        shutil.copy2(cargo_output, backend_bin)
```

- [ ] **Step 4: Run test — expect PASS**
- [ ] **Step 5: Commit**

### Task 0.2: Add Daemon Health Recovery to Benchmark Harness

**Problem:** When the backend daemon dies, all subsequent builds in a bench run fail silently. The harness needs a health check + restart.

**Files:**
- Modify: `tools/bench.py:413-458` (`prepare_molt_binary`)

- [ ] **Step 1: Add daemon health probe before each benchmark build**

In `prepare_molt_binary`, before invoking the CLI, add:

```python
# Kill any stale daemons if the backend binary was recently rebuilt
_prune_backend_daemons()
```

The function `_prune_backend_daemons()` already exists at line 265 — just call it at the top of `prepare_molt_binary`.

- [ ] **Step 2: Verify the full bench suite completes without cascade failures**

Run: `UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench.py --smoke --samples 1`
Expected: Both smoke benchmarks produce Molt results (no "Molt build/run failed").

- [ ] **Step 3: Commit**

### Task 0.3: Investigate and Fix Binary Size Bloat

**Problem:** Current benchmark builds produce ~31MB binaries vs baseline ~1.8MB. Full runtime static linking is happening unnecessarily.

**Files:**
- Investigate: `src/molt/cli.py` (link configuration)
- Investigate: `runtime/molt-runtime/Cargo.toml` (feature flags)

- [ ] **Step 1: Compare link flags between baseline era and now**

```bash
# Check what features are being pulled in
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli build --trusted --json tests/benchmarks/bench_sum.py 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d['data'], indent=2))" | grep -E "size|profile|artifacts"
```

- [ ] **Step 2: Identify which stdlib features are being linked for benchmark programs**

Benchmark programs like `bench_sum.py` should NOT need crypto, compression, networking. Check if `stdlib_full` is being used instead of `stdlib_micro`.

- [ ] **Step 3: Add `--stdlib-profile micro` for benchmark builds that don't need full stdlib**
- [ ] **Step 4: Verify binary size drops to < 3MB**
- [ ] **Step 5: Commit**

---

## Phase 1: Benchmark Suite Completeness

### Task 1.1: Fix WASM Benchmark Pipeline

**Problem:** All 7 WASM baselines show 0/N. The WASM pipeline has been broken since at least 2026-03-17 with "undeclared reference to function #90" errors in linked mode and "Direct-link mode unavailable" in unlinked mode.

**Files:**
- Investigate: `runtime/molt-backend/src/wasm.rs` (import ID registration)
- Modify: `bench/wasm_baseline.json` (update with green results)

- [ ] **Step 1: Run targeted WASM bench to capture current error**

```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_wasm.py \
  --linked --allow-unlinked --samples 1 --warmup 0 \
  --bench bench_sum --runner node --control-runner none 2>&1
```

- [ ] **Step 2: Fix the specific import ID/function reference mismatch in wasm.rs**

The 2026-02-24 progress log says this was fixed once ("registered missing `sys_*` import ids"). Check if new imports were added since then that aren't registered.

- [ ] **Step 3: Green all 7 WASM baseline benchmarks**
- [ ] **Step 4: Update `bench/wasm_baseline.json` with real data**
- [ ] **Step 5: Commit**

### Task 1.2: Expand Native Baseline from 16 to Full 51 Benchmarks

**Problem:** Only 31% of available benchmarks are in the baseline. Collections, async, JSON, CSV, ETL are completely unmeasured.

**Files:**
- Modify: `bench/baseline.json`
- Potentially modify: benchmark programs that fail to build

- [ ] **Step 1: Run all 51 benchmarks and triage failures**

```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench.py --samples 5 \
  --json-out bench/results/full_native_51_$(date +%Y%m%d).json 2>&1 | tee /tmp/bench_full.log
```

- [ ] **Step 2: For each failing benchmark, classify as:**
  - (a) Compiler bug — file issue and mark expected-fail
  - (b) Missing intrinsic — add to lowering backlog
  - (c) Timeout — increase timeout or mark as known-slow
  - (d) Test bug — fix the test

- [ ] **Step 3: Update `bench/baseline.json` with all passing benchmarks**
- [ ] **Step 4: Commit**

### Task 1.3: Add Missing Benchmark Categories

**Problem:** No benchmarks for: startup time, memory/GC pressure, compilation throughput per-program, concurrent workloads at scale.

**Files:**
- Create: `tests/benchmarks/bench_startup.py`
- Create: `tests/benchmarks/bench_gc_pressure.py`
- Create: `tests/benchmarks/bench_import_time.py`
- Create: `tests/benchmarks/bench_dict_comprehension.py`
- Create: `tests/benchmarks/bench_set_ops.py`
- Create: `tests/benchmarks/bench_class_hierarchy.py`
- Create: `tests/benchmarks/bench_exception_heavy.py`
- Modify: `tools/bench.py` (add to BENCHMARKS list)

- [ ] **Step 1: Create startup time benchmark**

```python
# tests/benchmarks/bench_startup.py
"""Measures pure startup + teardown overhead (no computation)."""
def main() -> None:
    print("ok")

if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Create GC pressure benchmark**

```python
# tests/benchmarks/bench_gc_pressure.py
"""Measures allocation-heavy workload that stresses GC/refcount."""
def main() -> None:
    results = []
    for i in range(1_000_000):
        results.append({"key": i, "value": [i, i+1, i+2]})
    print(len(results))

if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Create class hierarchy benchmark**

```python
# tests/benchmarks/bench_class_hierarchy.py
"""Measures method dispatch through class hierarchies."""
class Base:
    def compute(self, x: int) -> int:
        return x

class Mid(Base):
    def compute(self, x: int) -> int:
        return super().compute(x) + 1

class Leaf(Mid):
    def compute(self, x: int) -> int:
        return super().compute(x) * 2

def main() -> None:
    obj = Leaf()
    total = 0
    for i in range(5_000_000):
        total += obj.compute(i)
    print(total)

if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Create set operations benchmark**

```python
# tests/benchmarks/bench_set_ops.py
"""Measures set construction, membership, and set algebra."""
def main() -> None:
    a = set(range(0, 100000, 2))
    b = set(range(0, 100000, 3))
    union = a | b
    inter = a & b
    diff = a - b
    sym = a ^ b
    total = len(union) + len(inter) + len(diff) + len(sym)
    print(total)

if __name__ == "__main__":
    main()
```

- [ ] **Step 5: Create exception-heavy benchmark**

```python
# tests/benchmarks/bench_exception_heavy.py
"""Measures exception handling overhead in tight loops."""
def main() -> None:
    total = 0
    for i in range(2_000_000):
        try:
            if i % 3 == 0:
                raise ValueError(i)
            total += i
        except ValueError as e:
            total += int(str(e))
    print(total)

if __name__ == "__main__":
    main()
```

- [ ] **Step 6: Create dict comprehension benchmark**

```python
# tests/benchmarks/bench_dict_comprehension.py
"""Measures dict comprehension and iteration patterns."""
def main() -> None:
    data = {str(i): i * i for i in range(100000)}
    total = sum(v for v in data.values() if v % 2 == 0)
    inverted = {v: k for k, v in data.items()}
    print(total, len(inverted))

if __name__ == "__main__":
    main()
```

- [ ] **Step 7: Create import time benchmark**

```python
# tests/benchmarks/bench_import_time.py
"""Measures repeated module import overhead."""
import importlib

def main() -> None:
    total = 0
    for i in range(10000):
        mod = importlib.import_module("json")
        total += len(dir(mod))
    print(total)

if __name__ == "__main__":
    main()
```

- [ ] **Step 8: Add all new benchmarks to BENCHMARKS list in `tools/bench.py`**
- [ ] **Step 9: Add all new benchmarks to BENCHMARKS list in `tools/bench_wasm.py`**
- [ ] **Step 10: Run full suite to establish baselines**
- [ ] **Step 11: Commit**

### Task 1.4: WASM Benchmark Parity with Native

**Problem:** `bench_wasm.py` is missing `bench_json_roundtrip.py`, `bench_counter_words.py`, `bench_etl_orders.py`, and all new benchmarks from Task 1.3.

**Files:**
- Modify: `tools/bench_wasm.py` (BENCHMARKS list, lines 19-65)

- [ ] **Step 1: Add all missing benchmarks to WASM BENCHMARKS list**
- [ ] **Step 2: Run WASM suite and capture results**
- [ ] **Step 3: Commit**

### Task 1.5: Luau Benchmark Expansion

**Problem:** Only 1 hardcoded benchmark (procedural zone gen). No CLI flexibility.

**Files:**
- Modify: `tools/benchmark_luau_vs_cpython.py`

- [ ] **Step 1: Refactor to accept arbitrary benchmark files via CLI**

```python
parser.add_argument("benchmark", nargs="?", default=None,
    help="Path to benchmark .py file (default: built-in zone generator)")
```

- [ ] **Step 2: Add a standard benchmark suite list (reuse from bench.py)**
- [ ] **Step 3: Run at least 5 benchmarks through Luau transpiler**
- [ ] **Step 4: Commit**

### Task 1.6: Rust Transpiler Runtime Benchmarks

**Problem:** The Rust transpiler (`--target rust`) has ZERO runtime performance benchmarks. Only compilation speed is measured.

**Files:**
- Create: `tools/bench_rust_transpile.py`

- [ ] **Step 1: Create Rust transpiler benchmark harness**

Pattern: compile Python to Rust via `molt.cli build --target rust`, then `cargo build --release` the output, run the resulting binary, measure runtime.

- [ ] **Step 2: Run a small benchmark subset (sum, fib, matrix_math) through the Rust transpiler**
- [ ] **Step 3: Compare Molt-Rust runtime vs CPython vs Molt-native**
- [ ] **Step 4: Commit**

---

## Phase 2: Systematic Hot-Path Optimization (Jeff Dean Style)

### Task 2.1: Profile-Guided Hot-Path Identification

**Files:**
- Use: `MOLT_PROFILE_JSON=1` environment variable
- Create: `tools/profile_analyze.py` (aggregate profiler output)

- [ ] **Step 1: Run all benchmarks with profiling enabled**

```bash
for bench in tests/benchmarks/bench_*.py; do
  MOLT_PROFILE_JSON=1 UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli run --trusted "$bench" > /dev/null 2> "bench/results/profile_$(basename $bench .py).json"
done
```

- [ ] **Step 2: Create profile analysis tool**

Parse `molt_profile_json` output to produce a ranked hot-path report:
- Top 10 most-called intrinsics
- IC hit/miss ratios
- Attribute cache effectiveness
- Deopt rates
- Time distribution across phases

- [ ] **Step 3: Identify the top 5 optimization targets by expected impact**
- [ ] **Step 4: Commit**

### Task 2.2: Fix `bench_fib` Regression (Recursive Call Overhead)

**Problem:** `bench_fib` shows 0.05x CPython (20x slower!) despite being a pure recursive integer benchmark. The baseline shows 3.07x. Something in the call dispatch path has regressed catastrophically.

**Evidence needed:** Profile `bench_fib` with `MOLT_PROFILE_JSON=1` to identify:
- `call_bind_ic_hit` vs `call_bind_ic_miss` ratio
- Exception check density in hot path
- Whether recursive calls are going through the generic dispatch path

**Files:**
- Investigate: `runtime/molt-runtime/src/call/bind.rs` (call site IC)
- Investigate: `runtime/molt-backend/src/lib.rs` (call lowering)

- [ ] **Step 1: Profile fib benchmark**
- [ ] **Step 2: Identify root cause of regression**
- [ ] **Step 3: Fix regression**
- [ ] **Step 4: Verify speedup returns to >= 3x CPython**
- [ ] **Step 5: Commit**

### Task 2.3: Fix List Aggregate Regressions (sum_list, min_list, max_list)

**Problem:** All list aggregation benchmarks are 7-11x slower than CPython. These should be faster (iterating a list and summing/comparing is a primitive operation).

**Root cause hypothesis:** Missing fast-path for `sum(list)`, `min(list)`, `max(list)` builtins when the list contains homogeneous integers. Currently going through generic iterator protocol + dynamic dispatch per element.

**Files:**
- Investigate: `runtime/molt-runtime/src/builtins/` (sum/min/max implementations)
- Modify: Add specialized fast paths for `list[int]` cases

- [ ] **Step 1: Profile sum_list benchmark**
- [ ] **Step 2: Add typed fast-path for `sum()` over `list[int]`**

In the runtime `sum` implementation, detect when the iterable is a list of homogeneous ints and use a tight C-style loop instead of the generic iterator protocol:

```rust
// Pseudo-code for the fast path
if iterable.is_list() && iterable.all_ints() {
    let mut total: i64 = start_value;
    for item in iterable.as_int_slice() {
        total += item;  // No boxing, no dispatch
    }
    return MoltObject::from_int(total);
}
```

- [ ] **Step 3: Same pattern for `min()` and `max()`**
- [ ] **Step 4: Verify all three benchmarks >= 1x CPython**
- [ ] **Step 5: Commit**

### Task 2.4: Fix String Operation Regressions (str_split 0.64x, str_replace 0.73x)

**Problem:** `str_split` is 36% slower than CPython and `str_replace` is 27% slower. The Week 1 profiling showed `split_ws_ascii` dominates word_count — this is the same bottleneck.

**Root cause:** String split creates many small string objects with full allocation overhead. CPython uses a specialized small-string allocator.

**Optimization targets:**
1. SIMD-accelerated whitespace scanning (memchr-style)
2. Small string interning for common split results
3. Reduce per-substring allocation overhead (arena allocator for split results)

**Files:**
- Investigate: `runtime/molt-runtime/src/object/ops.rs` (split implementation)
- Investigate: Existing `split_ws_ascii` counter for hot-path data

- [ ] **Step 1: Profile str_split with counters**
- [ ] **Step 2: Implement SIMD whitespace scanner (portable + neon/sse4.2)**
- [ ] **Step 3: Add small-string arena for split results**
- [ ] **Step 4: Verify str_split >= 1.5x CPython**
- [ ] **Step 5: Commit**

### Task 2.5: Object Model Fast Paths (struct, attr_access, descriptor)

**Problem:** Baseline shows `bench_struct` at 2.45x but current run showed 0.04x (catastrophic regression). `attr_access` and `descriptor_property` also regressed badly.

**Root cause hypothesis:** Inline cache (IC) invalidation or attribute lookup path regression. The NaN-boxed object model should make attribute access fast if the IC is working.

**Files:**
- Investigate: `runtime/molt-runtime/src/builtins/attributes.rs`
- Investigate: `runtime/molt-runtime/src/object/` (attribute cache)

- [ ] **Step 1: Profile attr_access and struct benchmarks**
- [ ] **Step 2: Check IC hit/miss ratios — should be >95% hit rate**
- [ ] **Step 3: Fix any IC invalidation bugs or cache sizing issues**
- [ ] **Step 4: Verify struct >= 2x CPython, attr_access >= 1x CPython**
- [ ] **Step 5: Commit**

---

## Phase 3: Architecture-Level Optimization (Chris Lattner Style)

### Task 3.1: Cranelift Backend Tuning Matrix

**Problem:** Cranelift has many tunables that haven't been systematically swept.

**Files:**
- Modify: `runtime/molt-backend/src/lib.rs`
- Create: `tools/cranelift_sweep.py` (automated tunable sweep)

Tunable sweep matrix:
- `opt_level`: speed vs speed_and_size
- `regalloc_algorithm`: single_pass vs backtracking
- `log2_min_function_alignment`: 2, 3, 4, 5
- `enable_nan_canonicalization`: on/off
- `enable_jump_tables`: on/off
- `machine_code_cfg_info`: on/off
- `enable_alias_analysis`: on/off

- [ ] **Step 1: Create automated sweep script**
- [ ] **Step 2: Run sweep across all 51 benchmarks**
- [ ] **Step 3: Find Pareto-optimal configuration**
- [ ] **Step 4: Update default Cranelift flags**
- [ ] **Step 5: Commit**

### Task 3.2: NaN-Boxing Optimization Audit

**Problem:** The NaN-boxed 64-bit object representation (`TAG_INT`, `TAG_BOOL`, `TAG_NONE`, `TAG_PTR`, `TAG_PENDING`) determines the cost of every operation. Suboptimal tag checks or unnecessary boxing/unboxing in hot paths can cost 2-5x.

**Files:**
- Audit: `runtime/molt-runtime/src/object/` (MoltObject representation)

- [ ] **Step 1: Audit all tag check sequences in hot paths**

Look for patterns where:
- Objects are unboxed, operated on, then re-boxed (should stay unboxed)
- Tag checks happen inside loops (should be hoisted)
- Multiple tag checks for the same object (should be combined)

- [ ] **Step 2: Add `#[inline(always)]` to critical tag check functions**
- [ ] **Step 3: Profile before/after**
- [ ] **Step 4: Commit**

### Task 3.3: Memory Layout Optimization (Gabe Newell/Data-Oriented Design)

**Problem:** Python objects have poor cache locality by default. Dict iteration, list iteration, and attribute access patterns should be cache-friendly.

**Optimizations:**
1. **SoA (Struct of Arrays) for homogeneous lists**: When a list contains all ints or all floats, store values contiguously instead of as pointer arrays to boxed objects.
2. **Compact dict layout**: For dicts with string keys (most dicts), use a specialized layout with key hashes contiguous for fast lookup.
3. **Object pooling**: Reuse recently-freed objects of the same size class to reduce allocator pressure.

**Files:**
- Modify: `runtime/molt-runtime/src/object/` (list/dict specialization)

- [ ] **Step 1: Implement typed list backing store (int[], float[], generic[])**
- [ ] **Step 2: Implement compact string-key dict layout**
- [ ] **Step 3: Profile dict_ops and list_ops benchmarks before/after**
- [ ] **Step 4: Commit**

### Task 3.4: Compilation Pipeline PGO (Profile-Guided Optimization)

**Problem:** The Cranelift backend doesn't use PGO data to guide code layout, branch prediction hints, or function ordering.

**Files:**
- Modify: `runtime/molt-backend/src/lib.rs`
- Use: existing `PgoProfileIR` support

- [ ] **Step 1: Verify PGO data collection works end-to-end**

```bash
# Generate PGO profile
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli run --pgo-profile /tmp/fib.pgo tests/benchmarks/bench_fib.py
# Rebuild with PGO
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli build --pgo-profile /tmp/fib.pgo tests/benchmarks/bench_fib.py
```

- [ ] **Step 2: Implement PGO-guided function ordering (hot functions first)**
- [ ] **Step 3: Implement PGO-guided branch weight hints to Cranelift**
- [ ] **Step 4: Measure impact on fib, sum, matrix_math**
- [ ] **Step 5: Commit**

### Task 3.5: WASM-Specific Optimizations

**Problem:** WASM/native ratio is 4.81x median — target is < 2.5x.

**Optimizations:**
1. **Multi-value returns**: Cranelift WASM backend supports multi-value returns to avoid stack spilling for tuples
2. **Bulk memory operations**: Use `memory.copy`/`memory.fill` for string/bytes operations
3. **Exception handling**: Use native WASM exception handling (`try_table`/`throw`) instead of setjmp/longjmp emulation
4. **Code size**: Strip debug info, use `wasm-opt -Oz` post-processing
5. **Table-based dispatch**: Replace indirect function calls with table dispatch where possible

**Files:**
- Modify: `runtime/molt-backend/src/wasm.rs`

- [ ] **Step 1: Profile WASM execution in Node.js to find top bottlenecks**
- [ ] **Step 2: Implement bulk memory for string/bytes operations**
- [ ] **Step 3: Verify native exception handling is active (not setjmp)**
- [ ] **Step 4: Add wasm-opt post-processing to build pipeline**
- [ ] **Step 5: Measure wasm/native ratio improvement**
- [ ] **Step 6: Commit**

---

## Phase 4: Observability and Regression Prevention

### Task 4.1: Automated Benchmark CI Gate

**Problem:** Regressions can slip in undetected because benchmarks aren't run in CI.

**Files:**
- Create: `.github/workflows/bench.yml` or equivalent
- Modify: `tools/bench_diff.py` (add CI-friendly exit codes)

- [ ] **Step 1: Create CI benchmark job that runs smoke suite on every PR**
- [ ] **Step 2: Add `--fail-regression-pct 10` flag to fail on >10% regression**
- [ ] **Step 3: Store benchmark artifacts for trend analysis**
- [ ] **Step 4: Commit**

### Task 4.2: Live Performance Dashboard

**Files:**
- Create: `tools/bench_dashboard.py` (generates HTML report from JSON artifacts)

- [ ] **Step 1: Create dashboard that reads all `bench/results/*.json` files**
- [ ] **Step 2: Generate time-series charts showing performance trends**
- [ ] **Step 3: Highlight regressions in red, improvements in green**
- [ ] **Step 4: Commit**

### Task 4.3: Compilation Throughput Regression Gate

**Files:**
- Modify: `tools/compile_progress.py`

- [ ] **Step 1: Add regression threshold to compile_progress**
- [ ] **Step 2: Fail if any build lane regresses by >20%**
- [ ] **Step 3: Commit**

---

## Priority Order and Dependencies

```
Phase 0 (BLOCKING — do first):
  Task 0.1 → Task 0.2 → Task 0.3

Phase 1 (parallel after Phase 0):
  Task 1.1 (WASM fix) — independent
  Task 1.2 (native expansion) — independent
  Task 1.3 (new benchmarks) — independent
  Task 1.4 (WASM parity) — after 1.1
  Task 1.5 (Luau expansion) — independent
  Task 1.6 (Rust transpiler) — independent

Phase 2 (after Phase 1 baselines established):
  Task 2.1 (profiling) — first, informs all others
  Task 2.2 (fib regression) — after 2.1
  Task 2.3 (list aggregates) — after 2.1
  Task 2.4 (string ops) — after 2.1
  Task 2.5 (object model) — after 2.1

Phase 3 (after Phase 2 low-hanging fruit):
  Task 3.1 (Cranelift tuning) — independent
  Task 3.2 (NaN-boxing audit) — independent
  Task 3.3 (memory layout) — after 2.3, 2.5
  Task 3.4 (PGO) — independent
  Task 3.5 (WASM optimization) — after 1.1

Phase 4 (continuous, start after Phase 1):
  Task 4.1 (CI gate) — after 1.2
  Task 4.2 (dashboard) — after 1.2
  Task 4.3 (compile gate) — independent
```

## Expected Impact by Phase

| Phase | Metric | Current | Target | Method |
|-------|--------|---------|--------|--------|
| 0 | Benchmarks runnable | ~30% | 100% | Fix infra bugs |
| 1 | Benchmark coverage | 16 native, 0 WASM | 58+ native, 50+ WASM | Add missing |
| 2 | Median native speedup | ~2.6x | ~5x | Hot-path optimization |
| 2 | Worst-case native | 0.05x (fib) | >= 1.0x | Fix regressions |
| 3 | Median native speedup | ~5x | ~8x+ | Architecture wins |
| 3 | WASM/native ratio | 4.81x | < 2.5x | WASM-specific opts |
| 4 | Regression escape rate | Unknown | 0 | CI gates |
