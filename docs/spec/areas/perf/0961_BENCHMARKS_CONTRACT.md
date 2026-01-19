# Molt Benchmark Contract
**Document ID:** 0961
**Status:** Canonical
**Audience:** Molt contributors performance engineers
**Purpose:** Define how benchmarks are written, executed, validated, and interpreted for Molt.

---

## 1. Why this document exists
Benchmarks are the *currency of credibility* for Molt.

This document ensures:
- benchmarks are reproducible
- results are comparable over time
- claims map directly to the Metrics Slide (0960)

Any benchmark not conforming to this contract is **non-canonical**.

---

## 2. Required benchmark scenarios
Every benchmark must declare which scenario(s) it implements.

### Scenario A — DB-heavy list endpoint
- paginated SELECT
- JSON serialization
- auth + middleware
- realistic row counts (1k–100k)

### Scenario B — Background job fan-out
- enqueue N jobs
- bounded parallelism
- cancellation + retry
- failure injection

### Scenario C — Data transformation pipeline
- ingest → transform → emit
- vectorized operations
- async IO

Benchmarks may cover multiple scenarios.

---

## 3. Required outputs
Every benchmark run **must emit**:

| Metric | Required |
|------|----------|
| Latency | P50, P95, **P99** |
| Throughput | ops/sec per core |
| Memory | RSS (MB) |
| CPU | CPU-seconds |
| Metadata | hardware, cores, runtime version |

Preferred formats:
- JSON (primary)
- CSV (secondary)

Human-only output is insufficient.

---

## 4. Baseline rules
Benchmarks must compare against at least one baseline:
- Django (sync)
- FastAPI (async)
- Python + Celery (jobs)

Rules:
- identical logic
- identical schema
- identical DB
- identical hardware

If a baseline is omitted, the benchmark must explain why.

---

## 5. Warm-up and duration
- warm-up phase required (discarded)
- measurement window ≥ 5 minutes
- sustained load (not burst-only)

Cold-start benchmarks must be **explicitly labeled**.

---

## 6. Validation checklist
Before results are accepted:
- [ ] scenario declared
- [ ] metrics complete
- [ ] baselines included
- [ ] hardware disclosed
- [ ] ties to 0960 metrics

Missing any checkbox invalidates the benchmark.

---

## 7. What this document forbids
- microbenchmarks without system context
- average-only latency
- unbounded concurrency
- synthetic logic unlike real services

---

## 8. Guiding principle
> **Benchmarks exist to guide decisions, not win arguments.**
