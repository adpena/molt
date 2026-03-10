# Stdlib Intrinsics Loading

Status: Active

## Scope
This spec defines how stdlib modules load Molt intrinsics and the minimum
requirements for correctness, performance, and determinism. It is the canonical
checklist for new or modified stdlib shims.

## Loader Contract
- Resolve intrinsics through `src/molt/stdlib/_intrinsics.py` only.
- Resolution order is module `globals()` first, then `builtins._molt_intrinsics`.
- Do not create alternative registries, hidden loaders, or import-time side
  effects that bypass the canonical loader.

## Checklist
- Import `load_intrinsic` and `require_intrinsic` from `_intrinsics`.
- Required functionality must use `require_intrinsic` or raise explicit
  `RuntimeError`/`ImportError` when missing.
- Optional functionality must be explicit, capability-gated, and never fall
  back to host Python.
- Keep Python shims minimal: argument normalization, error mapping, and
  capability gating only.
- Lower hot paths and semantics into Rust intrinsics for performance and
  correctness.
- Register new intrinsics in
  `runtime/molt-runtime/src/intrinsics/manifest.pyi` and regenerate
  `src/molt/_intrinsics.pyi` plus
  `runtime/molt-runtime/src/intrinsics/generated.rs` via
  `python3 tools/gen_intrinsics.py`.

## Lint Gate
- `tools/check_stdlib_intrinsics.py` enforces the loader contract and runs in
  `tools/dev.py lint`.
