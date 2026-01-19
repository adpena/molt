# Molt DataFrame Execution Engine: Plan IR, Delegation, Fusion, Parallelism
**Spec ID:** 0502
**Status:** Draft
**Audience:** engine implementers, compiler/runtime engineers
**Goal:** Define how DataFrame operations execute fast while allowing multiple backends (Polars, DuckDB, Molt-native kernels).

## 0. Architecture Overview
Molt DataFrame has:
- API layer: DataFrame/Series/Expr
- Plan IR: a backend-agnostic DAG
- Engines:
  - PolarsEngine (primary Phase 1)
  - DuckDBEngine (SQL/optimizer escape hatch)
  - KernelEngine (Molt-native kernels, phased)

## 1. Plan IR (high-level)
### 1.1 Node kinds (minimum useful set)
- Source: InMemoryTable, ScanFile(CSV/Parquet), ScanDB(query)
- Project (select columns/expressions)
- Filter (predicate expr)
- WithColumn / Assign
- Aggregate (groupby keys + agg exprs)
- Join (left/right + join keys + join type)
- Sort
- Limit
- Distinct
- Explode (DF1+)
- Union/Concat

### 1.2 Expression IR (Expr)
- ColumnRef(name)
- Literal(value)
- BinaryOp(op, lhs, rhs)
- UnaryOp(op, x)
- Cast(dtype)
- IsNull / FillNull / Coalesce
- String ops (subset)
- Datetime ops (subset)
- UDFCall (restricted; DF1+)

## 2. Delegation Strategy (Phase 1)
Rules:
- Prefer Polars for dataframe-native expressions and lazy pipelines.
- Delegate to DuckDB when:
  - join/aggregate complexity benefits from SQL optimizer
  - window functions or advanced SQL features are needed
  - query can be pushed down efficiently

Decision can be:
- rule-based initially
- later profile-guided (cost model)

## 3. Fusion and avoiding intermediates
Phase 1:
- leverage Polars lazy execution for fusion
- avoid materializing intermediate tables

Phase 2:
- add Molt Plan-level fusion for engine-agnostic optimizations

## 4. Parallel execution
- Polars uses parallelism internally; expose config knobs
- KernelEngine uses Molt tasks/channels to run partitions in parallel
- For service workloads: avoid spawning excessive tasks per request; use bounded pools

## 5. Streaming execution (pipelines)
Support:
- chunked sources
- backpressure via bounded channels
- incremental aggregation (where feasible)

## 6. Determinism and reproducibility
- deterministic mode should avoid nondeterministic parallel reductions unless explicitly allowed
- stable sort requirements explicit

## 7. Hooks for Molt compilation
The compiler can:
- specialize expressions for known schemas
- generate native kernels for hot Expr subgraphs
- insert guards for schema stability

## 8. Acceptance criteria
- A non-trivial ETL DAG runs with minimal copies and measurable speedups vs pandas
- Plan IR can round-trip between engines with consistent outputs
