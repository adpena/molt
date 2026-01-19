# Molt DB IPC Contract: Query Requests and Results
**Spec ID:** 0915
**Status:** Draft (implementation-targeting)
**Priority:** P0 (async DB + demo offload path)
**Audience:** DB/runtime engineers, framework integrators
**Goal:** Define a stable IPC payload contract for DB queries executed by `molt_worker`, shared by Django/Flask/FastAPI adapters.

---

## 0. Scope
- Phase 0: SQLite-backed demo reads via `list_items` and an initial `db_query` contract.
- Phase 1: async Postgres client uses the same contract with full cancellation + pooling.
- This contract is framework-agnostic and intended for reuse across Django/Flask/FastAPI adapters.

---

## 1. Entrypoints
- `db_query` (read-only by default)
  - Executes a parameterized SQL query and returns rows in a deterministic format.
- `db_exec` (mutations only; requires explicit `allow_write=true` and `db.write` capability gating)
  - Executes a parameterized write statement and returns rows-affected metadata.

---

## 2. Request payload (MsgPack)
```json
{
  "db_alias": "default",
  "sql": "select id, status from items where status = :status",
  "params": {
    "mode": "named",
    "values": [
      {"name": "status", "value": "open"}
    ]
  },
  "max_rows": 1000,
  "result_format": "json",
  "allow_write": false,
  "tag": "items_list"
}
```

### 2.1 Field semantics
- `db_alias`: named database target (defaults to `default`).
- `sql`: non-empty SQL string; must be parameterized.
- `params`:
  - `mode=positional`: values are an ordered list.
  - `mode=named`: values are a list of `{name, value}` pairs, sorted by name.
  - Parameter entries may include an optional `type` (string) for explicit typing.
- `max_rows`: upper bound on rows returned (enforced by the worker). Defaults to `MOLT_DB_MAX_ROWS` when omitted.
- `result_format`: `json`, `msgpack`, or `arrow_ipc` (defaults to `json`).
- `allow_write`: must be `true` to allow mutation statements; otherwise writes are rejected.
- `tag`: optional query tag for metrics/logging.
  - For `db_exec`, `result_format` must be `json` or `msgpack` and `max_rows` is ignored.

### 2.2 Allowed parameter types
- `null`, `bool`, `int`, `float`, `str`, `bytes`.
- `null` requires an explicit `type` (e.g., `int8`, `text`, `uuid`) to avoid ambiguous binds.
- Anything else is rejected with `InvalidInput`.

---

## 3. Response payloads
Responses use the standard worker response envelope (`status`, `payload`, `error`, `metrics`).

### 3.1 `result_format=json` or `msgpack`
Payload:
```json
{
  "columns": ["id", "status"],
  "rows": [[1, "open"], [2, "closed"]],
  "row_count": 2
}
```

### 3.2 `result_format=arrow_ipc`
Payload is raw Arrow IPC bytes with the column schema and data batch.
The worker encodes Arrow IPC stream bytes and returns them with `codec=arrow_ipc`
so clients should treat the payload as raw bytes (no JSON/MsgPack decode).

### 3.3 Complex Postgres value encodings (`json`/`msgpack`)
- Arrays: nested lists of values (multi-dimensional arrays preserve nesting). If any lower bound is not `1`, the response uses a wrapper with explicit bounds:
  ```json
  {"lower_bounds": [0], "values": [1, 2, 3]}
  ```
- Ranges: objects with bounds and inclusivity:
  ```json
  {"empty": false, "lower": {"value": 1, "inclusive": true}, "upper": {"value": 10, "inclusive": false}}
  ```
  Use `null` for unbounded bounds (e.g., `"lower": null`).
- Multiranges: list of range objects using the same range encoding as above.
- Intervals: objects with signed components:
  ```json
  {"months": 1, "days": -7, "micros": 1234567}
  ```

