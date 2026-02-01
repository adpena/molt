# Molt DB Layer Vision: Async-First, High-Throughput, Django-Friendly
**Spec ID:** 0700
**Status:** Draft (product + architecture)
**Priority:** P0 for service usefulness
**Audience:** runtime engineers, DB engineers
**Goal:** Deliver a Molt-native async DB layer (starting with Postgres) that enables Go-class concurrency for web services and provides a practical Django adapter/migration path—without rewriting the Django ORM.
**Implementation status:** `runtime/molt-db` contains a bounded pool skeleton (sync, connection-agnostic), a feature-gated async pool primitive, a native-only SQLite connector used by `molt-worker`, and an async Postgres connector (tokio-postgres + rustls) with cancellation + statement caching. `molt-worker` now exposes `db_query`/`db_exec` for SQLite (sync) and Postgres (async) with Arrow IPC output for reads (including complex-type struct encodings for arrays/ranges/intervals/multiranges) and expanded Postgres type decoding (uuid/json/date/time → strings, arrays/ranges/intervals/multiranges → structured with lower-bound metadata). WASM parity now has a defined host interface + capability gating with a Node/WASI adapter in `run_wasm.js`; wasm-side `molt_db` client shims consume DB response streams into bytes/Arrow IPC.

---

## 0. Executive Summary
Rewriting the Django ORM for “true async” is a multi-year compatibility trap.
Instead, Molt should deliver an **async-first DB layer** that:
- provides **fast, predictable** database access
- integrates with Molt tasks/channels and structured cancellation
- decodes results into **typed structs** or **Arrow batches** (no Python object overhead in DF0)
- offers a **Django-friendly adapter** and IPC bridge for incremental adoption

This yields most of the real-world performance and concurrency wins at a fraction of the cost.

---

## 1. Product Goals
### 1.1 P0 goals
- Async Postgres client with pooling, prepared statements, cancellation, timeouts
- Efficient row decoding (typed) and bulk data paths (Arrow)
- Safe transaction scopes
- Observability: query metrics, pool stats, latency histograms
- Compatibility story for Django apps via:
  - IPC (Django → molt_worker → DB)
  - optional adapter surface (query builder that feels familiar)

### 1.2 Non-goals (initial)
- Full Django ORM semantics (signals, model hooks, all query features)
- Migrations and schema management parity (phase later)
- Supporting every DB at once (start with Postgres; add MySQL/SQLite later)

---

## 2. Key Concepts
- **Pool**: manages DB connections, backpressure, and fairness
- **Session**: a logical handle for executing queries (often scoped to request)
- **Transaction**: scope-managed, cancellation-aware
- **Statement**: prepared statement with cached plan and typed parameters
- **RowDecoder**: maps wire types → Rust-native → Molt-native representations

---

## 3. Why this beats rewriting Django ORM
- Most Django production cost is in DB I/O + serialization + row hydration.
- Async matters most for concurrency; the rest of the win comes from efficient decoding and fewer allocations.
- Django ORM compatibility is huge and subtle; recreating it slows Molt down and compromises the “no CPython extension” story.

---

## 4. Integration points with Molt runtime
- DB operations must **never block worker threads**; they park tasks on I/O readiness.
- Cancellation must propagate from request scope → DB ops → network socket.
- Metrics must be exported in a production-friendly format.

---

## 5. Deliverables
- `molt_db` crate (Rust)
- Molt runtime bindings (MIR ops or runtime calls)
- `molt_sql` query builder (subset)
- `molt_db_adapter` (Python package) using IPC to call `molt_worker` DB endpoints (Phase 1)
- Framework-agnostic DB IPC contract (`docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`) to share payload builders across Django/Flask/FastAPI.
