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
