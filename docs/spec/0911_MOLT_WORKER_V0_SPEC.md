# Molt Worker v0: IPC Execution Shell (stdio-first)
**Spec ID:** 0911
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** runtime engineers, AI coding agents
**Goal:** Define the minimal worker that can execute exported entrypoints safely and predictably.
**Implementation status:** Rust stdio shell exists in `runtime/molt-worker` with framing, export allowlist, deterministic demo entrypoints (`list_items`, `compute`, `offload_table`, `health`), plus `db_query`/`db_exec` (SQLite in sync mode, async Postgres with cancellation). `db_exec` requires `allow_write=true` and uses `db.write` capability gating. Cancellation and timeout checks are enforced in the fake DB path, compiled dispatch loops, pool waits, and Postgres queries, with queue/pool metrics emitted per request (microsecond + millisecond fields). Worker runtime is selectable via `--runtime sync|async` / `MOLT_WORKER_RUNTIME`.

---

## 0. Modes
### 0.1 Stdio mode (required v0)
- Worker reads framed messages from stdin, writes framed messages to stdout
- Enables easiest integration, testing, and portability

### 0.2 Unix socket mode (optional v0.1)
- Local domain socket: `/tmp/molt.sock`
- Better for production later, but not required for first win

---

## 1. Entrypoints
Worker loads a manifest at startup, e.g. `molt_exports.json`:
```json
{
  "abi_version": "0.1",
  "exports": [
    {"name": "list_items", "codec_in": "msgpack", "codec_out": "msgpack"}
  ]
}
```

Entrypoints are invoked by name with a payload.

---

## 2. IPC framing (minimal, robust)
- length-prefixed frames (u32 LE length + bytes)
- message encoding: MsgPack (or a tiny custom binary header + bytes)

### 2.1 Request fields (minimum)
- `request_id: u64`
- `entry: string`
- `timeout_ms: u32`
- `codec: enum` (msgpack/json/arrow_ipc reserved)
- `payload: bytes`

### 2.2 Response fields (minimum)
- `request_id: u64`
- `status: enum` (Ok, InvalidInput, Busy, Timeout, Cancelled, InternalError)
- `payload: bytes` (result if Ok)
- `error: string?` (present if not Ok)
- `metrics: map?` (optional v0)

---

## 3. Execution model
- Worker maintains:
  - bounded request queue
  - small fixed threadpool OR async runtime (implementation choice)
- Each request receives:
  - deadline
  - cancellation token (triggerable by client cancel frame or disconnect)

### 3.1 Tuning knobs (env + CLI)
- Threads: `--threads` overrides `MOLT_WORKER_THREADS` (defaults to CPU count).
- Queue depth: `--max-queue` overrides `MOLT_WORKER_MAX_QUEUE` (defaults to 64).
- Runtime: `--runtime sync|async` overrides `MOLT_WORKER_RUNTIME` (defaults to `sync`).
- Max rows: `MOLT_DB_MAX_ROWS` sets the default `db_query` row cap (default 1000).

### 3.2 SQLite DB mode (native)
- Set `MOLT_DB_SQLITE_PATH` to enable SQLite-backed `list_items` reads.
- Default is read-only; set `MOLT_DB_SQLITE_READWRITE=1` for read-write opens.
- `db_query` is available for SQLite reads and `db_exec` for SQLite writes (with `allow_write=true`), following
  `docs/spec/0915_MOLT_DB_IPC_CONTRACT.md` to keep Django/Flask/FastAPI adapters aligned.

