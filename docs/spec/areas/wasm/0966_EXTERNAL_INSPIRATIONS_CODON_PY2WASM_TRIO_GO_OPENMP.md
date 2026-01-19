# External Inspirations for Molt
**Doc ID:** 0966 (rev: OpenMP extension)
**Status:** Draft spec (v0.2)
**Audience:** Molt compiler/runtime engineers, library authors
**Goal:** Capture what Molt can learn, replicate, or intentionally diverge from in:

- **Codon** (AOT-compiled Python-like language)
- **Wasmer / py2wasm** (Python→WASM compilation pipelines)
- **Trio** (structured concurrency + cancellation semantics)
- **Go** (goroutines + channels + scheduler model)
- **OpenMP** (directive-based data parallelism + reductions)

This is a **design input doc** and a **work plan**: it states what to “steal,” why, and how to translate it into Molt’s tiered semantics and portability goals.

> This file is intended to replace/supersede `0966_EXTERNAL_INSPIRATIONS_CODON_PY2WASM_TRIO_GO.md`.

---

## 0. Why this matters

Molt is pursuing an ambitious combination:

- **Pythonic surface area** (enough to be useful for web + data)
- **native-speed execution** (runtime-first)
- **small binaries + cross-platform** (Go-like ergonomics)
- **WASM portability + browser/server interop**
- **no C-extension dependency** (ship Rust-native alternatives and/or stable ABIs)

To succeed, we must avoid reinventing everything blindly. We should:
- adopt proven compiler and runtime patterns,
- copy the user experience people love (esp. Go concurrency),
- learn from Python→native/WASM compilers (Codon, py2wasm) about what works and what breaks,
- and learn from mature parallelism models (OpenMP) about how to safely express and optimize CPU-bound data-parallel loops.

---

## 1. Snapshot: what these projects are (and why they matter)

### 1.1 Codon
Codon is an ahead-of-time compiler for a Python-like language that compiles to native machine code, focusing on performance and native multithreading. It uses LLVM and provides a plugin/extensibility model.

References:
- https://github.com/exaloop/codon
- https://docs.exaloop.io/
- https://cap.csail.mit.edu/sites/default/files/research-pdfs/Codon-%20A%20Compiler%20for%20High-Performance%20Pythonic%20Applications%20and%20DSLs.pdf

---

### 1.2 Wasmer / py2wasm
py2wasm is a Python→WASM toolchain (with Wasmer as runtime) built pragmatically, focusing on producing WASM artifacts and running them efficiently compared to “interpreter in WASM” approaches.

References:
- https://wasmer.io/posts/py2wasm-a-python-to-wasm-compiler
- https://github.com/wasmerio/py2wasm
- https://wasmer.io/posts/python-on-the-edge-powered-by-webassembly

---

### 1.3 Trio
Trio is an async library that popularized **structured concurrency**: tasks are spawned within explicit scopes (“nurseries”) with hierarchical cancellation (“cancel scopes”). It makes async error-handling and cancellation predictable.

References:
- https://trio.readthedocs.io/
- https://vorpus.org/blog/notes-on-structured-concurrency-or-go-statement-considered-harmful/

---

### 1.4 Go
Go’s concurrency model: goroutines + channels + select, supported by an M:N scheduler. It dominates server workloads because concurrency is a first-class product feature.

References:
- https://go.dev/src/runtime/HACKING
- https://rakyll.org/scheduler/

---

### 1.5 OpenMP
OpenMP is a standard for **directive-based parallel programming** (pragmas) for shared-memory systems. It provides:
- parallel regions (`parallel`)
- work-sharing (`for`, `sections`, `single`)
- reductions (`reduction`)
- scheduling controls (`schedule`)
- synchronization primitives

OpenMP is valuable as a model for CPU-bound parallel loops and reductions, and for understanding what “minimal parallel semantics” look like in the wild.

References:
- https://www.openmp.org/
- https://clang.llvm.org/docs/OpenMPSupport.html
- https://learn.microsoft.com/en-us/cpp/parallel/openmp/reference/openmp-directives?view=msvc-170

---

## 2. What Molt should steal from Codon

