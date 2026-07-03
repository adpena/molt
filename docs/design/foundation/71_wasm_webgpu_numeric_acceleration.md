# 71 - WASM, WebGPU, and Numeric Acceleration Plan

Status: research addendum, 2026-07-03

This document turns current WASM, WebGPU, WebNN, BLAS/LAPACK/GSL, and tensor
interchange research into Molt engineering obligations. It refines R4, R7, and
R8 from the live orchestration board. It is not a parallel roadmap and it does
not supersede `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`,
`docs/spec/areas/wasm/0970_BROWSER_NUMERIC_KERNEL_EMBED.md`, or
`docs/architecture/gpu-primitive-stack.md`.

## End State

Molt's performance stack is a capability-gated lowering system, not a set of
profile names:

1. Python semantics lower through shared facts into scalar, vector, tensor, ABI,
   and GPU primitives.
2. Proven-typed scalar and numeric loop paths emit native target instructions.
   Boxed runtime calls remain only for genuinely dynamic paths.
3. Array/tensor data moves through one typed strided storage authority:
   ndim, shape, strides, dtype/format, itemsize, owner/base, offset, release,
   lifetime, and device.
4. Third-party numerical ecosystems are admitted through upstream source,
   extension artifacts, C/API, ABI, buffer, capsule, and object closure custody.
   Molt does not reimplement NumPy, SciPy, pandas, tinygrad, BLAS, LAPACK, or
   GSL semantics in Molt-owned Python.
5. Every optimization claim has an opcode/artifact/profiling gate: generated
   feature manifest, import closure, host-call count, binary size, startup,
   throughput, allocation count, and parity oracle.

## Standards And Features To Track

### Deterministic Core WASM

Use these as near-term lowering targets when the target feature manifest proves
support:

- Multi-value returns for small tuple/error-value pairs and internal ABI
  results.
- Tail calls for tail-position Python control flow and continuation-shaped
  lowering. The WebAssembly tail-call proposal defines explicit `return_call`
  instructions that unwind the current frame before calling the callee.
- Exception handling for Python unwinding paths, replacing host-call exception
  traffic where the target profile supports the current exception model.
- Memory64 for large arrays and host profiles that need 64-bit linear-memory or
  table indexes. Keep 32-bit memories as the default for size and pointer
  compression until a program actually requires 64-bit address space.
- Threads and atomics for native/worker profiles where shared memory and
  cross-origin isolation or host support are present.
- JS Promise Integration for browser host capability calls that are naturally
  async, without rebuilding Molt async semantics as ad hoc JavaScript callbacks.

### SIMD And Vector Lanes

Use deterministic SIMD first:

- `simd128` is the baseline vector lane for WASM numeric kernels. The proposal
  adds a `v128` type with lane interpretations such as `i8x16`, `i16x8`,
  `i32x4`, `i64x2`, `f32x4`, and `f64x2`.
- Relaxed SIMD is a separate opt-in speed tier because it permits
  host-dependent numeric results. It may be used only under an explicit
  non-bit-exact feature gate with oracle budgets, never in the deterministic
  CPython-parity lane.
- Flexible vectors and half precision are future-proofing lanes. Record them in
  the target feature manifest now, but do not rely on them for baseline support
  until engine and toolchain support is proven.
- Wide arithmetic matters for Python integers, overflow checks, hashing, PRNGs,
  crypto, and bignum internals. It should become a backend lowering target when
  available, not a handwritten Python big-int shortcut.

### WasmGC And Reference Types

WasmGC is an object-layout target profile, not a replacement for Molt's current
runtime object authority:

- Use it for closed-layout object classes only after R3/R4 facts can prove
  layout, lifetime, and target support.
- Keep NaN-boxed or runtime-owned dynamic objects for open Python behavior.
- Gate WasmGC by browser/runtime/tooling validation, Binaryen behavior, startup,
  size, host-call count, allocation count, and parity. Unsupported runners fail
  closed or use the non-GC target profile.

### WASI And Component Model

The July 2026 reality is that WASI 0.3.0 has shipped. The plan must move from
"future async" to explicit adoption criteria:

- WIT is the interface description authority for Component Model worlds and
  capability-scoped interfaces.
- WASI 0.3 adds async Component Model primitives (`async func`, `stream<T>`,
  and `future<T>`) and is available in current Wasmtime and jco releases.
- Migration is gated by overhead and closure evidence. If Component Model
  linking widens imports or slows the hot path, keep raw imports for that target
  until the component path is optimized.