### 3.3 Postgres DB mode (async)
- Set `MOLT_DB_POSTGRES_DSN` to enable async Postgres reads (`db_query`) and writes (`db_exec`).
- Requires `--runtime async` or `MOLT_WORKER_RUNTIME=async`.
- Optional tuning:
  - `MOLT_DB_POSTGRES_MIN_CONNS`, `MOLT_DB_POSTGRES_MAX_CONNS`
  - `MOLT_DB_POSTGRES_MAX_IDLE_MS`, `MOLT_DB_POSTGRES_HEALTH_CHECK_MS`
  - `MOLT_DB_POSTGRES_CONNECT_TIMEOUT_MS`, `MOLT_DB_POSTGRES_QUERY_TIMEOUT_MS`
  - `MOLT_DB_POSTGRES_MAX_WAIT_MS`, `MOLT_DB_POSTGRES_STATEMENT_CACHE_SIZE`
  - `MOLT_DB_POSTGRES_SSL_ROOT_CERT`

---

## 4. Timeouts and cancellation
- Worker enforces `timeout_ms` strictly
- On timeout/cancel:
  - return status
  - drop/rollback resources
- Do not leak memory, tasks, or DB connections

**Implementation note:** current worker accepts `__cancel__` requests carrying a `request_id` payload; cancellation is honored during pool waits and execution (fake DB + compiled entrypoints). Async Postgres queries now propagate cancellation via protocol cancel; SQLite remains cooperative via periodic checks.

---

## 5. Safety hardening
- Validate entry name exists
- Reject invalid export names (empty or `__*` reserved); dedupe entries.
- Validate payload size limits
- Reject oversized frames early
- Never panic on malformed input (return InvalidInput)

---

## 7. Compiled Entrypoint Dispatch Plan (accepted, wire next)
- Compile the agreed plan; dispatch wiring is now unblocked by design.
- Schema (v0): `abi_version`, `exports` array of `{name, codec_in, codec_out}`; names must be non-empty, non-reserved, unique.
- Resolution: entry name must exist in the manifest; otherwise `InvalidInput`.
- Loader: compiled entries are registered at startup; if missing at runtime, return `InternalError` with a clear message.
- Cancellation: compiled calls must observe the same cancel/timeout tokens as fake DB; cancellation breaks long loops promptly and returns `Cancelled` with metrics.
- Error mapping: decoding errors → `InvalidInput`; uncaught panics → `InternalError`; missing codec → `InvalidInput`.
- Metrics: emit queue_ms, exec_ms, queue_depth, pool_in_flight/idle (where applicable) for compiled paths as well.
- Acceptance: add a parity test that compiled entries are discoverable from the manifest and that cancellation/timeout/error mapping match the fake handler behavior.
- Manifest schema (v0): `abi_version`, `exports` array of `{name, codec_in, codec_out}`; names must be non-empty, non-reserved, unique.
- Resolution: entry name must exist in the manifest; otherwise `InvalidInput`.
- Loader: compiled entries are registered at startup; if missing at runtime, return `InternalError` with a clear message.
- Cancellation: compiled calls must observe the same cancel/timeout tokens as fake DB; cancellation breaks long loops promptly and returns `Cancelled` with metrics.
- Error mapping: decoding errors → `InvalidInput`; uncaught panics → `InternalError`; missing codec → `InvalidInput`.
- Metrics: emit queue_ms, exec_ms, queue_depth, pool_in_flight/idle (where applicable) for compiled paths as well.
- Acceptance: add a parity test that compiled entries are discoverable from the manifest and that cancellation/timeout/error mapping match the fake handler behavior.

## 6. Observability (minimal v0)
- Log per-request:
  - start/end timestamp
  - queue time
  - exec time
  - status
- Optional: emit a JSON line per request for easy parsing

**Implementation note:** responses now include a `metrics` map with `queue_us`, `handler_us`, `exec_us`, `decode_us`, plus `queue_ms`/`exec_ms` for basic latency insight. `db_query` responses also include `db_alias`, `db_tag` (when set), `db_row_count`, `db_bytes_in`, `db_bytes_out`, and `db_result_format`.

---

## 7. Testing (must)
- unit tests for framing
- fuzz tests for decoding/invalid frames
- “kill worker mid-request” recovery test via `molt_accel` harness
