# Molt Metrics Slide — Canonical Performance & Value Metrics
**Document ID:** 0960
**Audience:** Investors, CTOs, Molt contributors
**Purpose:** Define the single source of truth for how Molt performance, value, and progress are measured — including the **exact graph layout** expected in decks and docs.

---

## The One Slide (Investor / CTO View)

> **Molt eliminates the Python rewrite tax by delivering predictable performance, simpler operations, and smaller artifacts — without changing teams or languages.**

---

## Core Metrics (all are required)

| Category | Metric | Python Baseline | Molt Target | Why It Matters |
|--------|------|-----------------|-------------|----------------|
| Latency | P99 request latency | Django / FastAPI | **2–10× lower** | Tail latency defines user experience |
| Throughput | Requests/sec per core | CPython runtime | **3–10× higher** | Infra efficiency |
| Concurrency | In-flight tasks | GIL-limited | **10–100× higher** | Real async scalability |
| CPU Efficiency | CPU per request | Python baseline | **30–70% less** | Cost and sustainability |
| Memory | RSS per worker | Python + Celery | **50–80% lower** | Density and stability |
| Artifacts | Deployable size | Containers | **5–50 MB static binary** | Cold start + shipping |
| Ops | Moving parts | Web + broker | **Single process** | Reliability |
| Rewrite Cost | Lines rewritten | Go/Rust rewrite | **~0** | Engineering years saved |

---

## Exact Graph Layout (MANDATORY)

This section defines the **only acceptable visual layout** for investor decks, blog posts, and benchmark reports.

### Graph 1: Tail Latency (Primary Graph)

**Type:** Log-scaled bar chart
**Title:** `P99 Latency — DB-Heavy Endpoint`
**X-axis:** Runtime
- Django (sync)
- FastAPI (async)
- Python + Celery (jobs, if applicable)
- **Molt (worker offload)**

**Y-axis:** Latency (milliseconds, log scale)

**Rules:**
- Always show **P99** (never P50 alone)
- Bars must be ordered from slowest → fastest
- Molt bar must be visually distinct (color or pattern)
- Annotate absolute values (e.g., `320ms`, `42ms`)

**Why:**
CTOs and investors care about worst-case behavior, not averages.

---

### Graph 2: Throughput per Core

**Type:** Bar chart
**Title:** `Throughput per Core (req/s)`
**X-axis:** Runtime (same order as Graph 1)
**Y-axis:** Requests per second per core

**Rules:**
- Same hardware
- Same workload
- Explicitly state cores used
- No stacked bars

---

### Graph 3: Memory Footprint Under Load

**Type:** Bar chart
**Title:** `RSS Memory per Worker Under Sustained Load`
**X-axis:** Runtime
**Y-axis:** MB RSS

**Rules:**
- Measure after warm-up
- Sustained load ≥ 5 minutes
- Molt binary size may be annotated separately

---

### Graph 4: Operational Complexity (Qualitative)

**Type:** Table or simple diagram

| Stack | Components |
|-----|------------|
| Django + Celery | Web, Broker, Workers, Scheduler |
| Molt | Single binary |

**Rules:**
- This is intentionally simple
- No marketing fluff
- Visual clarity over precision

---

### Optional Graph 5 (Later Stage): Cost per 1M Requests

**Type:** Bar chart
**Y-axis:** $ cost
**X-axis:** Runtime

Only include when numbers are production-backed.

---

## Required Benchmark Scenarios

All official benchmarks and demos must map to **at least one** scenario below.

### 1. DB-heavy list endpoint
- ORM-backed SELECT with pagination
- JSON serialization
- Auth + middleware

### 2. Background job fan-out
- Job ingestion
- Parallel execution
- Cancellation and retries

### 3. Data transformation pipeline
- Ingest → transform → emit
- Vectorized execution (Arrow/Polars)
- Async IO

---

## Measurement Methodology (Contract)

- Same logic, schema, DB, and hardware
- Warm and cold runs reported separately
- Always report P50 / P95 / **P99**
- Memory = RSS under sustained load
- CPU = CPU-seconds per request/job

Benchmarks violating this contract are invalid.

---

## Secondary Diagnostics (Non-Primary)
These metrics do not replace the core slide metrics, but they are required for
debugging demo behavior and DB offload performance:

- Worker: `queue_us`, `handler_us`, `exec_us`, `decode_us`, `queue_depth`,
  `pool_in_flight`, `pool_idle`, `payload_bytes`.
- DB (`db_query`): `db_alias` (string), `db_tag` (string), `db_row_count`,
  `db_bytes_in`, `db_bytes_out`, `db_result_format`.

## Why This Wins

- Investors see discipline, not hype
- CTOs see problems they recognize
- Engineers see clear targets

> **Win these graphs, and the ecosystem follows.**