### 2.1 Static-by-default with explicit escape hatches
Codon reinforces a key truth: high performance comes from specialization and rejecting/isolating the hardest dynamic features.

**Molt translation:** our Tier model expresses this:
- Tier 0/1 = “Codon-like world”
- Tier 2 = compatibility islands + deopt

**Spec requirement:** each Molt feature declares:
- tier eligibility
- required guards
- fallback semantics

---

### 2.2 Own the performance-critical libraries
Codon’s approach shows: you become useful when you ship fast “default” libs, otherwise users hit the ecosystem wall.

**Molt translation:** “no C extensions” implies a Rust-native baseline stack:
- `molt.json`
- `molt.http`
- `molt.sql`
- `molt.df` (DataFrame engine: Arrow/Polars/DuckDB-based)
- `molt.regex`

---

### 2.3 Treat compiler extensibility as first-class
Codon’s plugin model suggests: treat the compiler as a platform.

**Molt translation:** a build-time plugin system for:
- IR passes
- idiom recognizers
- library lowering hooks
- target backends (native/WASM)

---

## 3. What Molt should steal from py2wasm / Wasmer

### 3.1 WASM as a first-class deployment artifact
Molt should always be able to produce:
- native executable
- WASM module
- optional server-companion ABI glue

### 3.2 Packaging: deterministic, build-time-resolved
Do not promise “pip install arbitrary wheels” for WASM.
Instead:
- define a Molt package format for WASM artifacts
- resolve deps at build time (uv can help)
- ship curated, Rust-native modules

### 3.3 Avoid a bag of fallbacks
A “CPython fallback inside Molt” increases:
- binary size
- security surface
- unpredictability

**Recommendation:** Molt v0 should not ship “hidden interpreter fallback” as the default. If any fallback exists, it must be explicit, isolated, and measurable.

---

## 4. What Molt should steal from Trio (structured async)

### 4.1 Structured concurrency should be the default
Trio’s nursery + cancel scope model is a major usability win.

**Molt translation:** make structured concurrency the default:
- spawn inside a scope
- errors cancel siblings
- scope boundary re-raises

### 4.2 Cancellation is a semantic feature, not an afterthought
Cancellation must be:
- hierarchical
- predictable
- testable
- supported by “checkpoints” (safepoints)

---

## 5. What Molt should steal from Go (UX magic for servers)

Go’s “feel” comes from:
- cheap spawn
- explicit communication (channels)
- select/race semantics
- scheduler as product

**Molt translation:** “Molt Tasks” (goroutines, but safe) with:
- TaskGroup/Nursery ownership
- Cancel scopes
- Channels
- Select

---

## 6. What Molt should steal from OpenMP (CPU-parallel loops)

OpenMP is not “async concurrency for servers”. It’s “data parallelism for compute”.

Molt needs **both**:
- Go/Trio-like tasks for I/O-bound concurrency (web, DB, pipelines)
- OpenMP-like loop parallelism for CPU-bound kernels (data transforms, dataframe ops, JSON parsing, compression)

### 6.1 The key OpenMP ideas worth stealing
1. **A clear contract for parallel loops**
   - “These iterations may run in parallel.”
2. **Reduction as a first-class concept**
   - sum/min/max/and/or with identity values
3. **Scheduling policies**
   - static vs dynamic vs guided
4. **A mental model that survives optimization**
   - predictable behavior even when parallel

### 6.2 What NOT to copy from OpenMP
- pragma-based “sprinkle parallelism” everywhere without proofs
- relying on a platform-provided OpenMP runtime for correctness/portability
- GPU offloading complexity in v0

OpenMP is a **design influence** and a possible backend, not necessarily a runtime dependency.

---

## 7. Proposed Molt feature spec: Parallel Loops (OpenMP-inspired)

This is the core “OpenMP section” as Molt product/spec.

### 7.1 IR surface (compiler contract)
Add explicit IR primitives:
- `ParFor { iv, start, end, step, schedule, chunk, body }`
- `Reduction { op, identity, combine, local_var, global_var }`
- `Barrier` (rare; prefer structured reductions)

**Guiding constraint:** the compiler must be able to prove (Tier 0) or guard (Tier 1) that the loop body is safe to parallelize.

