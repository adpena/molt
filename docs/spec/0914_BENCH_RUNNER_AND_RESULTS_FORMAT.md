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
- machine info (best-effort)
- tool versions
- for each scenario:
  - req/s
  - p50/p95/p99/p999
  - error rate
  - payload size
- worker metrics summary (queue depth, avg queue time)

---

## 4. CI gates (later)
- nightly perf run on stable runner
- fail if:
  - throughput regresses > X%
  - p99 regresses > Y%
