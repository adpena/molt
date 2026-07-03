# 71 - WASM, WebGPU, and Numeric Acceleration Plan

Status: research addendum, 2026-07-03

This document turns current WASM, WebGPU, WebNN, BLAS/LAPACK/GSL, and tensor
interchange research into Molt engineering obligations. It refines R4, R7, and
R8 from the live orchestration board. It is not a parallel roadmap and it does
not supersede `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`,
`docs/spec/areas/wasm/0970_BROWSER_NUMERIC_KERNEL_EMBED.md`, or
`docs/architecture/gpu-primitive-stack.md`.

The obligation is the 100-year plan shape: portable semantic truth first,
capability-gated lowering second, target-specific acceleration third, and a
scoreboard that refuses any performance, size, startup, or compatibility claim
without live evidence. This document is an implementation contract: every
outcome below must be reducible to one authority, one lowering path, one
artifact/proof path, and one release scoreboard row before support is claimed.

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

## 100-Year Plan Outcomes

These are not stretch goals. They are the long-horizon invariants this plan must
keep making easier to satisfy, and they are the non-negotiable outcome shape for
R4, R7, and R8 work:

1. One semantic authority feeds every backend. Python version/platform facts,
   representation facts, dtype/shape/stride facts, target features, and package
   custody flow through generated/shared authorities, never through backend-local
   reclassification.
2. Profiles describe deployment envelopes only. Tree shaking, deforestation,
   symbol reachability, native artifact admission, and hot-path lowering are
   structural compiler behavior, not profile-name tricks.
3. Deterministic CPython parity is the default lane. Relaxed SIMD, approximate
   GPU math, f16, vendor BLAS, and provider-specific kernels are opt-in
   acceleration tiers with explicit error budgets and oracle rows.
4. The hot numeric path has no accidental object traffic. Proven scalar and
   vector lanes carry raw values through frontend, TIR, passes, backend, and
   runtime boundaries; boxes are introduced only at semantic escape points.
5. Browser execution is a first-class target, not a demo. WASM, WebGPU, WebNN,
   JS Promise Integration, Component Model, and WASI adoption all pass through
   the same artifact, parity, startup, and size gates as native.
6. Ecosystem support compounds through primitives. NumPy, SciPy, tinygrad, GSL,
   BLAS, LAPACK, Arrow, and DLPack support must expand by improving ABI/C-API,
   buffer, capsule, typed storage, module-state, and artifact custody, not by
   package-specific rewrites.
7. Artifact size and startup only ratchet down for claimed workloads. Every
   retained runtime import, table slot, native object, generated table, custom
   section, and package root must be justified by reachability evidence.
8. Tooling eliminates repeated manual inspection. Every recurring
   wasm-objdump/grep/WGSL/browser-profiler step becomes a one-command inspector
   with compact verdicts, machine-readable evidence, and proof-queue custody.
9. Deployment is boring and portable. Windows, macOS, Linux, browsers, workers,
   edge runtimes, and native hosts all resolve pinned toolchains and feature
   contracts through checked-in configuration.
10. A support claim means a green scoreboard row. If parity, performance,
    startup, binary size, browser behavior, or provider acceleration is not
    measured and version/platform-gated, it is not claimed support.

Each outcome must carry an evidence ledger before a release or partnership
handoff claims it:

- Authority: the generated/shared source of truth that owns the semantic fact,
  target capability, package artifact, or storage/lifetime invariant.
- Lowering proof: the exact frontend, TIR, pass, backend, runtime, and package
  custody path that consumes that authority without a duplicate lane.
- Artifact proof: hashes, import/export closure, opcode histogram, retained
  runtime features, native objects, package roots, and generated tables.
- Parity proof: CPython >=3.12 oracle result with TargetPythonVersion and
  platform gates, plus explicit tolerances only for opt-in approximate tiers.
- Performance proof: startup, size, memory, allocation, copy count, host-call
  count, and throughput rows against CPython and the relevant provider
  baseline.
- Negative proof: synthetic violations that fail closed when a feature,
  symbol, provider, license, target capability, or lifetime contract is absent.

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

### Proposal Watchlist

The WebAssembly proposal map should be treated as an input to Molt's target
feature manifest, not as ad hoc release trivia:

- Threads and JS Promise Integration are Phase 4 work and should be modeled as
  near-term server/browser capability rows.
- Wide arithmetic, compact imports, custom page sizes, stack switching, ESM
  integration, and related Phase 3 work should be tracked now because each can
  remove boxed helpers, import-section bloat, continuation trampolines, or host
  glue when the toolchain proves support.
- Memory control, flexible vectors, half precision, shared-everything threads,
  reference-typed strings, profiles, and type imports remain watchlist features.
  They must not enter a claimed fast path until validation, runtime support,
  size/startup impact, and parity gates exist.
- WebAssembly features with host-dependent behavior enter only through an
  explicit non-deterministic tier. The default CPython-parity lane cannot depend
  on engine-chosen numeric behavior.

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

GSL-specific outcome: Molt should be able to compile/link a user-admitted GSL
closure when licensing and target constraints are satisfied, but Molt's runtime
must not vendor GPL GSL code or hide unsupported GSL behavior behind Python
shims. The reusable deliverable is C ABI/source artifact custody plus typed
buffer interop for the reachable symbols.

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

## Rigorous Exit Criteria

This plan is complete only when these gates exist and pass on the claimed
target/profile rows. Any missing row is a fail-closed non-claim, not a partial
success. A gate is not considered present because a document describes it; it is
present only when the repo contains the command, generated authority, expected
machine-readable evidence, and a negative test proving the gate fails on the
class of violation it is meant to prevent.

