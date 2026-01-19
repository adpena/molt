# Molt — Funding Narrative
**Audience:** Seed / Series A investors, technical partners
**Positioning:** Open-source infrastructure with platform-level upside

---

## Executive summary
Molt is building the **next-generation Python execution platform** for production systems.

Python dominates backend development, data pipelines, and internal tooling—but it hits a wall at scale. Teams eventually rewrite critical services in Go or Rust, fragmenting systems and burning years of engineering time.

**Molt eliminates the rewrite cliff.**

It allows teams to:
- keep Python ergonomics
- gain Go-class concurrency and predictable latency
- ship static binaries and WASM modules
- scale services and workers without re-architecting

Molt is not a CPython replacement.
It is a **production runtime and compiler** designed for long-lived services, background jobs, and data-heavy endpoints.

---

## The problem (experienced, not theoretical)
Every serious Python shop eventually faces the same issues:
- unpredictable tail latency
- GIL-limited concurrency
- heavy object allocation and serialization costs
- brittle async + threadpool patterns
- complex infrastructure (Celery, brokers, sidecars)

The result is a familiar pattern:
> “Prototype in Python → rewrite in Go/Rust later.”

This rewrite tax costs:
- years of engineering time
- lost domain knowledge
- operational regressions
- slower product velocity

The industry has accepted this as normal. It shouldn’t be.

---

## Why existing solutions fail
- **PyPy / Numba / Cython** optimize narrow cases (numeric loops, functions), not services.
- **FastAPI** improves ergonomics but still runs on the same runtime model.
- **Async frameworks** add complexity without fixing cancellation, backpressure, or predictability.
- **C extensions** fragment the ecosystem and kill portability.

No solution addresses the *runtime contract* itself.

---

## Molt’s insight
The performance gap is not about Python syntax.
It’s about **semantics and execution guarantees**.

Molt introduces:
- explicit contracts instead of unbounded dynamism
- structured concurrency (tasks + channels)
- cancellation and backpressure as first-class concepts
- compiled boundaries for schemas, IPC, and DB decoding
- tiered semantics that allow aggressive optimization when constraints are met

This enables:
- ahead-of-time compilation
- predictable performance
- stable tail latency
- small deployable artifacts

---

## Go-to-market wedge
Molt starts where the pain is highest and adoption is easiest:

1. **Django + FastAPI services**
   - offload hot endpoints to a Molt worker
   - no rewrite, no framework switch
   - immediate throughput and latency gains

2. **Background jobs**
   - replace Celery-style stacks
   - simpler ops, correct cancellation
   - fewer moving parts

3. **Data-heavy APIs**
   - Arrow/Polars/DuckDB-backed execution
   - modern dataframe semantics
   - async, typed, vectorized pipelines

Each wedge compounds into the next.

---

## Why open source wins here
This category demands:
- trust
- ecosystem integration
- long-term stability

Molt follows a proven model:
- open core runtime and compiler
- permissive licensing for adoption
- paid offerings around:
  - enterprise support
  - hosted build/CI artifacts
  - managed workers
  - observability and compliance tooling

The moat is **semantic contracts + ecosystem gravity**, not closed code.

---

## Market size
- Python is used by tens of millions of developers
- Backend + data infrastructure is a multi-hundred-billion-dollar market
- Even modest penetration in “rewrite-avoidance” budgets is enormous

Molt competes not with Python tools, but with:
- Go rewrites
- Rust rewrites
- microservice sprawl
- platform engineering headcount

---

## Long-term vision
Molt becomes:
- the default runtime for Python services
- the execution layer beneath modern Python frameworks
- a bridge between server, worker, and WASM execution
- a unifying platform for data and services

The end state:
> **Python without the rewrite tax.**

---

## Why now
- Rust ecosystem maturity
- WASM viability
- schema-first API design mainstream adoption
- Python community ready for pragmatic performance, not purity

This window didn’t exist five years ago.

---

## Closing
Molt is building what Python teams have wanted for a decade but couldn’t articulate:
- keep Python
- lose the pain
- scale with confidence

That is a platform opportunity.
