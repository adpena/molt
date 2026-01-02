# Differential Testing: Pandas as Oracle (Correctness at Scale)
**Spec ID:** 0504  
**Status:** Draft (implementation-targeting)  
**Audience:** test engineers, kernel authors, AI coding agents  
**Goal:** Verify Molt DataFrame semantics by comparing outputs to pandas on the supported subset.

## 0. Why differential testing
Reimplementing pandas semantics from docs alone is error-prone.
The only reliable truth is:
- pandas behavior on real inputs
- for the subset we claim to support

We treat pandas as an oracle and build automated equivalence tests.

## 1. Test categories
### 1.1 Golden tests
Handwritten cases for known tricky behaviors:
- dtype coercions
- null handling
- join corner cases
- sorting stability
- groupby edge cases

### 1.2 Property tests (Hypothesis)
Generate random tables with bounded sizes and dtypes, then:
- run operation in pandas
- run operation in Molt DF engine
- compare results (values, dtypes, null masks, ordering semantics)

### 1.3 Metamorphic tests
Test invariants like:
- filter twice equals filter with combined predicate
- groupby agg on concatenated partitions equals agg merged (when associative)
- sorting after stable operations preserves expected properties

## 2. Comparison rules (must be explicit)
Comparing DataFrames requires choices:
- ordering: when is order guaranteed?
- floating point tolerance: use ulp/relative tolerance
- NaN/NA equivalence: pandas has subtle rules; define them per dtype
- categorical ordering: define policy

All comparisons must be implemented as library functions with unit tests.

## 3. Test harness design
- A Python harness that can run both engines:
  - pandas reference
  - Molt DF engine (via IPC to `molt_worker` or in-process if available)
- Seeded randomness for reproducibility
- Corpus minimization on failure (store reduced failing inputs)

Artifacts:
- failing case saved as Arrow IPC + JSON metadata
- attach to CI artifacts for debugging

## 4. CI strategy
- fast suite on every PR (small tables)
- nightly suite with heavier randomized workloads
- performance regression suite gated separately

## 5. Version pinning
Oracle testing requires version pinning:
- pin pandas/numpy versions for CI oracle runs
- document which oracle versions are supported

## 6. Acceptance criteria
- Each supported API in 0503 has:
  - at least N golden tests
  - property tests covering a range of dtypes and sizes
- Any divergence is either:
  - a bug (fix)
  - a documented semantic difference with explicit policy flag
