# I/O, Async, and Connectors (CSV/Parquet/DB) for Molt DataFrame
**Spec ID:** 0505  
**Status:** Draft  
**Audience:** runtime engineers, connector authors, AI coding agents  
**Goal:** Make Molt DataFrame useful for production pipelines by providing fast I/O and async-friendly connectors.

## 0. Core philosophy
- Compute is parallel; I/O is async-capable.
- Keep I/O off the scheduler threads (nonblocking or bounded blocking pools).
- Use Arrow as the interchange format.

## 1. File formats
### 1.1 CSV
- fast CSV reader/writer (Rust)
- schema inference with explicit override
- streaming/chunked reading

### 1.2 Parquet
- Parquet reader/writer (Rust)
- predicate pushdown where possible
- column projection pushdown
- streaming record batches

### 1.3 Arrow IPC
- canonical interchange for:
  - molt_worker IPC
  - browser/server transfers
  - interoperability with DuckDB/Polars

## 2. Databases
### 2.1 Preferred route: Molt-native DB connectors
- Postgres async client
- MySQL async client
- SQLite embedded option

### 2.2 Query execution model
- `read_sql` returns a DataFrame backed by Arrow batches
- streaming fetch with backpressure into Molt tasks/channels
- prepared statement caching

### 2.3 DuckDB as connector and optimizer
DuckDB can be used for:
- reading from many sources
- pushing down complex SQL
- joining external tables

## 3. Async story
Async is primarily for:
- reading/writing streams
- DB fetch
- network storage (S3-like) where supported

Compute remains parallel and vectorized; async wraps I/O boundaries.

## 4. Integration with Molt tasks/channels
- I/O stages emit Arrow batches into bounded channels
- compute stages consume and transform
- sinks write out incrementally
This enables true streaming pipelines.

## 5. Acceptance criteria
- Read CSV/Parquet into DataFrame faster than pandas on a representative dataset (or at minimum within a competitive band)
- DB read can stream and does not block scheduler threads
- Arrow IPC round-trip is correct and stable
