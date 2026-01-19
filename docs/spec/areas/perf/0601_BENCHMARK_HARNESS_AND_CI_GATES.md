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
