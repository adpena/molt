# Molt DB IPC Contract: Query Requests and Results
**Spec ID:** 0915
**Status:** Draft (implementation-targeting)
**Priority:** P0 (async DB + demo offload path)
**Audience:** DB/runtime engineers, framework integrators, AI coding agents
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
- Future: `db_exec` for mutations (requires explicit `allow_write=true` and capability gating).

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
- `max_rows`: upper bound on rows returned (enforced by the worker).
- `result_format`: `json`, `msgpack`, or `arrow_ipc`.
- `allow_write`: must be `true` to allow mutation statements; otherwise writes are rejected.
- `tag`: optional query tag for metrics/logging.

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
- DB-specific metrics should include `db_alias`, `tag`, row count, and bytes in/out.

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
