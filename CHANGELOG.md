# Changelog

## [Unreleased] - 2026-03-20

### WASM Optimization (Complete Sections 1-5 of WASM Optimization Plan)

#### Codegen Quality
- **br_table dispatch**: O(1) state machine dispatch for generators/coroutines with 5+ states (2-5x faster resume)
- **Box/unbox elimination**: Skip NaN-box tag checks for statically-typed integer ops (10-20% faster)
- **Dead local elimination**: Shared __dead_sink variable reduces WASM local count (2-5% smaller)
- **Local variable coalescing**: Greedy linear-scan reuse of __tmp/__v temporaries (5-15% smaller)
- **Constant folding**: Compile-time evaluation of fast_int arithmetic on known constants (3-5% smaller)
- **Instruction combining**: Const propagation through box/unbox, 5->2 insns for known-const unbox (3-8% faster)
- **local.tee**: 37 eliminated redundant LocalGet instructions across arithmetic hot paths
- **Constant caching**: Pre-materialized INT_SHIFT/INT_MIN/INT_MAX in function locals

#### WASM Proposals
- **Multi-value returns**: Internal functions returning 2+ values use WASM multi-value ABI (eliminates tuple allocation)
- **Tail calls**: return_call for tail-position calls in non-stateful functions
- **Native exception handling**: try_table/catch/throw (gated by MOLT_WASM_NATIVE_EH=1)
- **Bulk memory**: memory.fill for generator control blocks, memory_copy intrinsic
- **SIMD support**: Full SIMD instruction handling in WASI stub rewriter

#### Binary Size & Startup
- **Full LTO**: wasm-release profile with lto=true, codegen-units=1
- **wasm-opt integration**: Oz/O3 pass pipelines (DCE, code folding, local CSE, inlining)
- **--precompile**: Generate .cwasm via wasmtime compile (10-50x faster startup)
- **--wasm-profile pure**: Compile-time IO/async/time import stripping (30-50% smaller for pure compute)

### WASM Freestanding Target
- **--target wasm-freestanding**: Post-link WASI import stubbing for zero-WASI binaries
- **Strict allowlist**: --allow-undefined-file with 26 WASI + 14 trampoline symbols
- **wasm-validate integration**: Optional post-build validation

### Virtual Filesystem (VFS)
- **Mount-oriented VFS**: /bundle (read-only), /tmp (ephemeral r/w), /dev (stdio), /state (stub)
- **BundleFs**: In-memory from tar archive, symlink rejection, deterministic iteration
- **TmpFs**: RwLock-based, quota enforcement, rename support, between-request clear()
- **DevFs**: stdin/stdout/stderr pseudo-devices with 16 MB buffer cap
- **Capability system**: fs.bundle.read, fs.tmp.read/write, fs.state.read/write
- **open() routing**: VFS dispatch for WASM targets with fallback to std::fs
- **Module import**: VFS-aware is_file/read helpers, /bundle in sys.path
- **Snapshot**: molt.snapshot.json with SHA-256 integrity hash

### Packaging & Deployment
- **--bundle**: Tar archive packaging with manifest
- **--profile**: cloudflare/browser/wasi/fastly presets
- **Worker template**: Cloudflare Worker entry point generator

### Testing
- 34 Rust VFS unit tests
- 32+ Python tests (freestanding, stubbing, bundle, worker, snapshot, size tracking)
- 10 Rust WASM backend tests (br_table, dead local)
