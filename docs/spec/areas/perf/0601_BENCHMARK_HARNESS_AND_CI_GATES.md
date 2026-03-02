# Benchmark Harness + CI Gates for the Killer Demo
**Spec ID:** 0601
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** demo implementers, CI authors
**Goal:** Provide repeatable benchmarking and hard regression gates (performance is correctness).

---

## 0. Principles
- Benchmarks must be repeatable, pinned, and comparable.
- We measure **throughput, tail latency, CPU, memory**, and **worker queue behavior**.
- CI should not be flaky: use calibrated workloads and stable thresholds.

---

## 1. Tools
- Load testing:
  - `k6` (preferred for scripted scenarios)
  - `vegeta` (simple HTTP attack tool)
  - `hey` (quick smoke and local iteration)
- Profiling:
  - Linux: `perf`, `flamegraph`
  - macOS: Instruments (Time Profiler)
- Metrics:
  - worker exports JSON counters
  - optional Prometheus endpoint

---

## 2. Scenarios
### 2.1 Baseline
- Endpoint: `/baseline/`
- Payload: 1–4 KB JSON
- Steps: decode → validate → transform → encode
- Purpose: CPython reference

### 2.2 Offload
- Endpoint: `/offload/`
- Payload: same logical request but encoded via MsgPack
- Steps:
  - Django routes + auth (same)
  - offload compute to worker
  - worker returns response

### 2.3 Data path (optional)
- Endpoint: `/offload_table/`
- Payload: Arrow IPC bytes for 10k–100k rows, narrow schema
- Steps: filter + groupby sum + sort
- Purpose: prove “pandas usefulness” story

---

## 3. Metrics to collect (required)
For each run:
- req/s
- median, p95, p99, p999 latency
- error rate
- server CPU %
- worker CPU %
- RSS memory
- worker queue depth distribution
- worker deopt/guard stats (if enabled)
- payload sizes

Store results as JSON files under `bench/results/` with a timestamp.

---

## 4. Benchmark scripts (skeleton)
Directory:
```
bench/
  k6/
    baseline.js
    offload.js
    offload_table.js
  scripts/
    run_local.sh
    summarize.py
```

### 4.1 Suggested k6 checks
- status == 200
- latency thresholds (soft checks)
- error rate < 0.1%

---

## 5. CI gates (non-flaky design)
### 5.1 Where to run perf CI
- Nightly on dedicated runners is ideal.
- If not possible, run a small smoke perf test on PRs and a heavier test on schedule.
    - For compiler microbenchmarks, `tools/bench.py --smoke` provides a lightweight CI gate with JSON output.

### 5.2 Gates (initial conservative)
- Offload throughput must not regress by > 10% vs last main baseline
- P99 must not regress by > 15%
- Worker error rate must remain < 0.1%

These gates should tighten as the system stabilizes.

### 5.3 Artifact retention
CI must upload:
- k6 outputs
- summarized JSON
- worker logs
- any failing payload cases

---

## 6. Reproducibility
- Pin tool versions (k6, vegeta)
- Pin Python deps (uv lock)
- Record:
  - commit hash
  - machine info
  - kernel version
  - CPU model

---

## 7. Acceptance criteria
- One command can run baseline and offload benchmarks locally.
- Results are stored and summarized automatically.
- CI posts a markdown summary and fails on regressions.

---

## 8. PGO (Profile-Guided Optimization) Pipeline

### Overview

PGO can improve the Molt runtime (`molt-runtime`) performance by 5-15% on
branch-heavy code paths (function dispatch, exception handling, type checking).
The pipeline has three stages: instrument, profile, optimize.

### Workflow

```bash
# Stage 1: Instrument — build runtime with profiling instrumentation
RUSTFLAGS="-Cprofile-generate=/tmp/molt-pgo-data" \
  cargo build -p molt-runtime --profile release

# Stage 2: Profile — run representative workloads to collect profile data
# Use the benchmark suite as the training set:
PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --json-out /tmp/pgo-bench.json
# Also run differential test suite for broader coverage:
MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_TIMEOUT=60 \
  uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic

# Stage 3: Merge profiles and rebuild
llvm-profdata merge -o /tmp/molt-pgo-data/merged.profdata /tmp/molt-pgo-data/
RUSTFLAGS="-Cprofile-use=/tmp/molt-pgo-data/merged.profdata" \
  cargo build -p molt-runtime --profile release
```

### Integration with CI

PGO builds are NOT part of the standard CI pipeline due to the two-pass build
cost. They are intended for:

- **Release builds**: The release workflow can optionally enable PGO via
  `MOLT_PGO=1` environment variable.
- **Nightly performance tracking**: A separate nightly job can build PGO-optimized
  binaries and compare against non-PGO baselines.

### Expected Gains

| Category | Estimated Improvement |
|----------|---------------------|
| Function dispatch (call_bind, IC) | 10-15% |
| Type checking hot paths | 5-10% |
| String/bytes kernels | 2-5% (already SIMD-optimized) |
| Collection operations | 5-10% |

### Caveats

- PGO profiles are architecture-specific — profiles from x86-64 cannot be used
  for aarch64 builds.
- Profile training set must be representative; skewed profiles can pessimize
  cold paths.
- Rust PGO requires LLVM (not Cranelift) — this applies to the runtime
  compilation, not to Molt-compiled user code.

---

## 9. BOLT (Binary Optimization and Layout Tool)

### Status: Investigation Only

BOLT is a post-link optimizer from the LLVM project that reorders functions and
basic blocks based on runtime profile data. It operates on the final binary,
independent of the compiler.

### Relevance to Molt

BOLT could benefit the Molt runtime binary by:

- **Code layout optimization**: Reordering hot functions to minimize iTLB and
  icache misses.
- **Hot/cold splitting**: Moving rarely-executed error paths and diagnostics out
  of hot code regions.
- **Function reordering**: Placing frequently co-called functions adjacent in
  memory.

### Estimated Benefit

5-10% improvement on branch-heavy workloads (function dispatch, exception
handling). Minimal benefit for SIMD-heavy kernels (already cache-friendly).

### Prerequisites

- Linux x86-64 only (BOLT does not support macOS or aarch64 yet).
- Requires `perf` profile data (LBR-based for best results).
- Binary must not be fully stripped (needs symbol table for rewriting).

### Integration Plan

Not planned for near-term. BOLT integration would be a post-release optimization:

1. Build `molt-runtime` with `-Clink-arg=-Wl,--emit-relocs` to preserve relocations.
2. Run representative workload under `perf record -e cycles:u -j any,u`.
3. Convert perf data: `perf2bolt -p perf.data -o bolt.fdata molt-runtime`.
4. Optimize: `llvm-bolt molt-runtime -o molt-runtime.bolt -data bolt.fdata -reorder-blocks=ext-tsp -reorder-functions=hfsort`.
5. Benchmark A/B comparison.

### Decision Criteria

Adopt BOLT if:
- Measured improvement > 5% on the benchmark suite.
- Build pipeline complexity is manageable (single additional step).
- Linux-only limitation is acceptable (macOS/Windows would not benefit).
