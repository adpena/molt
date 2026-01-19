# Repro Bench Vertical Slice: Molt Worker + Django Offload + Bench Harness
**Spec ID:** 0910
**Status:** Draft (implementation-targeting)
**Priority:** P0 (this is “make it real”)
**Audience:** core contributors, demo implementers
**Goal:** Define the smallest buildable vertical slice that proves Molt’s web value proposition with reproducible benchmarks.

---

## 0. Deliverables (what must exist)
1) **`molt_worker` (native binary)**
   - Executes exported entrypoints using a simple IPC framing (stdio first)
   - Enforces timeouts/cancellation
   - Returns structured errors
   - Emits minimal metrics

2) **`molt_accel` (Python client library)**
   - Starts or attaches to a worker process (fallbacks to `molt-worker` in PATH with packaged exports)
   - Provides `@molt_offload(...)` decorator for Django (and a lower-level client API)
   - Handles worker restarts and transient failures cleanly
   - Exposes before/after hooks, metrics callbacks, and cancellation checks for request plumbing

3) **Demo Django app**
   - Two endpoints: `/baseline` (CPython-only) and `/offload` (calls worker)
   - Same logical payload/response
   - Optional: a “fake DB” simulation mode for stable benchmarking

4) **Benchmark harness**
   - k6 scripts for baseline/offload (+ optional data path)
   - A runner script (`bench/scripts/run_demo_bench.py`) to execute both, store results, and print a summary
   - Optional CI gates (nightly) for regressions

---

## 1. Why this vertical slice is the next step
- It forces Molt’s **IPC contract**, **cancellation semantics**, and **observability** to become real.
- It creates a reproducible benchmark artifact that the community can run and trust.
- It establishes the adoption wedge: “**one decorator** → performance win.”

---

## 2. Minimal repo layout (recommended)
```text
molt_worker/                 # Rust binary
molt_accel/                  # Python library
demo/django_app/             # Django app showing baseline vs offload
bench/k6/                    # load tests
bench/scripts/               # run + summarize
docs/spec/areas/demos/0910_*.md  # these specs
```

---

## 3. Non-negotiable behaviors (the “first stable contract”)
### 3.1 IPC semantics
- request is framed (length-prefixed) and contains:
  - request_id
  - entrypoint name
  - codec
  - payload bytes
  - deadline/timeout
- response returns:
  - request_id
  - status code enum
  - payload bytes (result) or error bytes (message)
  - optional metrics

### 3.2 Cancellation semantics
- if Django client disconnects, `molt_accel` cancels the in-flight request
- worker must:
  - stop executing promptly
  - return `Cancelled` status
  - release any resources (DB conns, buffers)
- timeouts are enforced by worker regardless of client behavior

### 3.3 Backpressure
- worker has a bounded queue
- when saturated, worker returns `Busy` quickly (or blocks acquire up to a small max wait)

---

## 4. Demo endpoint definitions
### 4.1 `/baseline`
- uses normal Python code path
- simulates “typical list endpoint work”:
  - parse request
  - validation
  - (optional) fake DB latency + decoding cost
  - encode JSON

### 4.2 `/offload`
- identical semantics
- offloads “the handler core” to Molt worker:
  - payload encoded as MsgPack (default)
  - worker returns MsgPack result
  - Django converts to JSON response (or passes through)

---

## 5. Benchmark requirements
- must measure:
  - req/s, p50, p95, p99, p999
  - error rate
  - CPU and RSS (at least coarse)
  - worker queue depth and time-in-queue
- must store results to a dated JSON artifact
- must provide a markdown summary output for easy sharing

**Implementation note:** the Django demo uses `molt_accel` metrics hooks to emit per-request
metrics (queue_us/queue_ms, handler_us, exec_us/exec_ms, decode_us, queue_depth) into a JSONL
file configured by `MOLT_DEMO_METRICS_PATH`; the bench runner aggregates these into the
JSON+markdown outputs.

---

## 6. Acceptance criteria (P0 “ship it”)
- On a developer laptop, running k6 with moderate concurrency:
  - offload shows a clear improvement in throughput and/or tail latency
  - cancellation works (abort requests; worker stops)
  - worker never crashes on bad inputs
- One command runs both benchmarks and prints a summary

---

## 7. Explicit non-goals (keep scope tight)
- Postgres integration (can come immediately after fake DB proves the plumbing)
- HTTP server rewrite (Django remains the gateway)
- full tracing/OTel (basic metrics first)