- Stream and caller-supplied-buffer work in WASI 0.3 aligns directly with Molt's
  zero-copy buffer and package artifact goals.

## WebGPU And WebNN

WebGPU is the browser GPU compute authority for Molt kernels. WebNN is a
separate optional neural-network graph backend. They should not be conflated.

### WebGPU

WebGPU should serve Molt's tensor/kernel lane:

- `molt.gpu` lowers through the existing lazy DAG, ShapeTracker, fusion, dtype,
  and renderer authority into WGSL compute pipelines.
- WGSL storage buffers, workgroup memory, atomics, f16, subgroup operations, and
  timestamp queries are feature-gated capabilities. Missing features must
  choose a deterministic lower tier or fail closed with a precise diagnostic.
- WebGPU execution must carry the same typed storage identity as native:
  dtype, shape, strides, offset, owner/base, and device lifetime.
- Browser proof must include actual WebGPU execution where available and a
  deterministic injected dispatcher only for CI lanes that cannot expose a GPU.
- Timestamp queries and adapter limits are observability inputs, not semantic
  facts. They can explain performance but must not change correctness.

### WebNN

WebNN should be an optional acceleration route for recognized inference graphs:

- It can lower whole neural-network subgraphs when the operator set, dtype,
  layout, precision, and device choice are proven compatible.
- It must not be used as a hidden fallback for general Python, NumPy, SciPy, or
  tinygrad operations.
- WebNN graph admission requires a parity oracle and a target feature row. If
  the browser does not expose WebNN, the same graph remains executable through
  CPU, WASM SIMD, native GPU, or WebGPU paths.

## BLAS, LAPACK, GSL, And Numeric Libraries

BLAS and LAPACK are the pattern to copy structurally: standard interfaces,
optimized implementations below them, and small portable reference paths only as
test/fallback baselines.

- BLAS gives the common vector, matrix-vector, and matrix-matrix building
  blocks used by high-quality linear algebra libraries.
- LAPACK intentionally concentrates dense linear algebra performance into Level
  3 BLAS so optimized per-machine kernels carry the speed.
- Molt should expose linear algebra through standard ABI/source custody, not by
  rewriting algorithms in Python.
- GSL is useful as a broad C numerical library target, but its GPL licensing
  means distribution must be license-gated. Support it through source admission
  and user/library custody, not by vendoring it into Molt's runtime.
- Native profiles should prefer provider BLAS/LAPACK where available:
  Accelerate/vecLib on Apple, OpenBLAS/BLIS, oneMKL, vendor libraries, or a
  checked reference artifact when performance is not claimed.
- WASM profiles need a source-recompiled static-link lane for the reachable
  symbols only. Do not ship a whole numerical library image to hide missing
  closure analysis.
- GPU profiles should treat GEMM, reductions, convolution/im2col, FFT, sparse,
  and batched kernels as canonical primitive families with target-specific
  implementations below one fact authority.

## Tensor And Buffer Interchange

The typed strided storage primitive is the keystone for ecosystem support.
Research confirms that Molt should align with existing interchange standards
instead of inventing a package-local tensor shape:

- CPython's buffer protocol already names producer/consumer roles for exposing
  large memory buffers without intermediate copying, with shape, strides, and
  contiguity request forms.
- DLPack supplies a minimal C ABI for n-dimensional tensors across CPU, CUDA,
  OpenCL, Vulkan, Metal, ROCm, WebGPU, and other device types.
- Arrow's C Data and Device interfaces supply ABI-stable columnar data exchange
  and zero-copy sharing between runtimes.
- Molt's storage authority should be able to project into these forms when the
  object, dtype, device, and lifetime contracts are satisfied. Projection is not
  a second storage authority.

## Tooling Obligations

Every repeated WASM/WebGPU diagnosis should become a one-command tool:

- A `molt wasm inspect` style tool should use `wasmparser`/`wasm-tools` to
  report section sizes, imports, exports, feature use, unresolved symbols,
  native opcode counts, boxed runtime-call counts, and retained profile roots.
- A `molt wasm diff` style tool should compare two artifacts by import closure,
  code size, opcode histogram, custom sections, feature requirements, and
  compressed size.
- A `molt wasm prove` style tool should fail if a claimed fast path still emits
  boxed calls, broad runtime resolver imports, unused native artifacts, or whole
  package images.
