# Changelog

## [Unreleased] - 2026-04-22

### Added
- **WebGPU Conv2d kernel** (`webgpu-engine.js`): Direct convolution compute shader (`molt_conv2d`) with 16x16 workgroup, fma()-optimized inner loop, zero-padding support. Conv2d is 60% of PaddleOCR compute -- now GPU-accelerated via `conv2d()` and `conv2dGPU()` methods.
- **Node.js WASM inference harness** (`tests/e2e/run_paddleocr_wasm.js`): Loads PaddleOCR WASM, discovers ONNX weights, measures instantiation time (11.0 ms), and enumerates exported OCR functions.
- **WASM SHA256 checksums**: `wasm/molt_runtime.wasm.sha256` and `wasm/molt_runtime_reloc.wasm.sha256` for deploy integrity verification.

### Performance
- WASM instantiation: 11.0 ms (Node.js, Apple Silicon) for 10.8 MB PaddleOCR binary
- Conv+Activation fusion: 62 nodes fused in ONNX graph optimization
- Chinese OCR parity verified (你好世界 round-trip correct)

### Deployed
- Falcon-OCR Worker redeployed (version ee5fadc6) -- all endpoints verified (/test, /test/paddle, /dashboard, /health)
- freeinvoicemaker.app verified live (HTTP 200)

## [Unreleased] - 2026-04-14

### Added
- **GPU inference proxy** (`gpu-proxy.js`): External GPU service forwarding for bfloat16-quality Falcon-OCR inference. Supports HuggingFace Inference Endpoints, Replicate, RunPod, Modal, and Fly.io. Wired into Worker as `X-Use-Backend: gpu` route.
- **ScanButton UX phases**: Extended `InitProgressPhase` with `inferring`, `decoding`, and `done` phases. Added `OcrProgress` interface for unified lifecycle progress reporting. `MoltOcrBackend.recognize()` now emits progress callbacks during inference.
- **Load test harness** (`tests/e2e/test_load.sh`): Concurrent load testing for /health, /invoice/fill, and /ocr GPU proxy endpoints with latency statistics.
- GPU inference status included in /health endpoint response

### Changed
- WASM binary analysis documented: 10.1 MB total (8.2 MB code / 9,934 functions, 1.8 MB data / 1,358 segments)
- Production status docs updated with GPU proxy status, WASM analysis, and load test results

## [Unreleased] - 2026-04-21

### Performance
- **matmul_f32_tiled**: K-loop unrolled by 4 with precomputed row pointers
- Rust SIMD release profile restored to `opt-level = "z"` for the core parity
  artifact.

### Changed
- Rust-only `matmul_f32_fast` removed from the core SIMD artifact until it has
  cross-backend parity coverage and a separate size/performance lane.
- Production status docs updated with SIMD benchmark numbers, CF services inventory, differential test results

## [Unreleased] - 2026-04-20

### Added
- Falcon-OCR WASM compilation and execution (13 MB, 3.9 MB gzipped)
- 26 tinygrad-conformant GPU primitives (molt-gpu crate)
- 7 shader renderers (MSL, WGSL, GLSL, CUDA, HIP, OpenCL, MIL)
- Workers AI OCR with retry logic and model fallback
- x402 payment integration ($0.001/request USDC)
- Template-from-scan feature
- NL invoice filling
- Tiered KV cache with H2O scoring
- Browser WASM loader with IndexedDB caching
- INT4 quantized model (129 MB, 5 shards)

### Fixed
- SCCP dead block elimination (SSA dominance violation)
- WASM import alias resolution (frontend)
- WASM linker table ref preservation (null function traps)
- Turnstile iPad Safari login (explicit render mode)

### Performance
- Fused matmul: 10.9x speedup
- SIMD everywhere (WASM, Rust, all shaders)
- WASM binary: 44 MB -> 13 MB (strip + optimize)
- 7,359 dead exports eliminated

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
