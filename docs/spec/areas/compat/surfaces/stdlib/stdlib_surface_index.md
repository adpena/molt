# Stdlib Surface Index (CPython 3.12+)

**Status:** Active
**Owner:** stdlib + runtime + tooling
**Scope:** CPython stdlib module/submodule/API coverage, intrinsic lowering status, and platform availability.

## Canonical Inputs
- CPython library reference index:
  - `docs/python_documentation/python-3.12-docs-text/library/index.txt`
- CPython 3.12/3.13/3.14 union baseline source:
  - `tools/stdlib_module_union.py`

## Canonical Stdlib Surface Files
- Primary stdlib compatibility matrix:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
- Intrinsic backing tracker (policy/gates):
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_backing.md`
- Generated intrinsic audit:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
- Generated asyncio API coverage:
  - `docs/spec/areas/compat/surfaces/stdlib/asyncio_surface.generated.md`
- Stdlib union baseline and gate contract:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md`
- Generated CPython availability matrix:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md`

## Required Dimensions
For stdlib surfaces, coverage rows should include or clearly document:
- `py312`, `py313`, `py314` behavior shape.
- `native`, `wasm_wasi`, `wasm_browser` status.
- OS-specific status where relevant (`linux`, `macos`, `windows`).
- Intrinsic status and differential evidence.

## Update Commands
- `python3 tools/check_stdlib_intrinsics.py --update-doc`
- `python3 tools/gen_stdlib_module_union.py`
- `python3 tools/gen_compat_platform_availability.py --write`
