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
- preserve exception-pending exports, memory, and host-call exports required by
  runners while rejecting legacy table writers and numeric callable aliases;
- publish final active element segments and their generated callable-table
  attestation only after all link and optimization rewrites, then validate the
  attestation against the final types, functions, tables, dispatches, and
  mutations before exposing the artifact.
- derive WebAssembly feature flags from the selected target feature profile
  (`wasm-mvp`, `wasm-refs`, `wasm-gc`) defined in
  `docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md`; linkers and
  optimizers may not infer GC/reference support from Cargo profile names,
  browser-family assumptions, or `wasm-opt` availability.

### WASM link-facts authority

`runtime/molt-wasm-facts` owns the one validated, single-pass scan of
WebAssembly artifact bytes. Its schema-v7 result covers functions, operators, references,
tables, elements, mutations, callable-table attestations, custom sections,
linking symbols (including original symbol ordinals), function names, GOT binding
evidence, and canonical import/export types. The Rust model and scanner
in `runtime/molt-wasm-facts/src/{model,scan}.rs` are the schema and parsing
authorities; schema changes must update the Rust producer and its exact consumer
contract together.

`tools/wasm_link_fact_provider.py` owns the only Python boundary. It validates
the exact schema-v7 JSON field set, freezes the decoded data, and projects
linker-facing `WasmImportFact`, `WasmExportFact`, and `WasmLinkFacts` values.
Python may perform typed lookups and decode compact tuple rows from that result,
but it must not parse the whole WebAssembly artifact or reconstruct import,
export, liveness, linking-symbol, or GOT facts. The former Python full-module
parser lane is deleted. The exhaustive call graph and intermediate root sets
remain scanner-internal.

The complete generated runtime function registry is checked against all runtime
function exports before essential roots are subtracted for tree shaking.
CPython-ABI data edges use the generated symbol kind and require an immutable,
nonshared i32 address global. GOT facts record optional initial addresses,
binding flags, and definedness for every relocation; exact binding and global
shape are required only when the bridge selects a CPython-ABI data symbol.
Unrelated weak, local, or undefined PIC symbols do not fail a scan. Malformed
relocation extents and out-of-range symbol/global indices still fail closed.

Final callable layout counts are local to the artifact being published. App
and monolithic publication derive the app entry count from their final active
elements, including entries added by native linking. Shared-runtime publication
retains the common runtime/app boundary and required fixed prefix, validates its
own runtime entries, and publishes an app entry count of zero. An application's
entry count must never enter the shared runtime bytes or its CDN identity. The
app's final layout and attestation remain independently validated; runtime
cacheability does not erase application metadata.

Link-time `linking` and `reloc.*` sections are consumed before stable app identity
markers are published, then removed before any optimizer or index-changing
transform. Function import removal also drops the stale `name` section; other
debug sections follow the selected debug policy. Python decoders retained for
section rewriting are codecs only; import/export/linking decisions consume the
same Rust facts provider used for validation and publication.

The scan is `O(module bytes + operators + reference edges + functions)`. Its
memory is `O(functions + reference edges + active elements + table facts)`;
Python consumers allocate only the typed projections they use. One
content-addressed provider is bound to a hash-sealed scanner snapshot for each
link invocation and reuses facts for identical bytes. The phase timing artifact
records scanner hash time, child scan time, calls, content-cache hits, input
bytes, and response characters. Section-walk and reserialization counters cover
the remaining Python byte-rewrite utilities; retired Python full-parser counters
are not part of the current performance contract.

Static archives use `molt.cli.static_archive_identity` for GNU/BSD framing,
ordered member identity, and reads through one stable source handle. The WASM
projection adds object bounds and target-format checks; it owns no archive
parser. Before linking, raw objects and whole archives create required provider
and data-address obligations. Lazy archives contribute candidates only. Exact
linker extraction evidence admits both the monolithic and split-app roles before
publication; dormant member names cannot force runtime exports or missing
provider/address failures.

### Linker Source and Loader Closure

The link fingerprint covers the complete local Python source closure rooted at
`tools/wasm_link.py`, not a hand-maintained tool list. Static `import`/`from`
syntax, package initializers, namespace portions, and statically provable
`importlib.import_module`/`__import__` calls resolve through
`molt.cli.python_import_resolution`; `molt.cli.python_source_closure` owns the
transitive walk and an atomic performance cache. A non-literal dynamic edge is
either declared in its checked manifest or fails closed.

Import discovery requests the binding fixpoint only when the canonical binding
authority finds a possible importer identity origin. This is an absence proof,
not spelling-based callee specialization: positive sources retain full alias,
mutation and deferred-body analysis, and relative statements retain module
context analysis. Live module resolution and exact source-byte identities remain
mandatory on cache hits; no whole-graph freshness assumption replaces them.

The walk retains captured source bytes for identity, while each analysis owns
its AST only for the duration of that projection. Aliased module contexts reuse
the captured bytes and independently derive their import facts. Persisted
analysis rows pass strict decoding and dynamic-contract validation before reuse;
valid hits keep their serialized storage, and misses replace only their selected
variant. Pruning remains independent of misses, and a failed traversal publishes
no partial cache. This compiler-side cache adds no checks to emitted binaries.

Private-name mangling is owned by `molt.python_private_names`, shared by
custody, discovery, binding analysis and lowering. Importing this primitive
does not initialize compiler analysis. Capture-source and proof-authority
closures bind the shared module directly; no relocated-module shim remains.

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

The optimizer publication sidecar uses the exact
`molt.wasm-optimizer-attestation.v4` schema. It binds the executable digest,
verified Binaryen version, optimization level and flags, debug preservation,
canonical pipeline digest, optimizer input/output hashes, and final published
output hash. Both cached and fresh generations admit that sidecar against the
requested tool and debug policy. Timing, host paths, and cache-hit telemetry
remain outside the reproducible publication identity. The sidecar is published
with the linked/split output family and its source-bound final link receipt.

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

1. Attribute child CPU and whole-process-tree RSS independently for `wasm-ld`,
   facts scanning, Binaryen, validation, and atomic publication on real release
   artifacts; preserve the per-phase evidence in the linker benchmark report.
2. Execute and retain the native/WASM linker matrix on Windows, Linux, and macOS
   for x86_64/aarch64 rather than treating cross-target plan construction as
   execution evidence.
3. Add size dashboards for linked representative artifacts: raw size, gzip size,
   function count, data segment count, and export count.
4. Add regression tests for runtime table initialization and signature
   normalization before enabling any more aggressive ICF/export pruning.
5. Add a `wasm-gc` feature-profile probe lane that validates Binaryen GC flags,
   runner/browser support, export preservation, and measured deltas against the
   matching `wasm-mvp` artifact before any WasmGC lowering lands.


### Optimizer evidence by publication role

A split link carries two independent optimizer attestations. The `optimizer`
output role binds the linked companion's own execution to its final bytes;
`app_optimizer` binds the split app's execution to the final app bytes. The size
attestation embeds the app attestation. Each sidecar is published atomically with
its artifact and is removed when optimization is disabled. A sidecar for one
artifact must never attest a different artifact's digest or optimizer execution.
The scanner executable is captured once per invocation and its expected SHA-256
is checked before snapshot creation or child execution.
