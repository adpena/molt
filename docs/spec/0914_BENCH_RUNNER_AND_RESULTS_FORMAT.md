# Bench Harness: Runner Script + Results Format
**Spec ID:** 0914
**Status:** Draft
**Priority:** P0
**Audience:** benchmarking contributors, CI authors
**Goal:** Ensure benchmarks are reproducible and easy to share.

---

## 0. Bench tools
Preferred:
- `k6` scripted scenarios
Optional:
- `vegeta`, `hey` for quick checks

---

## 1. Bench scripts
- `bench/k6/baseline.js`
- `bench/k6/offload.js`

Each must:
- set concurrency stages
- validate status codes
- record latency distribution

---

## 2. Runner script behavior
A single script (bash or python) should:
1) start worker (if needed)
2) start Django
3) run baseline k6
4) run offload k6
5) store raw outputs and summarized JSON
6) print a markdown summary

---

## 3. Results format (JSON)
Store:
- git commit hash
- timestamp
- machine info (best-effort; platform/system/CPU count)
- tool versions (python, k6)
- for each scenario:
  - req/s
  - p50/p95/p99/p999
  - error rate
  - payload bytes per request (sent/received)
- worker metrics summary (queue depth, queue/exec time, payload bytes when available)
- process metrics (server/worker CPU avg/max, RSS avg/max KB, proc count, samples)
- process context (server mode and worker/server PIDs used for sampling)
- fake DB config when set (delay_ms, decode_us_per_row, cpu_iters)

---

## 4. CI gates (later)
- nightly perf run on stable runner
- fail if:
  - throughput regresses > X%
  - p99 regresses > Y%
