# Molt Compilation Model — Production Architecture

Status: target architecture / partially landed (refreshed 2026-06-20).
The live codebase, `Cargo.toml`, and guarded `cargo metadata` are authoritative;
this document describes the production direction and calls out the current gaps.
See [parallel_build_architecture.md](../design/parallel_build_architecture.md)
for the crate-extraction and incremental-build routing plan.

## Live State Snapshot (2026-06-27)

- Runtime leaf crates already exist and are wired from
  `runtime/molt-runtime/Cargo.toml`, including core, collections, math, text,
  serial, crypto, compression, net, asyncio, regex, path, itertools, difflib,
  logging, http, xml, ipaddress, zoneinfo, stringprep, and tk. `molt-runtime-protobuf`
  exists as a workspace package, but is not yet a `molt-runtime` facade
  dependency.
- `stdlib_stringprep` is leaf-owned: the old in-facade `builtins/stringprep.rs`
  fallback is deleted, `molt_stringprep_*` resolver ownership is generated into
  `molt-runtime-stringprep/src/intrinsics_generated.rs`, and the `molt-runtime`
  facade delegates through that leaf sub-registry behind `stdlib_stringprep`.
  Feature-on/feature-off `molt-runtime` checks prove the facade no longer
  carries a duplicate stringprep authority.
- `molt-runtime-text` now owns the always-on codec identity registry, generated
  codec alias table, and generated single-byte charmap tables as well as the
  feature-gated `html` and `unicodedata` implementations. The small
  `codec_registry` module is a non-optional runtime dependency and is the
  canonical descriptor source for direct codec labels, Python `encodings` module
  names, ordinal limits, and text-I/O classes. `tools/gen_codecs.py` filters
  CPython alias reference data through that registry and derives the named
  single-byte table set from the same descriptor source; it generates alias rows
  plus encode/decode maps from the repo-pinned
  `src/molt/stdlib/encodings/*.py` modules, so `molt-runtime` remains only the
  caller/error adapter for codec execution. The `codec-tables` dev gate watches
  the generator, generated tables, Python encoding modules, and that runtime
  consumer so the old facade-owned charmap shim cannot reappear silently. The
  heavier html/unicodedata modules
  remain gated by `stdlib_text`; `molt_html_*` and
  `molt_unicodedata_*` resolver arms are gated by `stdlib_text`, and
  feature-on/feature-off runtime checks prove the facade no longer carries a
  duplicate text authority for those modules.
- `stdlib_zoneinfo` is leaf-owned: the old in-facade `builtins/zoneinfo.rs`
  fallback is deleted, `molt_zoneinfo_*` resolver arms are gated by
  `stdlib_zoneinfo`, and feature-on/feature-off runtime checks prove the facade
  no longer carries a duplicate zoneinfo authority.
- `molt-runtime` is not yet a pure facade. It still owns substantial runtime
  implementation, so the precompiled-per-import library model below is the
  target architecture rather than a completed current guarantee.
- `release-fast` already uses thin LTO/high codegen-unit parallelism for
  compiler iteration; shipped output profiles retain whole-program optimization
  where runtime performance and size require it.
- The runtime intrinsic resolver source is split by generated category:
  `runtime/molt-runtime/src/intrinsics/generated.rs` remains the canonical
  `INTRINSICS` manifest table, and
  `runtime/molt-runtime/src/intrinsics/generated_resolvers/` owns category
  resolver modules. `stringprep` is the first generated per-leaf-crate
  sub-registry; the remaining registry work is to lift the other generated
  categories the same way as runtime facade extraction proceeds.
- `molt-backend-wasm` owns WASM instruction projection and the `wasm-encoder`
  dependency; `molt-tir` remains the backend-neutral TIR/LIR/representation
  authority and no longer carries backend-specific WASM codegen.
- Backend source identity for rebuild freshness and module/function cache keys
  is derived from `runtime/molt-backend/Cargo.toml` feature edges plus local
  path-dependency closure. A WASM edit tracks `molt-backend-wasm`, native/LLVM
  edits track `molt-backend-native`, and shared `molt-ir`/`molt-tir`/
  `molt-passes`/`molt-codegen-abi` edits invalidate the backend lanes that
  actually depend on them.
