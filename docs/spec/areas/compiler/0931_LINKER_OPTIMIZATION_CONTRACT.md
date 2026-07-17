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
- use section garbage collection where supported;
- do not enable native identical-code folding while the runtime stores function
  addresses as semantic identities for async poll functions and function/code
  metadata keys;
- keep extension modules able to resolve host-provided Molt symbols at load
  time instead of forcing fake definitions into extension objects.

### WASM Linking

WASM link commands must:

- rely on `wasm-ld` section garbage collection where possible; lld's
  WebAssembly port defaults to `--gc-sections` for size-oriented linking;
- treat generated runtime callable names as a catalog, not a root set: builtins,
  intrinsics, GPU entrypoints, and C/API shims are imported only when the app
  reachability plan observes them or a generated runtime structure, such as the
  poll table, owns their slot;
- prefer `--export-if-defined` for optional runtime exports so missing optional
  symbols do not fail the link but required exports are still explicitly
  enumerated;
- avoid broad `--export-all` except for debug-only diagnostics because it
  expands the public ABI and defeats tree shaking;
- preserve `molt_table_init`, exception-pending exports, table refs, memory,
  and host-call exports required by runners;
- use post-link table-ref materialization only after validating the output with
  runtime tests that exercise indirect calls.
- derive WebAssembly feature flags from the selected target feature profile
  (`wasm-mvp`, `wasm-refs`, `wasm-gc`) defined in
  `docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md`; linkers and
  optimizers may not infer GC/reference support from Cargo profile names,
  browser-family assumptions, or `wasm-opt` availability.

### Linker Source and Loader Closure

The link fingerprint covers the complete local Python source closure rooted at
`tools/wasm_link.py`, not a hand-maintained tool list. Static `import`/`from`
syntax, package initializers, namespace portions, and statically provable
`importlib.import_module`/`__import__` calls resolve through
`molt.cli.python_import_resolution`; `molt.cli.python_source_closure` owns the
transitive walk and an atomic performance cache. A non-literal dynamic edge is
either declared in its checked manifest or fails closed.

Browser and Node loader assets are a separate generated graph rooted in
`src/molt/browser_asset_graph.toml`. Every JavaScript asset has an explicit
browser/node/shared role, source type, and content hash. Packaging, deployment,
proof scopes, and link fingerprints consume `wasm_loader_asset_closure`; a new
loader edge therefore changes one generated authority and every consumer.

### Post-Link Optimization

Binaryen/`wasm-opt` and future post-link optimizers may be used only behind
reproducible before/after checks:

- the optimized binary must validate;
- exported symbol sets required by Molt runners must match the contract;
- linked Falcon/Tinygrad smoke tests must still pass;
- size and cold-start improvements must be recorded in `bench/results/` or
  `logs/` with exact command lines.

The portable `wasm-mvp` baseline keeps Binaryen GC disabled so `wasm-opt` cannot
rewrap flattened function types into GC-only recursive type groups. A `wasm-gc`
artifact may enable Binaryen GC only when the target contract proves runner,
browser, and deployment-host support, and only with the same export-contract,
size, cold-start, allocation-count, host-call-count, and throughput evidence as
the non-GC artifact it replaces.

### Disallowed Shortcuts

- No linker flags that mask undefined required symbols.
- No removal of runtime exports to make a size target pass.
- No test-specific export allowlists.
- No host-CPython fallback to compensate for missing linked behavior.
- No treating generic `wasm-opt -O*` output as accepted without end-to-end
  Molt runner verification.
- No enabling WasmGC or reference-types globally to work around missing lowering,
  package custody, C/API, buffer, or import/link closure. Feature profiles must
  expose real target capability and real IR/runtime facts.

## Current High-Value Work

1. Add a measured `wasm-opt -Oz --converge` lane for release artifacts with
   export-contract verification.
2. Add native link command snapshot tests for Darwin GPU framework propagation,
   extension-module dynamic lookup, and Cargo-emitted native deps.
3. Add size dashboards for linked Falcon artifacts: raw size, gzip size,
   function count, data segment count, and export count.
4. Add regression tests for runtime table initialization and signature
   normalization before enabling any more aggressive ICF/export pruning.
5. Add a `wasm-gc` feature-profile probe lane that validates Binaryen GC flags,
   runner/browser support, export preservation, and measured deltas against the
   matching `wasm-mvp` artifact before any WasmGC lowering lands.