Every exit row must record:

- Command: the exact proof-queue lane, CI job, or deterministic local command
  that produces the evidence.
- Inputs: source file, target profile, Python version, OS/arch, feature
  manifest row, package/root manifest, and provider selection.
- Outputs: log path, artifact path, artifact hash, compact verdict, and
  machine-readable evidence file.
- Oracle: CPython or provider parity source, tolerance policy, and
  version/platform gate.
- Regression guard: the synthetic violation or fixture that fails if duplicate
  authority, boxed fallback, broad reachability, unsupported target behavior, or
  missing lifetime custody reappears.
- Scoreboard row: startup, size, throughput, memory, allocation, host-call,
  import-retention, and browser/provider result where applicable.

1. Target feature manifest:
   `native`, `wasm-server`, `wasm-browser`, `wasm-edge`,
   `wasm-browser-webgpu`, and `wasm-browser-webnn` rows declare scalar WASM,
   simd128, relaxed SIMD, EH, tail calls, memory64, threads/atomics, WasmGC,
   JSPI, Component Model/WASI, WebGPU, WebNN, f16, subgroups, timestamp-query,
   provider BLAS/LAPACK/GSL, and unsupported-reason diagnostics.
2. Reachability and deforestation:
   the artifact inspector proves every retained import, export, table slot,
   runtime feature, static native object, source root, and generated table is
   reachable from user code. Whole package/library images fail unless the user
   program genuinely reaches them.
3. Scalar WASM lowering:
   proven-typed integer and float loops emit native WASM scalar opcodes and do
   not retain boxed numeric runtime calls. The proof records opcode histograms,
   host-call counts, allocation counts, and parity against CPython.
4. SIMD lowering:
   vectorizable kernels emit `v128`/simd128 instructions under deterministic
   gates. Relaxed SIMD is rejected from deterministic lanes and accepted only
   with explicit target, oracle tolerance, and scoreboard annotation.
5. WasmGC profile:
   closed-layout object classes may use GC structs/arrays only after validation
   proves layout, lifetime, nullability, type checks, startup, size, and parity.
   Dynamic Python objects stay on the runtime-owned representation unless facts
   prove a safe GC lowering.
6. WASI/Component Model:
   WIT worlds are generated from Molt interface authority. Component adoption
   must prove import closure does not widen, hot-path throughput does not
   regress, async semantics remain CPython-compatible, and zero-copy stream or
   caller-supplied-buffer behavior is measured where claimed.
7. WebGPU:
   real browser execution passes parity for each claimed kernel family. The row
   records adapter, limits, features, WGSL validation, workgroup/subgroup/f16
   use, timestamp-query availability, dispatch time, copy counts, and fail-closed
   fallback behavior. A JavaScript-only fake dispatcher is CI scaffolding, never
   acceptance for a WebGPU claim.
8. WebNN:
   only whole recognized inference graphs enter WebNN. Admission records
   operator set, dtype, layout, precision, device, parity oracle, and fallback
   row. General NumPy/SciPy/tinygrad calls cannot silently route through WebNN.
9. BLAS/LAPACK/GSL:
   native rows name the provider and symbol closure; WASM rows use source-
   recompiled reachable objects only; license metadata is enforced; performance
   claims compare CPython/NumPy/SciPy plus provider baselines where applicable.
10. Typed storage interchange:
    buffer protocol, memoryview, ndarray/tensor construction, DLPack, Arrow C
    Data/Device, WebGPU buffers, and native ABI calls project from one storage
    authority with owner/base/lifetime/release tests and resize/export guards.
11. Startup and size:
    raw, stripped, gzip, and brotli size; cold/warm compile; cold/warm
    instantiate; first call; and retained imports all have ratchets per
    profile. A regression requires an explicit accepted scoreboard delta.
12. Cross-platform proof:
    Windows, macOS, and Linux rows run under pinned checked-in toolchain
    contracts. Unsupported platform/architecture combinations fail with precise
    diagnostics keyed to the target feature manifest.
13. Browser E2E:
    the browser path runs in a real browser or browser-equivalent automation,
    not only Node. It verifies artifact loading, manifest interpretation,
    runtime imports, table/callable exports, typed-array or buffer transfer,
    exception propagation, and result parity.
14. Observability:
    `molt wasm inspect`, `molt wasm diff`, and `molt wasm prove` or their exact
    successors produce compact verdicts and durable evidence paths. Manual
    wasm-objdump/grep evidence is insufficient after the tool exists.
15. Release scoreboard:
    R8 scoreboards compare against CPython, PyPy, Codon, and provider libraries
    where meaningful. Faster-than-CPython claims are removed unless the green
    row exists for the exact benchmark, target, profile, and platform.

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
  `https://wasi.dev/roadmap`,
  `https://github.com/WebAssembly/WASI/releases/tag/v0.3.0`
- Component Model and WIT:
  `https://component-model.bytecodealliance.org/`,
  `https://component-model.bytecodealliance.org/design/wit.html`,
  `https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md`
- WebGPU and WGSL:
  `https://www.w3.org/TR/webgpu/`,
  `https://gpuweb.github.io/gpuweb/explainer/`,
  `https://www.w3.org/TR/WGSL/`,
  `https://github.com/gpuweb/gpuweb/blob/main/proposals/subgroups.md`
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
  `https://docs.wasmtime.dev/examples-fast-instantiation.html`,
  `https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html`
