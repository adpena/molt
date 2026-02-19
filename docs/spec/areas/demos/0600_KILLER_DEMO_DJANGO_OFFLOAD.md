# Killer Demo Spec: Django Endpoint Offload to Molt Worker (Go-class Concurrency + Arrow/MsgPack)
**Spec ID:** 0600
**Status:** Draft (demo-as-product)
**Priority:** P0 (first public proof)
**Audience:** demo implementers, runtime engineers
**Goal:** Show a real Django app gaining Go-like throughput and stable tail latency by offloading one endpoint to a Molt-compiled worker via IPC, then define a clear path to an in-process fast lane for lower-overhead dispatch.

---

## 0. What this demo proves (the “why anyone cares”)
This demo must prove, in a single sitting, that Molt delivers:
1) **Production usefulness without a rewrite** (Django stays in CPython)
2) **Native concurrency** (tasks/channels) with stable P99/P999 latency
3) **Simple deploy story** (a single `molt_worker` binary + a small Python package)
4) **Data-heavy path** support (Arrow/MsgPack payloads; optional Polars/DuckDB ops)

The demo is a wedge into both:
- web teams (Django)
- data teams (pandas-like transforms and tabular payloads)

---

## 1. Repo layout (suggested)
```
demo/
  django_app/
    manage.py
    demoapp/
      views.py
      urls.py
      settings.py
  molt_worker_app/
    pyproject.toml
    app/
      entrypoints.py   # exported Molt entrypoints
  molt_accel/
    python/
      molt_accel/      # client library
    protocol/
      schema.md        # IPC protocol notes (links to 0302)
bench/
  k6/
  vegeta/
  hey/
  datasets/
docs/
  demo/
    README.md
```

---

## 2. Demo story (user narrative)
1) Start Django normally.
2) Hit `/baseline/` endpoint (CPython-only) under load.
3) Hit `/offload/` endpoint:
   - Django authenticates, parses headers, routes request (unchanged)
   - Decorator sends payload to `molt_worker` through `molt_accel`
   - Worker computes response using Molt concurrency and fast serialization
4) Show results:
   - throughput (req/s)
   - CPU usage
   - P99/P999 latency
   - deploy simplicity

---

## 3. Endpoints
### 3.1 Baseline endpoint (CPython only)
- Performs:
  - JSON decode
  - light validation
  - small transformation (simulate business logic)
  - JSON encode

### 3.2 Offload endpoint (Molt)
- Performs:
  - MsgPack decode (or JSON)
  - heavier compute + optional dataframe ops
  - MsgPack encode (or JSON)

**Optional “data path”:** accept an Arrow IPC payload representing a small table (e.g., 10k rows) and run:
- filter + groupby aggregate + join or sort (Polars/DuckDB backend)
Return summary + optionally a reduced Arrow table.

---

## 4. Success criteria (must be measurable)
### 4.1 Performance
- Offload endpoint achieves **≥ 5×** throughput vs baseline on the same machine, same concurrency level (P0 target).
- Tail latency:
  - P99 stable (no GC-like pauses, no global lock artifacts)
  - P999 within a predictable band

> Note: 10× is an aspirational headline; 5× + better P99 is already an “oh wow” moment for Django users.

### 4.2 Operational simplicity
- `molt_worker` is a single executable.
- `pip install molt_accel` (or uv) installs the client.
- Django config change is minimal (decorator or middleware).

### 4.3 Reliability
- Cancellation works (client abort cancels worker task).
- Timeouts work (worker returns Timeout cleanly).
- Worker never crashes on bad input (returns structured error).

---

## 5. Implementation approach (phased)
### Phase A (Week 1)
- Implement `molt_worker` binary with one exported entrypoint:
  - `compute(payload) -> payload`
- Implement `molt_accel` client:
  - spawn/attach to worker
  - submit jobs
  - receive results
- Minimal Django integration decorator:
  - `@molt_offload(entry="compute")`

### Phase B (Week 2)
- Add concurrency in worker:
  - internal task pool
  - channels for work queues
- Add MsgPack codec (default) and JSON fallback.
- Add metrics: time_ms, allocs, queue depth.

### Phase C (Week 3)
- Add optional Arrow path:
  - accept Arrow IPC bytes
  - run Polars/DuckDB transform
  - return Arrow or summary

### Phase D (Week 4)
- Harden:
  - fuzz input decoding
  - soak tests
  - chaos (kill worker, restart)
  - CI regression benchmarks

### Phase E (planned): In-process fast path (CinderX-like integration lane)
- Keep the worker path as the default migration lane.
- Add an opt-in in-process execution mode for selected endpoints that are:
  - precompiled at deploy/build time,
  - loaded at Django worker startup,
  - invoked through a stable ABI boundary (no runtime compilation on requests).
- Route selection:
  - `worker` mode: current stdio IPC model (best isolation).
  - `in_process` mode: direct call into precompiled handler (lowest latency overhead).
- Safety invariants for in-process mode:
  - preserve capability checks, timeout/cancellation semantics, and deterministic error mapping.
  - fail closed to worker mode or explicit 5xx when ABI/capability checks fail (no silent semantic drift).

---

## 6. Demo script (the “live run”)
1) Start worker:
   - `./molt_worker --socket /tmp/molt.sock --exports molt_exports.json`
2) Start Django:
   - `python manage.py runserver`
3) Run load:
   - `k6 run bench/k6/baseline.js`
   - `k6 run bench/k6/offload.js`
4) Show graphs and printed summary.

---

## 7. Talking points (what you say on stage)
- Django stays Django.
- Molt is not replacing CPython; it’s giving you a **native deployment target** for performance-critical work.
- This is the migration story:
  - offload one endpoint
  - offload worker jobs
  - compile more services over time
- Arrow/MsgPack makes it fast and structured.
- Concurrency is the proof Molt is real.

---

## 8. “One slide” truth
**Molt makes Python services deploy and scale like Go, without a rewrite.**

---

## 9. Architecture rationale (CinderX-style JIT/static vs Molt lanes)
Why CinderX exists:
- Large CPython fleets need incremental speedups without changing deployment or extension stories.
- JIT improves hot long-lived code paths after warmup.
- Static Python reduces dynamic overhead where type information is stable.

Why Molt keeps an explicit worker lane:
- process isolation and fault containment,
- strict capability and cancellation boundaries,
- framework-agnostic offload contract (Django/Flask/FastAPI).

How Molt can compete:
- keep worker IPC as the broad adoption path,
- add an opt-in in-process lane for latency-sensitive endpoints,
- retain AOT determinism and no-runtime-compile deployment semantics while reducing per-request overhead.
