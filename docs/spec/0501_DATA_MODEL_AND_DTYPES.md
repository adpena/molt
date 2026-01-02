# Molt Data Model and DTypes (Arrow-First)
**Spec ID:** 0501  
**Status:** Draft (implementation-targeting)  
**Audience:** runtime engineers, kernel authors, AI coding agents  
**Goal:** Define a columnar data model suitable for Polars/DuckDB integration now and Molt-native kernels later.

## 0. Guiding Decisions
- **Arrow-compatible columnar arrays** are the canonical in-memory representation.
- DataFrames are collections of named columns + metadata.
- Nulls are explicit (bitmap) and never represented as boxed Python objects in DF0.

## 1. Core Types
### 1.1 Scalar dtypes (DF0 required)
- Int: i8/i16/i32/i64, u8/u16/u32/u64
- Float: f32/f64
- Bool
- Utf8 string (variable length, offset buffer + data buffer)
- Binary (bytes)
- Date/Datetime (naive) as i32/i64 with unit metadata
- Duration

### 1.2 Nullable dtypes
Nullability is a property of the array:
- values buffer
- optional validity bitmap
- optional offsets buffer (strings/binary/list)

## 2. Structured and nested types (DF1+)
- List
- Struct
- Categorical/Dictionary encoding
- Decimal (phase-in; requires careful semantics)

## 3. Index Model (staged)
### 3.1 DF0
- Optional “row id” index or no index.
- No automatic alignment by index.
- Operations are row-position based by default.

### 3.2 DF1
- Single-level index with limited alignment semantics.
- Explicit opt-in alignment operations.

### 3.3 DF2
- Modern core pandas-style index behaviors (phased; requires extensive oracle tests).

## 4. Object dtype policy
Object dtype is the trapdoor into slow Python semantics.
Policy:
- DF0: disallow by default (compile-time error or explicit opt-in that routes to slow path)
- DF1: allow with restrictions (e.g., only strings, or only small payloads)
- DF2: aim for compatibility via controlled representation, but treat as a performance escape hatch, not default

## 5. Coercion and type promotion
Define explicit, deterministic promotion rules for DF0:
- integer + integer -> widen
- int + float -> float
- bool + int -> int
- string concatenation rules explicit
- missing data promotion explicit

DF1+ expands toward pandas-like coercion rules. All expansions must be backed by differential tests vs pandas.

## 6. Memory layout and performance
- Favor contiguous buffers
- Use chunked arrays for streaming/large data
- Avoid per-row allocations
- Prefer dictionary encoding for repetitive strings (phase-in)

## 7. Interop requirements
### 7.1 Polars
- Must map Molt arrays to Polars series with minimal copy.
### 7.2 DuckDB
- Prefer Arrow scan / Arrow exchange paths.
### 7.3 Browser/WASM
- Arrow IPC bytes are the portable interchange format.
