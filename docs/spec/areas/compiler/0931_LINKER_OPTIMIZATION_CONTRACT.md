# Linker Optimization Contract

**Status:** Active contract
**Owner:** compiler/tooling

## Provenance

Molt linker work is grounded in these primary sources:

- LLVM lld WebAssembly port documentation:
  <https://lld.llvm.org/WebAssembly.html>
- LLVM lld design documentation for ELF/COFF/Wasm linkers:
  <https://lld.llvm.org/NewLLD.html>
- Binaryen and `wasm-opt` optimizer documentation:
  <https://github.com/WebAssembly/binaryen>
- `wasm-opt` option model:
  <https://docs.rs/wasm-opt/latest/wasm_opt/struct.OptimizationOptions.html>
- mold linker project documentation:
  <https://github.com/rui314/mold>
- BOLT binary optimizer paper:
  Maksim Panchenko et al., **"BOLT: A Practical Binary Optimizer for Data
  Centers and Beyond"**, arXiv:1807.06735:
  <https://arxiv.org/abs/1807.06735>

## Non-Negotiable Linker Rules

Correctness wins over size or speed. Linker optimization must never hide
missing symbols, silently alter ABI boundaries, or remove runtime exports that
are required by host runners, browser hosts, split-runtime workers, extension
modules, or dynamic intrinsic resolution.

### Native Linking

Native link commands must:

- include runtime static libraries in a way that preserves circular references
  and exported runtime symbols;
- include Cargo-emitted native library dependencies such as `-l*`,
  `-L*`, and Darwin framework flags;
- include Darwin runtime frameworks required by enabled GPU backends;
- use section garbage collection and safe identical-code folding only when the
  target platform/linker supports the flag and tests prove no symbol identity
  contract is broken;
- keep extension modules able to resolve host-provided Molt symbols at load
  time instead of forcing fake definitions into extension objects.

### WASM Linking

WASM link commands must:

- rely on `wasm-ld` section garbage collection where possible; lld's
  WebAssembly port defaults to `--gc-sections` for size-oriented linking;
- prefer `--export-if-defined` for optional runtime exports so missing optional
  symbols do not fail the link but required exports are still explicitly
  enumerated;
- avoid broad `--export-all` except for debug-only diagnostics because it
  expands the public ABI and defeats tree shaking;
- preserve `molt_table_init`, exception-pending exports, table refs, memory,
  and host-call exports required by runners;
- use post-link table-ref materialization only after validating the output with
  runtime tests that exercise indirect calls.

### Post-Link Optimization

Binaryen/`wasm-opt` and future post-link optimizers may be used only behind
reproducible before/after checks:

- the optimized binary must validate;
- exported symbol sets required by Molt runners must match the contract;
- linked Falcon/Tinygrad smoke tests must still pass;
- size and cold-start improvements must be recorded in `bench/results/` or
  `logs/` with exact command lines.

### Disallowed Shortcuts

- No linker flags that mask undefined required symbols.
- No removal of runtime exports to make a size target pass.
- No test-specific export allowlists.
- No host-CPython fallback to compensate for missing linked behavior.
- No treating generic `wasm-opt -O*` output as accepted without end-to-end
  Molt runner verification.

## Current High-Value Work

1. Add a measured `wasm-opt -Oz --converge` lane for release artifacts with
   export-contract verification.
2. Add native link command snapshot tests for Darwin GPU framework propagation,
   extension-module dynamic lookup, and Cargo-emitted native deps.
3. Add size dashboards for linked Falcon artifacts: raw size, gzip size,
   function count, data segment count, and export count.
4. Add regression tests for runtime table initialization and signature
   normalization before enabling any more aggressive ICF/export pruning.