- Binaryen/`wasm-opt` remains a post-link optimizer, but Molt's own IR and
  reachability facts must do the semantic shrinking first. `wasm-opt` must never
  be the only thing preventing profile bloat.
- Wasmtime host startup work should use precompiled modules, pooling allocator,
  copy-on-write heap images, and `InstancePre` where the embedding supports
  them.

## Scoreboard Rows

Each target/profile row must record:

- CPython parity result and explicit TargetPythonVersion/platform gate.
- Raw, stripped, gzip, and brotli artifact sizes.
- Cold compile, warm compile, cold instantiate, warm instantiate, and first-call
  times.
- Host-call count and retained import count.
- Opcode histogram for scalar WASM, SIMD, atomics, EH, tail calls, memory64,
  GC, and relaxed SIMD.
- Allocation count, peak memory, and memory-copy count.
- Throughput vs CPython, PyPy, Codon where applicable, and native BLAS/GPU
  provider where a provider claim is made.
- Browser E2E row for WebGPU/WebNN claims, including adapter features and
  fail-closed fallback behavior.

## Immediate Concrete Work

1. Add a generated target feature manifest for native, wasm-server, wasm-browser,
   wasm-edge, wasm-browser-webgpu, and wasm-browser-webnn.
2. Add opcode/import histogram gates for R4a/R4b: a proven typed arithmetic path
   must emit native WASM scalar or SIMD opcodes and must not retain the boxed
   runtime-call lane.
3. Extend the typed strided storage primitive so buffer, ndarray, DLPack, Arrow,
   and GPU device storage are projections of one authority.
4. Add WebGPU feature probing and artifact metadata to browser packaging.
5. Add source-recompiled BLAS/LAPACK/GSL admission metadata with license,
   provider, symbols, object closure, and per-target artifact custody.
6. Build the one-command WASM artifact inspector before the next manual
   wasm-objdump/grep loop repeats.

## Primary Sources Checked

- WebAssembly feature tracking and proposals:
  `https://webassembly.org/features/`,
  `https://github.com/WebAssembly/proposals`
- WebAssembly SIMD:
  `https://github.com/WebAssembly/simd/blob/main/proposals/simd/SIMD.md`
- WebAssembly relaxed SIMD:
  `https://github.com/WebAssembly/relaxed-simd/blob/main/proposals/relaxed-simd/Overview.md`
- WebAssembly GC:
  `https://github.com/WebAssembly/gc/blob/main/proposals/gc/Overview.md`
- WebAssembly memory64:
  `https://github.com/WebAssembly/memory64/blob/main/proposals/memory64/Overview.md`
- WebAssembly tail calls:
  `https://github.com/WebAssembly/tail-call/blob/main/proposals/tail-call/Overview.md`
- WebAssembly threads:
  `https://github.com/WebAssembly/threads/blob/main/proposals/threads/Overview.md`
- WebAssembly JS Promise Integration:
  `https://github.com/WebAssembly/js-promise-integration/blob/main/proposals/js-promise-integration/Overview.md`
- WebAssembly flexible vectors, half precision, and wide arithmetic:
  `https://github.com/WebAssembly/flexible-vectors`,
  `https://github.com/WebAssembly/half-precision`,
  `https://github.com/WebAssembly/wide-arithmetic`
- WASI roadmap:
  `https://wasi.dev/roadmap`
- Component Model and WIT:
  `https://component-model.bytecodealliance.org/`,
  `https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md`
- WebGPU and WGSL:
  `https://www.w3.org/TR/webgpu/`,
  `https://gpuweb.github.io/gpuweb/explainer/`,
  `https://www.w3.org/TR/WGSL/`
- WebNN:
  `https://www.w3.org/TR/webnn/`
- Python buffer protocol:
  `https://docs.python.org/3.12/c-api/buffer.html`
- DLPack:
  `https://dmlc.github.io/dlpack/latest/`
- Apache Arrow C Data Interface:
  `https://arrow.apache.org/docs/format/CDataInterface.html`
- BLAS and LAPACK:
  `https://www.netlib.org/blas/`,
  `https://www.netlib.org/lapack/`
- GSL:
  `https://www.gnu.org/software/gsl/doc/html/intro.html`
- wasm-tools, Binaryen, and Wasmtime startup:
  `https://github.com/bytecodealliance/wasm-tools`,
  `https://github.com/WebAssembly/binaryen`,
  `https://docs.wasmtime.dev/examples-fast-instantiation.html`
