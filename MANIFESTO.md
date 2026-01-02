# Molt: A Founding Manifesto

## What Molt Is

Molt is a compiler and runtime that transforms a **verified per-application subset of Python**
into **extremely fast native binaries**, primarily for:

- web servers
- API backends
- workers and background jobs
- databases and database-adjacent code
- data pipelines and ETL

Molt is not a Python interpreter.
Molt is not a Python VM.
Molt is not a CPython fork.

**Molt is Python shedding its skin into native code.**

---

## Core Beliefs

### 1. Performance Comes from Semantics, Not Syntax
Python is slow not because it is “interpreted”, but because:
- everything is dynamic
- everything is heap-allocated
- everything is indirect
- everything is mutable

Molt’s job is to discover which of those properties are **actually used** by a program,
and eliminate the rest.

---

### 2. Compatibility Is a Product Decision
Perfect Python compatibility is not a virtue.
**Explicit, documented tradeoffs** are.

Molt defines compatibility tiers:
- Tier 0: frozen, production-grade, fastest possible
- Tier 1: guarded, transitional, slower but flexible

Users choose the tier.
Molt enforces the rules.

---

### 3. The Runtime Matters More Than the Language
Most performance wins come from:
- data layout
- allocation strategy
- object representation
- specialization
- concurrency model

The implementation language is secondary.
That said: **Rust is the correct spine for Molt.**

---

### 4. Fast by Default, Correct by Construction
Molt prefers:
- static proofs where possible
- runtime guards where necessary
- explicit deoptimization paths
- differential testing against CPython

Crashes are bugs.
Silent miscompilation is unacceptable.

---

### 5. AI Is a Tool, Not a Crutch
AI is used:
- at development time
- to propose invariants
- to generate tests
- to explore optimization space

AI is **not required at runtime**.

---

## The Endgame

If Molt succeeds:
- Python web services will deploy like Go services
- binaries will be small, fast, and self-contained
- latency-sensitive workloads will be viable
- Python will no longer be synonymous with “slow backend”

That is the standard.
Anything less is not worth building.