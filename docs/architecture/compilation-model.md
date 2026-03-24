# Molt Compilation Model — Production Architecture

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

### First Run (cold cache, ~60s)
```
molt build app.py
  1. Compile molt-runtime-core → libmolt_core.a        (Layer 1-3, cached)
  2. Compile user code IR → user.o                     (~2s)
  3. Link: user.o + libmolt_core.a → app               (~1s)
```

### Subsequent Runs (warm cache, ~3s)
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
| `molt-runtime-core` | `libpython3.*.a` core | Object model, GC, exceptions, module system, call dispatch, builtin types+functions | ~55K lines |
| `molt-runtime-math` | `_math` module | math.floor, math.sqrt, math.sin, etc. | ~2K lines |
| `molt-runtime-net` | `_socket`, `_ssl` | socket, http, websocket, TLS | ~15K lines |
| `molt-runtime-asyncio` | `_asyncio` | Event loop, tasks, futures, streams | ~30K lines |
| `molt-runtime-serial` | `_json`, `_csv`, `_struct` | Serialization/deserialization | ~5K lines |
| `molt-runtime-crypto` | `_hashlib`, `_hmac` | Hashing, HMAC, PBKDF2 | ~3K lines |
| `molt-runtime-compression` | `_bz2`, `_lzma`, `zlib` | Compression/decompression | ~2K lines |
| `molt-runtime-tk` | `_tkinter` | GUI bindings | ~17K lines |

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
│ ~50KB        │  │ ~1-2MB        │
└──────────────┘  └───────────────┘
```

- `runtime.wasm`: Pre-compiled, tree-shaken, CDN-cacheable
- `app.wasm`: Tiny user code module, imports from runtime
- Worker.js: Instantiates both, stitches imports

## Native Backend Changes Required

### 1. Pre-compilation Infrastructure

The `molt-runtime-core` crate compiles to `libmolt_core.a` via:
```
cargo build -p molt-runtime-core --release
```

This `.a` file is cached at `~/.molt/cache/lib/libmolt_core.a` (or equivalent).
The fingerprint includes: Rust toolchain version, target triple, feature flags.

### 2. Backend Split: User Code Only

The native backend (`molt-backend`) should ONLY compile user code IR to `.o`.
It should NOT re-compile the runtime. Instead:

```rust
// Current (broken): compile ALL 1000+ functions
backend.compile(ir_with_everything)

// Fixed: compile ONLY user functions
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
- Move builtins/math.rs → molt-runtime-math/src/lib.rs
- Move builtins/json.rs → molt-runtime-serial/src/lib.rs
- Each crate produces its own .a file
- Linker only includes crates the user imports

### Phase 4: WASM module splitting
- Core runtime → runtime.wasm (pre-compiled, CDN-cached)
- User code → app.wasm (tiny, per-deploy)
- Worker.js stitches them via WebAssembly imports
```