- Remaining structural work: finish runtime facade composition, finish per-crate
  intrinsic registries, isolate native backend codegen into its own crate, and
  preserve deterministic cache/build-state custody across concurrent agents.

## Overview

Molt follows CPython's layered architecture, adapted for AOT compilation:

```
┌─────────────────────────────────┐
│  Layer 6: User Code             │  Compiled per-invocation (~2s)
│  (app.py, game.py, etc.)        │  Output: user.o
├─────────────────────────────────┤
│  Layer 5: Stdlib (Python)       │  Compiled from src/molt/stdlib/
│  (collections, functools, etc.) │  Output: stdlib IR → part of user.o
├─────────────────────────────────┤
│  Layer 4: Stdlib (Native)       │  Pre-compiled, cached .a library
│  (math, json, os, socket, etc.) │  Only linked if user imports them
├─────────────────────────────────┤
│  Layer 3: Builtin Functions     │  Part of core runtime .a
│  (print, len, range, etc.)      │  Always linked
├─────────────────────────────────┤
│  Layer 2: Builtin Types         │  Part of core runtime .a
│  (int, str, list, dict, etc.)   │  Always linked
├─────────────────────────────────┤
│  Layer 1: Core Runtime          │  Pre-compiled, cached .a library
│  (object model, GC, exceptions) │  Always linked
└─────────────────────────────────┘
```

## Compilation Flow

### Target First Run (cold cache)
```
molt build app.py
  1. Compile molt-runtime-core → libmolt_core.a        (Layer 1-3, cached)
  2. Compile user code IR → user.o                     (~2s)
  3. Link: user.o + libmolt_core.a → app               (~1s)
```

### Target Subsequent Runs (warm cache)
```
molt build app.py
  1. [cached] libmolt_core.a exists, skip
  2. Compile user code IR → user.o                     (~2s)
  3. Link: user.o + libmolt_core.a → app               (~1s)
```

### With Stdlib Imports (e.g., `import math`)
```
molt build app.py  # where app.py uses `import math`
  1. [cached] libmolt_core.a
  2. [cached] libmolt_math.a (or compiled on first use)
  3. Compile user code IR → user.o
  4. Link: user.o + libmolt_core.a + libmolt_math.a → app
```

## Crate Structure

### Pre-compiled (cached as .a files)

| Crate | CPython Equivalent | Contents | Size |
|-------|-------------------|----------|------|
| `molt-runtime-core` | `libpython3.*.a` core | Shared runtime core; not yet the complete object-model authority | live leaf |
| `molt-runtime-collections` | list/dict/set/tuple clusters | Container helpers and collection-facing intrinsics | live leaf |
| `molt-runtime-math` | `_math` module | math intrinsics and numeric helpers | live leaf |
| `molt-runtime-text` | str/bytes/codecs clusters | Text and codec helpers | live leaf |
| `molt-runtime-serial` | `_json`, `_csv`, `_struct` target area | Serialization/deserialization helpers | live leaf |
| `molt-runtime-crypto` | `_hashlib`, `_hmac`, `secrets` target area | Hashing, HMAC, PBKDF2, scrypt, and secrets helpers plus the leaf-owned crypto intrinsic resolver | live leaf |
| `molt-runtime-compression` | `_bz2`, `_lzma`, `zlib` target area | Compression/decompression helpers | live leaf |
| `molt-runtime-net` / `molt-runtime-http` | `_socket`, `_ssl`, HTTP target area | Network and HTTP helpers | live leaf |
| `molt-runtime-asyncio` | `_asyncio` target area | Event loop, tasks, futures, streams | live leaf |
| `molt-runtime-regex`, `-path`, `-xml`, `-ipaddress`, `-zoneinfo`, `-stringprep`, `-tk` | stdlib clusters | Feature-cohesive runtime leaves | live leaves |
| `molt-runtime-protobuf` | protobuf target area | Workspace leaf candidate; not currently a `molt-runtime` facade dependency | workspace package |

### Compiled Per-Invocation

