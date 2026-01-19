# Async Postgres: Pooling, Wire Protocol, Prepared Statements
**Spec ID:** 0701
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** DB/runtime engineers
**Goal:** Define the Molt-native async Postgres client behavior (pooling, backpressure, cancellation) suitable for service workloads.

---

## 0. Implementation philosophy
- Prefer a mature Rust ecosystem foundation (tokio + rustls) where appropriate.
- Keep the public Molt API stable and minimal; internal implementation can evolve.
- Treat tail latency as a correctness constraint.

---

## 1. Connection Pool
### 1.1 Pool parameters (required)
- `min_conns`, `max_conns`
- `max_idle_ms`
- `connect_timeout_ms`
- `query_timeout_ms` (default per request)
- `max_wait_ms` (backpressure limit for acquiring a connection)
- `health_check_interval_ms`
- `statement_cache_size`

**Implementation status:** `molt-db` now provides a feature-gated async pool plus an async Postgres connector (tokio-postgres + rustls) with per-connection statement caching; `molt-worker` uses it for `db_query`/`db_exec` with cancellation. Type decoding now covers uuid/json/date/time (stringified) plus arrays/ranges/intervals/multiranges (structured), with explicit lower-bound metadata when needed; Arrow IPC now supports complex type encodings and wasm parity remains pending (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:missing): wasm parity for DB client shims/tests).

### 1.2 Acquire semantics
Acquire must be:
- async (task parks; no OS thread block)
- fairness-aware (avoid starvation)
- cancellation-aware (if scope cancels, acquire returns Cancelled)

### 1.3 Backpressure
If pool is exhausted:
- callers wait up to `max_wait_ms`
- then fail with `PoolExhausted` (or return BUSY in IPC)

---

## 2. Prepared statements
### 2.1 Statement cache
- LRU cache per connection or per pool (policy-defined)
- keyed by SQL + parameter types
- avoid re-prepare storms under load

**Implementation note:** current `molt-db` Postgres connector uses a per-connection LRU keyed by SQL+types, sized via `statement_cache_size`.

### 2.2 Typed parameters
- DF0 requires explicit types for parameters (or inferred safely)
- avoid implicit string formatting (SQL injection risk and slow)

---

## 3. Cancellation + timeouts
- Every query attaches to a cancellation token
- On cancellation:
  - attempt protocol-level cancel where feasible
  - drop/close socket if required to guarantee stop
- Timeouts:
  - enforced both client-side and optionally via server-side statement timeout (configurable)

---

## 4. TLS + auth
- TLS via rustls
- support:
  - password auth
  - SCRAM (phase-in)
  - IAM-ish auth (out of scope v0.1 unless needed)

---

## 5. Observability
Export:
- pool size, in-use, idle, waiters
- acquire latency
- query latency histogram
- error counts by code
- bytes sent/received

---

## 6. Testing
- integration tests against a real Postgres in CI (docker)
- fuzz parameter decoding/encoding boundaries
- soak tests for pool contention
- cancellation tests (ensure queries stop)

---

## 7. WASM parity plan (required)
- Implemented: WIT host interface for `db_query`/`db_exec` with stream handles + Arrow IPC streaming headers; `db.read`/`db.write` capability gating enforced in `molt-runtime`.
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:missing): implement wasm-side `molt-db` shims that consume the response stream and surface results as bytes/Arrow IPC.
- Implemented: Node/WASI host adapter in `run_wasm.js` that forwards `db_query`/`db_exec` to `molt-worker` and streams responses via the DB host interface.
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P2, status:planned): ship additional production host adapters (CF Workers, browser) and wasm parity tests that exercise real DB backends with cancellation.
