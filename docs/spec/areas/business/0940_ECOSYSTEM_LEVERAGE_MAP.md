# Molt Ecosystem Leverage Map: Use, Emulate, or Replace
**Spec ID:** 0940
**Status:** Strategic Design Document
**Audience:** Molt core maintainers, contributors, investors
**Goal:** Identify the most valuable existing libraries and ecosystems Molt should **use directly**, **emulate conceptually**, or **replace with Molt-native implementations**, and explain why.

---

## 0. Why this document exists
Great platforms are not built by rewriting everything.
They are built by:
- standing on strong foundations
- being opinionated about where compatibility ends
- investing only where leverage compounds

This document defines Molt’s stance toward key libraries and ecosystems.

---

## 1. Classification legend
Each technology is classified as one of:

- **USE** — integrate directly; do not compete
- **EMULATE** — copy ideas/UX, not internals
- **REPLACE** — intentionally build a Molt-native alternative
- **DEFER** — valuable, but not early priority

---

## 2. Toolchain & developer workflow

### 2.1 `uv` (Astral)
**Classification:** USE
**Why:** Best-in-class dependency resolution and reproducibility.

**Molt strategy:**
- Treat `uv.lock` as a canonical input
- Use lockfile hash as part of Molt artifact identity
- Avoid inventing a new resolver

---

### 2.2 Ruff / `ty` (Astral)
**Classification:** USE → EXTEND

**Why:**
- Rust-native, fast, and already trusted
- `ty` can become Molt’s Type Facts producer

**Molt strategy:**
- Integrate `ty` into `molt check`
- Consume machine-readable type facts
- Do not reimplement a type checker

---

## 3. Web boundary & schema ecosystem

### 3.1 Pydantic v2
**Classification:** USE (authoring) → REPLACE (runtime)

**Why:**
- De facto standard for schema definition
- Rust core already exists
- Huge adoption in FastAPI/Django ecosystems

**Molt strategy:**
- Accept Pydantic models as input
- Compile them into Schema IR + codecs
- Eliminate Pydantic runtime calls in hot paths

---

### 3.2 msgspec
**Classification:** EMULATE

**Why:**
- Excellent example of typed, fast codecs
- Clean API design
- Strong performance benchmarks

**Molt strategy:**
- Learn from its design
- Potentially interop
- Build Molt-native compiled codecs instead

---

### 3.3 orjson
**Classification:** USE (fallback / reference)

**Why:**
- Fast JSON
- Widely deployed
- Useful baseline for benchmarks

**Molt strategy:**
- Optional fallback
- Benchmark reference
- Not a core dependency long-term

---

## 4. Web frameworks & servers

### 4.1 FastAPI
**Classification:** EMULATE

**Why FastAPI wins:**
- Schema-driven APIs
- Minimal ceremony
- Automatic docs
- Async-first ergonomics

**Why Molt should not clone it:**
- Heavy runtime reflection
- DI graph complexity
- ASGI constraints

**Molt strategy:**
- Preserve ergonomics
- Compile routing + schemas
- Avoid ASGI as a core abstraction

---

### 4.2 Django
**Classification:** USE (control plane)

**Why:**
- Massive ecosystem
- ORM, admin, auth
- Deep production trust

**Molt strategy:**
- Do not replace Django
- Offload hot endpoints/jobs via `molt_accel`
- Gradual migration path

---

### 4.3 Starlette / ASGI
**Classification:** DEFER / ADAPTER ONLY

**Why:**
- Dominant async interface today
- Flexible but dynamic

**Molt strategy:**
- Optional adapter for interop
- Do not design Molt runtime around ASGI

---

## 5. Async runtime & HTTP foundations (Rust side)

### 5.1 Tokio
**Classification:** USE

**Why:**
- Industry-standard async runtime
- Mature scheduling and IO

---

### 5.2 Hyper / h2 / rustls
**Classification:** USE

**Why:**
- Battle-tested HTTP/TLS stack
- Small, composable pieces

**Molt strategy:**
- Build Molt HTTP runtime on these
- Expose Python-shaped APIs on top

---

## 6. Database & storage

### 6.1 Postgres drivers (asyncpg, psycopg3)
**Classification:** EMULATE (semantics)

**Why:**
- Good async patterns
- Familiar APIs

**Molt strategy:**
- Implement Molt-native async DB layer
- Focus on cancellation, typed decoding, fairness

---

### 6.2 SQLAlchemy ORM
**Classification:** DEFER / PARTIAL EMULATION

**Why:**
- Powerful but heavy
- Dynamic by design

**Molt strategy:**
- Do not replicate ORM
- Offer adapters or query builders
- Focus on fast execution, not ORM expressiveness

---

## 7. Data & analytics

### 7.1 Apache Arrow
**Classification:** USE (core interop)

**Why:**
- Universal columnar format
- IPC, analytics, ML, DBs

**Molt strategy:**
- Make Arrow the backbone for tabular data
- Use it for IPC, WASM, analytics

---

### 7.2 Polars
**Classification:** USE

**Why:**
- Best-in-class DataFrame engine
- Rust-native, vectorized

**Molt strategy:**
- Use Polars as execution backend
- Layer pandas-compat surface gradually

---

### 7.3 DuckDB
**Classification:** USE

**Why:**
- SQL engine embedded anywhere
- Complements Polars perfectly

**Molt strategy:**
- Integrate for analytics-heavy workloads
- Avoid building a query optimizer from scratch

---

## 8. Background jobs & workers

### 8.1 Celery / RQ / Dramatiq
**Classification:** REPLACE

**Why replace:**
- External brokers
- Poor cancellation semantics
- Operational complexity

**Molt strategy:**
- Native structured concurrency
- No broker required initially
- Explicit retries and backpressure

---

## 9. WASM & portability

### 9.1 WASM runtimes (wasmtime/wasmer)
**Classification:** USE

**Why:**
- Portable execution
- Browser/server symmetry

**Molt strategy:**
- Compile Molt modules to WASM
- Define stable ABI + schema contracts

---

## 10. Observability

### 10.1 tracing / OpenTelemetry
**Classification:** USE

**Why:**
- Structured, async-aware
- Industry standard

**Molt strategy:**
- Make observability default
- Propagate context across tasks and IPC

---

## 11. Summary table

| Area | Use | Emulate | Replace |
|----|----|----|----|
| Dependency resolution | uv | | |
| Typing | ty | | |
| Schemas | Pydantic (authoring) | | Runtime |
| Web UX | | FastAPI | |
| Control plane | Django | | |
| HTTP runtime | Tokio/Hyper | | |
| Analytics | Arrow/Polars/DuckDB | | |
| Background jobs | | | Celery |
| WASM | wasmtime | | |

---

## 12. Guiding rule
> **Molt should only replace things where the replacement unlocks a fundamentally better contract.**

Everything else should be leveraged, not rebuilt.

This discipline is how Molt stays small, fast, and inevitable.