| Component | Contents | Typical Size |
|-----------|----------|-------------|
| User code | app.py → user.o | 1-50KB |
| Stdlib glue | src/molt/stdlib/*.py → IR → included in user.o | 10-100KB |

## WASM Variant

For WASM, the same layered approach applies:

```
┌──────────────┐  ┌───────────────┐
│ app.wasm     │  │ runtime.wasm  │  Pre-compiled, CDN-cached
│ (user code)  │──│ (Layers 1-4)  │
│ tree-shaken  │  │ tree-shaken   │
└──────────────┘  └───────────────┘
```

- `runtime.wasm`: Pre-compiled, tree-shaken, CDN-cacheable
- `app.wasm`: Tiny user code module, imports from runtime
- Worker.js: Instantiates both, stitches imports

## Native Backend Target Changes

### 1. Pre-compilation Infrastructure

The target model compiles runtime leaves such as `molt-runtime-core` to
linkable libraries via:
```
cargo build -p molt-runtime-core --release
```

This `.a` file is cached under the canonical artifact/cache root
(`MOLT_CACHE`/`MOLT_EXT_ROOT`, not an ambient home-directory cache). The
fingerprint includes: Rust toolchain version, target triple, feature flags.

### 2. Backend Split: User Code Only

The native backend target is to compile only user code IR to `.o` and link
runtime leaves from cached libraries instead of re-compiling runtime IR:

```rust
// Current target gap: compiling runtime/user code through one large plan.
backend.compile(ir_with_everything)

// Target: compile ONLY user functions.
let user_ir = ir.functions.retain(|f| !f.is_stdlib);
backend.compile(user_ir)  // ~10 functions, ~2s
```

### 3. Linker Integration

The CLI's link step combines:
```
ld -o app user.o -lmolt_core [-lmolt_math] [-lmolt_net] ...
```

Only stdlib crates that the user imports are linked. The linker's
`--gc-sections` / `-dead_strip` removes unused functions.

## categories.toml Mapping

```toml
[core]           → molt-runtime-core (always linked)
[builtin]        → molt-runtime-core (always linked)
[internal]       → molt-runtime-core (always linked)
[stdlib.math]    → molt-runtime-math (linked if `import math`)
[stdlib.json]    → molt-runtime-serial (linked if `import json`)
[stdlib.socket]  → molt-runtime-net (linked if `import socket`)
[stdlib.asyncio] → molt-runtime-asyncio (linked if `import asyncio`)
# etc.
```

## Implementation Phases

### Phase 1: Separate user IR from stdlib IR (backend)
- Tag each FunctionIR with `is_stdlib: bool`
- Backend only compiles functions where `is_stdlib == false`
- Stdlib functions are resolved at link time from the pre-compiled .a

### Phase 2: Pre-compile core runtime
- `cargo build -p molt-runtime` already produces libmolt_runtime.a
- Cache it with a fingerprint
- CLI links user.o against it

### Phase 3: Split stdlib into crates
- Continue moving runtime authority into existing leaf crates such as
  `molt-runtime-math`, `molt-runtime-crypto`, `molt-runtime-serial`, and `molt-runtime-text`
- Each crate produces its own .a file
- Linker only includes crates the user imports

### Phase 4: WASM module splitting
- Core runtime → runtime.wasm (pre-compiled, CDN-cached)
- User code → app.wasm (tiny, per-deploy)
- Worker.js stitches them via WebAssembly imports
```

## GPU Primitive Stack

The `molt-gpu` crate (`runtime/molt-gpu/`) provides a tinygrad-conformant GPU compute subsystem. It implements all of deep learning with 26 compute primitives, a zero-copy ShapeTracker view system, lazy evaluation DAG, kernel fusion, and multi-backend rendering (Metal, WebGPU/WGSL, CUDA, HIP).

For user code that imports `tinygrad`:
```
molt build app.py  # where app.py uses `from tinygrad import Tensor`
  1. [cached] libmolt_core.a
  2. [cached] libmolt_gpu.a (molt-gpu crate)
  3. Compile user code IR → user.o
  4. At runtime: Tensor ops build LazyOp DAG → schedule → fuse → render → execute on GPU
```

The GPU stack operates at a different level than the AOT compiler: tensor operations construct a computation DAG at runtime, which is then scheduled, fused, rendered to shader source, and dispatched to the GPU. The AOT compiler compiles the Python control flow; the GPU stack handles the data flow.

See [gpu-primitive-stack.md](gpu-primitive-stack.md) for the full architecture.
