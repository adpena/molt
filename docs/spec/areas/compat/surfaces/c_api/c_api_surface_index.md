# C-API Surface Index

**Status:** Active
**Owner:** runtime + tooling
**Scope:** `libmolt` C-API contract and symbol coverage map.

## Canonical C-API Surface Files
- `libmolt` C-API v0 contract:
  - `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`
- C-API symbol matrix:
  - `docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md`

## Source Alignment
Track C-API surface against CPython C-API docs:
- `docs/python_documentation/python-3.12-docs-text/c-api/index.txt`

## Non-Negotiable Rule
If a symbol is listed as implemented, there must be a reproducible test and a concrete runtime implementation entry in the same change.

## Ecosystem ABI Rule
C-API symbol coverage is necessary but not sufficient for ecosystem support.
NumPy/SciPy/pandas-style claims must advance through the source-recompiled
extension pipeline: compile upstream sources against Molt headers, link Molt
runtime symbols, stage package-native artifacts through import custody, execute
deterministic workloads, and report the reachable object/symbol closure. Do not
replace that pipeline with Molt-side reimplementations of whole upstream APIs.