### 7.2 Source surface (user-facing)
Two staged approaches:

#### Stage A (Pythonic surface; zero new syntax)
Provide a library API:
```python
from molt import parallel

parallel.for_range(0, n, fn=lambda i: ...)
```

Or a decorator:
```python
@parallel.kernel
def body(i: int) -> None: ...
parallel.for_range(0, n, body)
```

#### Stage B (Molt-extended syntax; optional)
Introduce Molt-only sugar (not valid CPython):
```molt
parfor i in range(n):
    ...
```

**Recommendation:** ship Stage A first.

### 7.3 Safety rules (must be explicit)
A `ParFor` is only valid if:
- loop iterations are independent, OR
- all cross-iteration state is expressed as a supported **reduction**, OR
- writes are provably disjoint (e.g., `out[i] = ...`)

Forbidden (Tier 0/1) in a parallel loop body:
- mutation of shared Python objects (dict/list) without proven disjointness
- dynamic attribute writes
- I/O (unless explicitly marked “allowed but non-deterministic”)
- dependence on iteration ordering (unless schedule=static and order is preserved, which is expensive)

### 7.4 Reduction semantics
Support first:
- `sum`, `min`, `max`
- boolean `any/all` style reductions
- user-defined reduction types later (v1+)

**Spec:** reductions must be associative (and ideally commutative).
If not, results may differ from sequential order; strict mode should forbid non-associative reductions.

### 7.5 Scheduling semantics (portable subset)
Implement a portable subset:
- `static` (default): split range evenly
- `dynamic(chunk=k)`: work-stealing from a queue
- `guided`: later

### 7.6 Determinism modes
- **deterministic mode:** fixed scheduling + stable reduction combine order (slower)
- **performance mode:** best-effort scheduling (faster)

Expose this as a compile flag.

---

## 8. Implementation options (runtime)

### Option 1 — “Molt-native parallel runtime” (recommended)
- A dedicated thread pool for CPU kernels
- Work-stealing scheduler (Rayon-like)
- Small, static-link-friendly
- Integrates with Molt’s profiling/telemetry

Pros:
- portable
- controllable
- consistent semantics across platforms

Cons:
- engineering effort

### Option 2 — Use OpenMP runtime (`libomp`) as backend (optional)
- Emit calls that rely on OpenMP runtime

Pros:
- mature scheduling policies

Cons:
- packaging and distribution complexity (esp. macOS)
- hard to control binary size and determinism
- not suitable for WASM

**Recommendation:** treat OpenMP as a conceptual model; implement Molt-native runtime first.

---

## 9. WASM implications
WASM parallelism depends on threads + shared memory support and host policies. In browser environments, threading is not always available or enabled. Therefore:

- WASM target must support a **single-threaded fallback** for `ParFor`
- `ParFor` should degrade to `Loop` automatically if threads unavailable
- SIMD and vectorization are the primary WASM perf levers (parallel threads optional)

---

## 10. How this integrates with Molt Tasks (Go/Trio model)

**Rule:** do not unify everything under “async tasks.”
We need two planes:

1. **I/O concurrency plane** (Tasks): request routing, DB I/O, pipelines, background jobs
2. **CPU parallel plane** (ParFor): data transforms, dataframe kernels, parsing, compression

Interop rules:
- A Task can invoke a ParFor kernel.
- ParFor kernels should not perform I/O.
- Cancellation: ParFor should observe cancellation checkpoints at chunk boundaries.

This separation keeps semantics and performance predictable.

---

## 11. Open questions (must decide)
1. How to classify “parallel unsafe” operations in the tier model?
2. Determinism: do we default to deterministic scheduling?
3. How do we surface “this loop was parallelized” telemetry to users?
4. What is the minimum “kernel subset” we support in v0?

---

## 12. Bottom-line recommendation
Yes, we should incorporate OpenMP analysis—because Molt needs a clear story for **CPU-parallel data kernels**, especially for dataframe work and high-performance library implementations.

But we should not “depend on OpenMP” as a runtime requirement in v0.
We should:
- **steal the semantic model**, and
- implement a **Molt-native parallel loop runtime** that can target native and degrade cleanly on WASM.