### 3.4 Complex Postgres value encodings (`arrow_ipc`)
- Arrays: `Struct<lower_bounds: List<Int32>, values: List<...>>`, where `values` nests one List per dimension.
- Ranges: `Struct<empty: Bool, lower: Struct<value: T, inclusive: Bool>, upper: Struct<value: T, inclusive: Bool>>`.
- Multiranges: encoded as array structs with `lower_bounds` plus `values` as `List<RangeStruct>`.
- Intervals: `Struct<months: Int32, days: Int32, micros: Int64>`.
Lower bounds are always present in Arrow IPC, even when all bounds are `1`.

### 3.5 `db_exec` response (json/msgpack)
Payload:
```json
{
  "rows_affected": 12,
  "last_insert_id": 123
}
```
- `rows_affected`: count of rows mutated by the statement.
- `last_insert_id`: present for SQLite inserts when available; omitted otherwise.

---

## 4. Errors + status mapping
- `InvalidInput`: malformed SQL, params, or format.
- `Busy`: pool exhausted beyond max wait.
- `Timeout`: query exceeded deadline.
- `Cancelled`: request cancelled by the client.
- `InternalError`: unexpected driver or decoding failure (no crash).

---

## 5. Observability
- `metrics` includes `queue_us`, `handler_us`, `exec_us`, `decode_us`.
- DB-specific metrics include `db_alias`, `db_tag` (when set), `db_row_count`,
  `db_bytes_in`, `db_bytes_out`, and `db_result_format`. `db_exec` also reports
  `db_rows_affected` and `db_last_insert_id` when available.
- Metrics values may be numeric or string (strings for `db_alias`, `db_tag`,
  `db_result_format`).

---

## 6. Determinism requirements
- Parameter ordering for named params is canonical (sorted by name).
- Result ordering is deterministic per SQL semantics (caller should specify `ORDER BY` when needed).
- `max_rows` enforcement is strict.

---

## 7. Validation + tests
- Unit tests for payload validation in `molt_db_adapter`.
- Integration tests with SQLite (Phase 0) and Postgres (Phase 1).
- Cancellation tests must verify that long-running queries abort.

---

## 8. WASM host interface (db_query/db_exec)
WASM builds must call the host via WIT intrinsics that mirror the IPC request
shape. The host is responsible for executing the query and streaming the
response bytes back into the module.

### 8.1 WIT signatures
```
db_query(ptr: molt-ptr, len: u64, out: molt-ptr, cancel_token: molt-object) -> s32
db_exec(ptr: molt-ptr, len: u64, out: molt-ptr, cancel_token: molt-object) -> s32
```
- `ptr/len`: MsgPack-encoded request payload (same schema as section 2).
- `out`: pointer to a `u64` where the host writes a stream handle.
- `cancel_token`: `None` or an integer cancel token id (use current token when `None`).

### 8.2 Return codes
- `0`: OK, stream handle written to `out`.
- `1`: invalid input (null pointer with non-zero length, invalid token).
- `2`: invalid output pointer.
- `6`: capability denied (`db.read`/`db.write`).
- `7`: host unavailable or internal error before streaming begins.

### 8.3 Response stream format
The host writes a stream handle and then sends frames over the stream:
1) **Header frame** (MsgPack map):
   - `status`: `ok`, `invalid_input`, `busy`, `timeout`, `cancelled`, `internal_error`
   - `codec`: `json`, `msgpack`, or `arrow_ipc`
   - `payload`: optional bytes (required for `json`/`msgpack`)
   - `error`: optional string
   - `metrics`: optional map (same keys as section 5)
2) **Payload frames** (optional):
   - For `arrow_ipc`, the host sends raw Arrow IPC bytes as one or more frames.
   - For `json`/`msgpack`, no payload frames are sent; the header `payload` holds the bytes.

The host must close the stream after the last frame. Errors after the header
should close the stream and are surfaced as `internal_error` unless the host can
send a specific status header first.
