# Molt DataFrame Vision: Pandas-Useful Fast Path, Long-Term Modern Compatibility
**Spec ID:** 0500
**Status:** Draft (product + architecture)
**Priority:** P0 (usefulness), P1 (long-term compatibility)
**Audience:** runtime/compiler engineers, data engineers, AI coding agents

## 0. Executive Summary
Molt’s data stack should become **super useful for pandas-heavy workloads** without requiring CPython C extensions.
The fastest path is **integration-first**:

- **Arrow** as the universal columnar memory format
- **Polars** as the primary vectorized compute engine in Phase 1
- **DuckDB** as the SQL optimizer/execution escape hatch in Phase 1–2
- A Molt-owned **DataFrame API and Plan IR** that can target multiple engines
- A long-term roadmap to a **Molt-native kernel library** and a **modern pandas-core compatibility layer**

This avoids reinventing decades of work while preserving the ability to own performance, semantics, and WASM portability over time.

## 1. Product Goals
### 1.1 Primary goals
1) **Fast runtime** on real ETL and service workloads (joins, groupby, serialization, feature engineering)
2) **Production hardening** (determinism where declared, strong testing oracle, predictable memory)
3) **Deployment simplicity** (single binary in Molt-native mode, no CPython extension loading)
4) **Great developer experience** (pandas-like ergonomics where it matters)

### 1.2 Non-goals (early)
- Full historical pandas compatibility (legacy edge cases, deprecated APIs, exotic object-dtype behavior)
- A “drop-in” replacement for every 3rd-party pandas plugin
- In-browser “full pandas” at large scale (browser constraints dominate)

## 2. The Long-Term Dream (Modern Core Pandas Compatibility)
Molt’s long-term goal is **full compatibility with the most modern, current “core pandas” API**—meaning:
- prioritize the stable, actively used DataFrame/Series/Index operations and I/O paths
- exclude historically deprecated/legacy surfaces and “accidental behaviors”
- focus on semantics that matter in production data pipelines

### 2.1 What “modern core pandas” means (operational definition)
A “core pandas” compatibility target includes, at minimum:
- `DataFrame` / `Series` construction and column selection
- boolean filtering, `assign`, `rename`, `drop`, `astype`
- `groupby(...).agg(...)` for common aggregates
- `merge/join` for common join types
- `sort_values`, `sort_index` (with a constrained index model early)
- missing data handling for standard nullable dtypes
- `to/from` for: CSV, Parquet, Arrow, and common DB flows (via connectors)
- time series support for modern datetime types (phased; timezones later)

It explicitly treats:
- `object` dtype as a restricted/slow tier
- full index alignment semantics as a staged compatibility tier

### 2.2 Compatibility tiers (DataFrame-specific)
- **DF0: FastFrame (Tier 0)**
  Columnar, vectorized, parallel. Restricted semantics:
  - no object dtype by default
  - limited index semantics (or explicit “no-align” mode)
  - deterministic and predictable performance

- **DF1: Pandas-ish (Tier 1)**
  Adds semantics to ease migration:
  - broader dtype coercions
  - more index features
  - may include guarded slow paths

- **DF2: Modern Core Pandas (long-term)**
  Aim for near-complete compatibility with modern core behaviors and a formal test oracle.

## 3. Architecture Overview
### 3.1 The key abstraction: Molt DataFrame API + Plan IR
Molt owns:
- user-facing API (`molt_dataframe.DataFrame`, `Series`, `Expr`)
- internal **Plan IR** describing transformations
- execution engines:
  - Polars engine
  - DuckDB engine
  - Molt-native kernels engine (later)

This ensures Molt can:
- ship quickly using mature engines
- gradually replace or specialize kernels
- compile the orchestration and concurrency story cleanly

### 3.2 Why Arrow is mandatory
Arrow provides:
- portable columnar memory model
- easy interchange between engines
- clean IPC bridge for CPython/pandas fallback
- a plausible browser/WASM story for smaller datasets

## 4. “Super Useful” Use Cases (P0)
- Web services doing feature transforms on request payloads
- Workers performing batch ETL
- Joining/aggregating tables for analytics endpoints
- Streaming pipelines with backpressure (integrate with Molt tasks/channels)
- Pandas migration: keep code shape, get speed and deployment simplicity

## 5. What ships first (Phase 1)
- Molt DataFrame API + Plan IR
- Polars backend for most operations
- DuckDB backend for complex joins/SQL-style analytics and query planning
- Arrow IPC bridge for:
  - process boundary calls to CPython pandas (migration safety valve)
  - browser/server data exchange (moderate sized payloads)

## 6. Success Metrics
- 10× faster than CPython/pandas on at least one representative ETL workload in CPU-bound phases (or strong improvements in throughput/tail latency in services)
- predictable memory growth (no object-dtype explosions in DF0)
- compatibility: pass a curated “modern core pandas” test suite for the supported subset
