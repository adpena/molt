# Row Decoding: Typed Structs and Arrow Batches (No Python Object Tax)
**Spec ID:** 0703
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** DB engineers, data engineers
**Goal:** Make DB reads fast by decoding directly into efficient representations.

---

## 0. Two output modes
### 0.1 Typed struct mode (services)
- map columns directly into a fixed struct layout
- minimal allocations
- ideal for request/response services

### 0.2 Arrow batch mode (data/pipelines)
- decode into Arrow record batches
- enables zero/low-copy interchange with:
  - Polars
  - DuckDB
  - Arrow IPC over wire
- ideal for ETL and analytics endpoints

---

## 1. Type mapping (Postgres → Molt)
Define explicit mappings:
- int2/int4/int8 → i16/i32/i64
- float4/float8 → f32/f64
- text/varchar → utf8 (owned buffer; optionally dictionary encoded)
- bytea → binary
- bool → bool
- timestamp/date → i64/i32 with unit metadata
- json/jsonb → bytes or parsed (policy) (TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): json/jsonb decode policy).

Unsupported types:
- require explicit casting in SQL or return as bytes in DF1 (TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): unsupported type fallback policy).

---

## 2. Null handling
- null bitmap per column
- service structs may use Option<T> or sentinel representation depending on policy
  (TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): option vs sentinel policy).

---

## 3. Decoding performance requirements
- avoid per-row dynamic dispatch
- prefer columnar decoding
- reuse buffers across batches
- batch size configurable (default tuned)

---

## 4. Testing
- correctness tests per type mapping
- fuzz invalid encodings
- large batch performance tests
