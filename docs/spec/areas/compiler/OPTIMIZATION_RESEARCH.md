# Compiler Optimization Research: State of the Art

> Compiled 2026-03-12. Research survey of optimization techniques relevant to Molt's
> Python-to-native compilation pipeline (HIR -> TIR -> LIR -> Cranelift/WASM).

---

## Table of Contents

1. [MLIR and Modern Compiler Infrastructure](#1-mlir-and-modern-compiler-infrastructure)
2. [Cranelift Optimization Techniques](#2-cranelift-optimization-techniques)
3. [PyPy and Tracing JIT Insights](#3-pypy-and-tracing-jit-insights)
4. [Graal/GraalVM and Partial Evaluation](#4-graalgvm-and-partial-evaluation)
5. [V8 TurboFan and Hidden Classes](#5-v8-turbofan-and-hidden-classes)
6. [LuaJIT and Luau Optimization](#6-luajit-and-luau-optimization)
7. [Escape Analysis State of the Art](#7-escape-analysis-state-of-the-art)
8. [Reference Counting Optimization](#8-reference-counting-optimization)
9. [Cache-Oblivious Algorithms](#9-cache-oblivious-algorithms)
10. [SIMD Auto-Vectorization](#10-simd-auto-vectorization)

---

## 1. MLIR and Modern Compiler Infrastructure

### Key Sources

- [MLIR Users](https://mlir.llvm.org/users/) -- canonical list of MLIR-based compilers
- [Triton MLIR Dialects (DeepWiki)](https://deepwiki.com/triton-lang/triton/3-mlir-dialects-and-ir-system) -- multi-level dialect design
- [ML-Triton: A Multi-Level Compilation and Language (arXiv)](https://arxiv.org/pdf/2503.14985) -- 2025 paper on multi-level IR
- [MLIR Transform Dialect (arXiv 2024)](https://www.arxiv.org/pdf/2409.03864v2) -- writing lowering pipelines in IR
- [Modular: What about MLIR?](https://www.modular.com/blog/democratizing-ai-compute-part-8-what-about-the-mlir-compiler-infrastructure) -- Mojo's use of MLIR
- [Mojo Vision](https://docs.modular.com/mojo/vision/) -- KGEN compiler architecture
- [MLIR Progressive Lowering Tutorial (Ch5)](https://mlir.llvm.org/docs/Tutorials/Toy/Ch-5/) -- official MLIR tutorial
- [Transform-dialect schedules (EuroLLVM 2024)](https://llvm.org/devmtg/2024-04/slides/StudentTechnicalTalks/Morel-Transform-DialectSchedules.pdf) -- auto-tuning pipelines

### Core Techniques

**Progressive lowering through dialect layers.** MLIR's key insight is that compilation is best modeled as a series of progressive transformations between domain-specific intermediate representations (dialects). Each dialect preserves domain semantics at its level, enabling optimizations that would be impossible at lower levels. The canonical lowering path is:

```
High-level domain dialect -> Structured ops (linalg) -> Affine/SCF -> MemRef/Arith -> LLVM dialect -> Native
```

**How Mojo uses MLIR.** Mojo's compiler (codenamed KGEN, "kernel generator") is built on MLIR Core and forms the backbone of Mojo's metaprogramming. It controls the full stack from high-level Python-like syntax through systems programming to hardware-specific codegen. Key lesson: owning the dialect stack lets you apply domain-specific rewrites at every level.

**How Triton uses MLIR.** Triton's dialect system provides a structured framework for compiling high-level tensor operations to GPU code. Key aspects: multi-level representation, layout-centric design (encoding data layouts as dialect attributes), hardware abstraction, and extensibility. The TritonGPU dialect carries layout information through the compilation pipeline.

**Transform dialect for auto-tuning (2024).** The MLIR Transform Dialect turns pass composition into first-class IR, enabling auto-tuning and static validation of pipelines with negligible overhead (<=2.6% compile-time increase). This supports parameterized search for kernel optimality.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Progressive lowering | Already done (HIR->TIR->LIR) | Foundation | Already implemented |
| Custom dialect for Python semantics | Could formalize TIR as an MLIR dialect | Medium | Very High |
| Transform dialect for pass scheduling | Could auto-tune Molt's pass ordering | Medium | High |
| Layout-centric design (Triton-style) | Useful for future GPU/SIMD lanes | High for GPU | Medium |

**Recommendation**: Molt already has progressive lowering. The main takeaway is that MLIR's value lies in the _dialect ecosystem_ -- community-contributed optimization passes. For Molt, investing in MLIR would only pay off if we needed GPU codegen (NVPTX via MLIR), which aligns with the staged GPU plan. For CPU, Cranelift is the right choice. **Do not adopt MLIR for CPU-only workloads.**

---

## 2. Cranelift Optimization Techniques

### Key Sources

- [Cranelift E-Graph RFC](https://github.com/bytecodealliance/rfcs/blob/main/accepted/cranelift-egraph.md) -- the egraph-based mid-end
- [Aegraphs: Acyclic E-graphs for Production Compilers (EGRAPHS 2023)](https://pldi23.sigplan.org/details/egraphs-2023-papers/2/-graphs-Acyclic-E-graphs-for-Efficient-Optimization-in-a-Production-Compiler) -- Cranelift's novel approach
- [Cranelift vs LLVM](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/compare-llvm.md) -- official comparison doc
- [Cranelift Progress 2022](https://bytecodealliance.org/articles/cranelift-progress-2022) -- egraph adoption
- [rustc_codegen_cranelift Nov 2024](https://bjorn3.github.io/2024/11/14/progress-report-nov-2024.html) -- current status
- [CGO 2024: Compile-Time Analysis of Compiler Frameworks](https://home.cit.tum.de/~engelke/pubs/2403-cgo.pdf) -- benchmark data
- [VeriISLE: Verifying Instruction Selection](http://reports-archive.adm.cs.cmu.edu/anon/2023/CMU-CS-23-126.pdf) -- formal verification of ISLE

### Core Techniques

**Acyclic E-graphs (aegraphs).** Cranelift is the first production compiler to use e-graphs for its mid-end. The innovation is a hybrid between a CFG and an e-graph: pure operators are represented in the e-graph while side-effecting operations remain on a "side-effect skeleton" in the CFG. Key properties:

- **Acyclicity**: No fixpoint iteration needed; single-pass application of rewrite rules
- **Scoped elaboration**: Converts the e-graph back to a CFG by traversing the dominator tree; subsumes GVN and LICM automatically
- **Unified framework**: Replaces separate GVN, LICM, constant folding, and rematerialization passes

**What Cranelift does well:**
- Fast compilation (10-20x faster than LLVM)
- E-graph-based GVN + LICM + constant folding in a single pass
- ISLE (Instruction Selection Language): a verified DSL for lowering rules
- Reasonable register allocation (new faster allocator landed in 2024)

**What Cranelift lacks vs LLVM (~14% codegen quality gap):**
- No auto-vectorization (critical gap)
- No loop unrolling / loop transformations
- No polyhedral loop optimizations
- Limited alias analysis
- No interprocedural optimizations
- No profile-guided optimization (PGO)
- Weaker addressing-mode lowering (x86-64 and aarch64)
- No mid-level IR for machine-independent optimizations above Cranelift IR

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| ISLE rewrite rules | Can extend Cranelift's mid-end for Python patterns | Medium | Medium |
| Pre-Cranelift loop transformations | Add loop unrolling/tiling in TIR/LIR before Cranelift | High (10-30%) | Medium |
| Pre-Cranelift SIMD lowering | Emit explicit SIMD ops in LIR for Cranelift to select | High (2-8x for loops) | High |
| PGO-like profiling | Instrument dev builds, feed back to TIR specializer | Medium (5-15%) | Medium |
| Custom ISLE patterns | Python-specific patterns (e.g., range iteration) | Medium | Low |

**Recommendation**: The 14% gap vs LLVM is acceptable for Molt's use case (compile speed matters). The biggest wins come from doing optimizations _before_ Cranelift: loop unrolling in LIR, explicit SIMD emission, and type-specialized lowering. These bypass Cranelift's limitations entirely.

---

## 3. PyPy and Tracing JIT Insights

### Key Sources

- [Tracing the Meta-Level: PyPy's Tracing JIT (ACM 2009)](https://dl.acm.org/doi/10.1145/1565824.1565827) -- seminal paper
- [Musings on Tracing in PyPy (PyPy Blog, Jan 2025)](https://pypy.org/posts/2025/01/musings-tracing.html) -- retrospective by core devs
- [Pycket: Tracing JIT for Functional Languages](https://www.ccs.neu.edu/home/samth/pycket-draft.pdf) -- meta-tracing applied to Racket
- [CO-OPTIMIZING Hardware and Meta-Tracing JIT (Cornell 2019)](https://www.csl.cornell.edu/~cbatten/pdfs/ilbeyi-arch-jit-cuthesis2019.pdf) -- hardware co-design

### Core Techniques

**Meta-tracing.** PyPy doesn't trace the user program directly -- it traces the _interpreter_ executing the program. This lets the JIT "see through" interpreter dispatch, boxing, and dynamic lookup. The result is that a single JIT framework works for any language implemented on the RPython toolchain.

**What works well (AOT-applicable lessons):**

1. **Allocation removal through trace linearity.** Traces have no control-flow merges, making escape analysis trivially simple: a forward pass identifies allocations, a backward pass removes those that don't escape the trace. This is _much_ simpler than general EA algorithms. **AOT lesson**: In Molt's TIR, after type specialization removes most polymorphism, many code paths become linear. We could apply trace-style allocation removal to specialized TIR functions.

2. **Call-site-dependent path splitting "for free."** Tracing naturally inlines through call sites, producing specialized versions per call context. **AOT lesson**: Molt's type-directed monomorphization already achieves this. The lesson is that call-site specialization is one of the highest-impact optimizations for Python.

3. **Guard-based speculation.** Traces insert guards (type checks) and the rest of the trace assumes the guard passed. **AOT lesson**: Molt already has a guard/deopt model (see `0191_DEOPT_AND_GUARD_MODEL.md`). The PyPy experience confirms this is the right architecture.

4. **Integer range and bit analysis.** PyPy tracks known bits of integer variables to optimize heap operations and dictionary access. **AOT lesson**: Molt's TIR could carry integer range facts to eliminate bounds checks and optimize hash computations.

**What doesn't work (avoid these):**

1. **Performance cliffs.** When traces fail to form (highly branchy code, bytecode interpreters), performance degrades severely. Method-based JITs offer more consistent performance. **AOT lesson**: Molt, being AOT, doesn't have this problem -- all code is compiled.

2. **Heuristic complexity.** Deciding when to stop inlining, how to handle recursion, and managing trace length adds complexity that "loses a lot of the simplicity of tracing." **AOT lesson**: Molt should use simple, predictable heuristics for inlining/specialization rather than trying to be clever.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Trace-style allocation removal on linear TIR | Very high -- simplifies EA enormously | High (10-40%) | Medium |
| Integer range/bit analysis in TIR | Medium -- optimize bounds checks, hashing | Medium (5-10%) | Low |
| Guard-based speculation model | Already implemented | Foundation | Done |
| Call-site specialization | Already done via monomorphization | Foundation | Done |

**Recommendation**: The single most portable insight from PyPy is that _linear code paths make allocation removal trivial_. After Molt's type specialization pass, many function bodies are effectively linear (all type-polymorphic branches resolved). We should implement a lightweight "linear allocation removal" pass in TIR that operates on these simplified bodies.

---

## 4. Graal/GraalVM and Partial Evaluation

### Key Sources

- [The Futamura Projection and GraalVM Truffle](https://devm.io/java/graalvm-truffle-framework-futamura) -- overview
- [Practical Second Futamura Projection (OOPSLA 2019)](https://dl.acm.org/doi/10.1145/3359061.3361077) -- partial eval for interpreters
- [GraalVM Publications](https://github.com/oracle/graal/blob/master/docs/Publications.md) -- research papers
- [Partial Escape Analysis and Scalar Replacement for Java (CGO 2014)](https://dl.acm.org/doi/10.1145/2581122.2544157) -- PEA algorithm
- [IPEA: Inlining-Benefit Prediction (VMIL 2022)](https://dl.acm.org/doi/10.1145/3563838.3567677) -- interprocedural PEA
- [Under the Hood of GraalVM JIT Optimizations](https://medium.com/graalvm/under-the-hood-of-graalvm-jit-optimizations-d6e931394797) -- optimization overview

### Core Techniques

**First Futamura Projection.** Given an interpreter I and a program P, partially evaluating I with respect to P yields a compiled program. Truffle implements this: write an interpreter with AST-node specialization, and the Graal compiler partially evaluates (constant-folds) the interpreter's dispatch logic away, producing native code for the specific program.

**Why this matters for Molt**: Molt is _already_ an AOT compiler, not an interpreter. We don't need the Futamura projection. However, the _optimization techniques_ that make Truffle fast are directly applicable.

**Partial Escape Analysis (PEA).** GraalVM's crown jewel optimization. Unlike traditional escape analysis (which is flow-insensitive -- an object either escapes everywhere or nowhere), PEA is flow-sensitive: it determines _on which branches_ an object escapes and moves allocation to only those branches. Objects that don't escape are scalar-replaced (decomposed into local variables).

Key results:
- Double-digit speedups on Scala/Java benchmarks using Streams, Lambdas, iterators
- Up to 24.62% improvement on specific benchmarks with IPEA
- Geometric mean improvement of 1.79% across 36 industry benchmarks (IPEA)

**IPEA (Interprocedural PEA).** Extends PEA across procedure boundaries by predicting inlining benefits: "if we inline this callee, how much allocation can PEA remove?" This guides inlining decisions to maximize allocation elimination.

**Speculative optimizations with deoptimization.** Graal speculates on type profiles and inserts deoptimization points. When assumptions are violated, it falls back to interpreted execution and recompiles. Recent work (PLDI 2022) introduced "Deoptless" -- using dispatched on-stack replacement and specialized continuations instead of full deoptimization.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Partial Escape Analysis in TIR | Very high -- Python creates many temporary objects | Very High (15-40%) | High |
| IPEA for inlining decisions | High -- guides which functions to monomorphize | Medium (5-15%) | High |
| Scalar replacement of aggregates | Very high -- eliminate tuple/namedtuple allocation | Very High (20-50% for FP code) | Medium |
| Speculation + deopt model | Already have guard/deopt framework | Foundation | Done |

**Recommendation**: PEA is the highest-impact optimization Molt is missing today. Python idioms create enormous numbers of temporary objects: tuple returns, iterator state, context managers, comprehension intermediates. PEA would eliminate most of these allocations on non-escaping paths. **This should be a P0 optimization for the TIR pass pipeline.**

The implementation approach:
1. Start with intraprocedural PEA on specialized TIR
2. Track "virtual objects" -- objects that exist only as collections of local variables
3. At merge points, materialize virtual objects only if they escape on any successor path
4. Later, add IPEA to guide monomorphization/inlining decisions

---

## 5. V8 TurboFan and Hidden Classes

### Key Sources

- [V8 Inline Caching Deep Dive](https://braineanear.medium.com/the-v8-engine-series-iii-inline-caching-unlocking-javascript-performance-51cf09a64cc3)
- [V8 Optimization: TurboFan, Hidden Classes & Performance](https://huntize.com/learn/understanding-v8-and-code-optimization/)
- [Ignition and TurboFan Pipeline](https://github.com/thlorenz/v8-perf/blob/master/compiler.md)
- [Hidden V8 Optimizations](https://medium.com/@yashschandra/hidden-v8-optimizations-hidden-classes-and-inline-caching-736a09c2e9eb)

### Core Techniques

**Hidden classes (Maps/Shapes).** V8 assigns each object a "hidden class" (called a Map) that describes its layout: which properties exist, their types, and their memory offsets. Objects with the same property set in the same order share a hidden class. This converts dynamic property lookup into static offset access.

**Inline Caching (IC).** Each property access site records the hidden classes it has seen:
- **Monomorphic**: One hidden class seen -> direct offset load (fastest, ~1 instruction)
- **Polymorphic**: 2-4 hidden classes -> linear check of cached shapes
- **Megamorphic**: 5+ hidden classes -> fall back to hash lookup (slowest)

TurboFan uses IC data as type feedback: if a property access is monomorphic, the optimized code assumes the specific hidden class and emits a direct memory load with a guard.

**4-tier compilation pipeline.** V8 uses Sparkplug (baseline) -> Maglev (mid-tier) -> TurboFan (advanced). Each tier applies progressively more aggressive optimizations based on accumulated type feedback.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Hidden classes / shape system | Molt uses NaN-boxed objects; class layouts are known at compile time | Low (already done better) | N/A |
| Inline caching for attribute access | Relevant for `__getattr__` on user classes with inheritance | Medium (5-15%) | Medium |
| Type feedback for specialization | AOT type inference replaces runtime profiling | Low | N/A |
| Monomorphic dispatch guards | Already in guard/deopt model | Foundation | Done |

**Recommendation**: Molt's AOT type inference already provides what V8 gets from runtime type feedback. The main applicable technique is IC for cases where AOT analysis cannot fully resolve attribute lookups (e.g., deep class hierarchies with method overrides). Implement polymorphic inline caches as a fallback path in the runtime, guarded by type checks that the TIR specializer emits.

---

## 6. LuaJIT and Luau Optimization

### Key Sources

- [LuaJIT Allocation Sinking (wiki)](https://github.com/tarantool/tarantool/wiki/LuaJIT-Allocation-Sinking-Optimization) -- detailed algorithm
- [LuaJIT Optimizations (wiki)](https://github.com/tarantool/tarantool/wiki/LuaJIT-Optimizations) -- full optimization list
- [How We Make Luau Fast](https://luau.org/performance/) -- Luau's optimization approach
- [A Walk with LuaJIT (Polar Signals 2024)](https://www.polarsignals.com/blog/posts/2024/11/13/lua-unwinding)
- [Luau Recap July 2024](https://devforum.roblox.com/t/luau-recap-july-2024/3082271) -- recent improvements

### LuaJIT Core Techniques

**Allocation sinking.** LuaJIT's most impressive optimization. Uses a two-phase mark-and-sweep algorithm on the trace IR:

1. **Mark phase** (backward): marks allocations that _cannot_ be sunk (those referenced by snapshots, PHIs, loads, calls, or non-sinkable stores)
2. **Sweep phase** (forward): tags unmarked allocations and related stores as sunk

Performance: 26.9s -> 0.2s on point-arithmetic benchmarks (134x speedup). The optimized inner loop emits only 4 SSE2 `addsd` instructions per iteration with zero allocations.

**Store-to-load forwarding.** Advanced alias analysis replaces loads from temporary objects with the stored values directly. When combined with sinking, this eliminates all memory operations for temporary objects.

**Snapshot handling for sunk allocations.** When execution exits a trace with sunk allocations, they must be "unsunk" -- actually allocated and populated. This is handled by a data-driven exit handler that reconstructs objects from register/spill-slot values.

### Luau Optimization Techniques

**Type-directed codegen.** Luau controls the entire stack (unlike TypeScript), so type annotations drive code generation. The JIT uses type annotations to specialize code paths and infer absence of side effects for arithmetic and builtins.

**Specific 2024 improvements:**
- Guard-based codegen for `bit32` operations: ~30% improvement on affected benchmarks
- Function inlining with heuristic-driven decisions for local functions
- Compile-time-bounded loop unrolling
- Import optimization: `math.max` resolved at load time in pure environments

**Fastcall mechanism.** Builtin functions like `math.max(x, 1)` bypass normal stack frame setup. Specialized implementations handle common cases directly.

**Upvalue optimization.** ~90% of upvalues are immutable captures; these are captured by value, eliminating allocation overhead.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Allocation sinking (LuaJIT-style) | Very high -- eliminates temporary object allocations | Very High (10-100x for FP code) | High |
| Store-to-load forwarding | High -- reduces memory traffic | High (5-20%) | Medium |
| Type-directed codegen (Luau) | Already doing this via TIR specialization | Foundation | Done |
| Guard-based type checks (Luau) | Already in guard/deopt model | Foundation | Done |
| Fastcall for builtins | Molt intrinsics already bypass Python call protocol | Foundation | Done |
| Loop unrolling with bounds | Should add to LIR | Medium (5-15%) | Low |

**Recommendation**: LuaJIT's allocation sinking is the gold standard for temporary object elimination in dynamic languages. The mark-and-sweep approach on SSA IR is directly implementable in Molt's LIR. Combined with PEA from the GraalVM section, this would eliminate the vast majority of temporary allocations in Python code (tuples, small lists, iterator objects, comprehension intermediates).

Since Molt targets Luau as a backend, we should also study Luau's native codegen more carefully to ensure our emitted Luau code is amenable to its optimizer -- specifically, ensuring type annotations are present and that we emit patterns the Luau compiler recognizes for fastcall and upvalue optimization.

---

## 7. Escape Analysis State of the Art

### Key Sources

- [Escape Analysis for Java (Choi et al., OOPSLA 1999)](https://dl.acm.org/doi/10.1145/320385.320386) -- foundational algorithm
- [OpenJDK HotSpot Escape Analysis](https://wiki.openjdk.org/display/HotSpot/EscapeAnalysis) -- production implementation
- [Partial Escape Analysis for Java (CGO 2014)](https://dl.acm.org/doi/10.1145/2581122.2544157) -- GraalVM PEA
- [IPEA (VMIL 2022)](https://dl.acm.org/doi/10.1145/3563838.3567677) -- interprocedural PEA
- [Escape Analysis Across Languages (kipply's blog)](https://kipp.ly/escape-analysis/) -- comparison across PyPy, LuaJIT, V8, C++, Go
- [HotSpot EA Status](https://cr.openjdk.org/~cslucas/escape-analysis/EscapeAnalysis.html) -- current HotSpot implementation

### Algorithm Comparison

| Algorithm | Sensitivity | Scope | Used By | Performance |
|-----------|-----------|-------|---------|-------------|
| Connection Graph (Choi 1999) | Flow-insensitive | Intraprocedural + summaries | HotSpot C2 | Baseline |
| Partial EA (Stadler 2014) | Flow-sensitive | Intraprocedural | GraalVM | 2-5x more allocations eliminated |
| IPEA (Prokopec 2022) | Flow-sensitive | Interprocedural | GraalVM Native Image | Up to 24.62% improvement |
| Trace-based (PyPy) | N/A (linear traces) | Single trace | PyPy | Trivially complete on traces |
| Sinking (LuaJIT) | Forward/backward | Single trace | LuaJIT | 134x on micro-benchmarks |

### Escape States

Objects are classified into three escape states:
1. **NoEscape** -- object does not escape the creating method/scope; candidate for scalar replacement
2. **ArgEscape** -- object is passed as argument but does not globally escape; can be stack-allocated
3. **GlobalEscape** -- object is stored in a global or heap location; must be heap-allocated

### Key Insight: Partial vs Full EA

Full EA (HotSpot C2) is _flow-insensitive_: if an object escapes on _any_ control path, it is treated as escaping everywhere. This is conservative -- many objects escape only on uncommon paths (error handling, logging, debug output).

PEA (GraalVM) is _flow-sensitive_: it tracks virtual objects through the CFG and materializes them only at merge points where they escape. This captures the common pattern:

```python
point = Point(x, y)       # virtual
result = point.x + point.y  # scalar replaced
if debug:
    log(point)              # materialized only on this branch
return result
```

### Applicability to Molt

**Python-specific escape patterns to target:**
- Tuple packing/unpacking (function returns, multiple assignment)
- Iterator protocol objects (`__iter__` / `__next__` state)
- Context manager `__enter__`/`__exit__` state
- Comprehension intermediate lists/sets/dicts
- `dataclass` and `NamedTuple` instances in arithmetic code
- Exception objects in `try`/`except` that are caught and discarded

**Recommended implementation for Molt:**

Phase 1 (Medium complexity, High impact):
- Implement flow-sensitive PEA in TIR after type specialization
- Track "virtual objects" (tuples, small user classes) as sets of local variables
- At merge points, check if any successor materializes; if not, keep virtual
- Scalar-replace NoEscape objects

Phase 2 (High complexity, Medium impact):
- Add IPEA to guide inlining decisions
- Use interprocedural summaries to predict allocation removal benefits

Estimated impact: 15-40% reduction in allocation traffic for typical Python code.

---

## 8. Reference Counting Optimization

### Key Sources

- [Perceus: Garbage Free Reference Counting with Reuse (PLDI 2021)](https://dl.acm.org/doi/10.1145/3453483.3454032) -- Koka's RC algorithm
- [Counting Immutable Beans (arXiv 2019)](https://arxiv.org/pdf/1908.05647) -- Lean 4's RC foundations
- [Lean 4 Theorem Prover and Programming Language](https://pp.ipd.kit.edu/uploads/publikationen/demoura21lean4.pdf) -- RC in Lean 4
- [Reference Counting with Frame-Limited Reuse (ICFP 2022)](https://dl.acm.org/doi/10.1145/3547634) -- improved reuse analysis
- [Biased Reference Counting (PACT 2018)](https://dl.acm.org/doi/10.1145/3243176.3243195) -- multithreaded RC
- [Static Uniqueness Analysis for Lean 4 (Master's Thesis)](https://pp.ipd.kit.edu/uploads/publikationen/huisinga23masterarbeit.pdf) -- static ownership
- [PEP 703: Making the GIL Optional](https://peps.python.org/pep-0703/) -- CPython biased RC adoption
- [Optimizing Reference Counting with Borrowing (Master's Thesis)](https://antonlorenzen.de/master_thesis_perceus_borrowing.pdf) -- borrow optimization

### Perceus Algorithm

Perceus achieves "garbage-free" reference counting through three key innovations:

1. **Precise RC insertion.** `dup` (inc_ref) is delayed as late as possible (pushed to leaves of derivation trees); `drop` (dec_ref) is generated as early as possible (immediately after last use). This minimizes the window where RC operations are live.

2. **Reuse analysis.** When an object's reference count drops to zero, its memory can be _reused_ for the next allocation of the same size/type. The "resurrection hypothesis" (Lean 4): many objects die just before creating an object of the same kind. This is pervasive in:
   - Functional data structure updates (balanced trees, lists)
   - AST transformation passes (compilers, theorem provers)
   - Iterator state machines

3. **Drop specialization.** Instead of a generic `drop` that decrements and conditionally frees, Perceus generates _specialized_ drop functions per type that know the exact layout and can recursively drop children without dynamic dispatch.

### Lean 4's "Functional But In-Place" (FBIP)

Lean 4 extends Perceus with a programming paradigm where pure functional code achieves in-place mutation:

```
-- If `xs` has refcount 1, this is an in-place update
let ys = xs.push(42)  -- reuses xs's memory
```

The compiler statically detects reuse opportunities and emits destructive updates when the reference count is provably 1. Preliminary results show competitive performance with OCaml and GHC.

### Biased Reference Counting (BRC)

BRC optimizes multithreaded RC by observing that most objects are accessed by a single thread:

- Each object has an owner thread, a local counter, and a shared counter
- Owner thread: non-atomic increment/decrement of local counter
- Other threads: atomic CAS on shared counter
- Object freed when `local + shared == 0`

Performance: 2x faster RC operations in the common case; 22.5% average execution time reduction; 7.3% throughput improvement for server workloads.

**CPython adoption**: PEP 703 (free-threaded CPython) uses biased reference counting, validating the technique for Python runtimes.

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Perceus precise RC insertion | Very high -- Molt already uses RC | Medium (5-15%) | Medium |
| Reuse analysis | Very high -- eliminate malloc/free pairs | High (10-30%) | Medium |
| Drop specialization | High -- avoid generic drop dispatch | Medium (5-10%) | Low |
| FBIP (Lean-style in-place update) | High -- list/dict functional updates | High (10-40% for FP patterns) | High |
| Biased RC | Relevant for thread-safe code without GIL | High for multithreaded | Medium |
| Borrowing optimization | Very high -- eliminate redundant inc/dec pairs | High (10-20%) | Medium |

**Recommendation**: This is the area with the most direct applicability to Molt's existing architecture. Molt already uses RC (NaN-boxed objects with refcounting). The improvements should be layered:

1. **Borrowing analysis** (P0): Statically identify parameters and local variables that don't need inc_ref/dec_ref because they borrow from the caller. This is already partially done (`protect_callargs_aliased_return` fix) but should be systematized.

2. **Reuse analysis** (P1): When a `drop` immediately precedes an allocation of the same type/size, reuse the memory. This eliminates malloc/free pairs in hot loops (list comprehensions, map operations).

3. **Drop specialization** (P1): Generate per-type drop functions that know the exact layout. Avoid virtual dispatch on drop paths.

4. **Biased RC** (P2): When Molt supports free-threading, adopt biased RC to avoid atomic operations on the common (single-threaded) path.

---

## 9. Cache-Oblivious Algorithms

### Key Sources

- [Cache-Oblivious Algorithms (Algorithmica)](https://en.algorithmica.org/hpc/external-memory/oblivious/) -- practical guide
- [Cache-Oblivious Algorithms and Data Structures (Demaine)](https://cs.au.dk/~gerth/MassiveData02/notes/demaine.pdf) -- tutorial
- [Cache-Oblivious Algorithms and Data Structures (Brodal)](https://cs.au.dk/~gerth/slides/swat04invited.pdf) -- survey
- [Cache-Friendly Algorithms (AlgoCademy)](https://algocademy.com/blog/cache-friendly-algorithms-and-data-structures-optimizing-performance-through-efficient-memory-access/) -- practical guide
- [LOOPerSet: Large-Scale Dataset for Polyhedral Compilation (arXiv 2025)](https://arxiv.org/html/2510.10209) -- ML-driven loop optimization
- [Polyhedral Compilation](http://polyhedral.info/) -- comprehensive resource

### Core Techniques

**Cache-oblivious design principle.** Algorithms that perform well on arbitrary memory hierarchies without knowing cache parameters. The key technique is recursive divide-and-conquer: problems are split into subproblems until they fit in cache at _some_ level, without needing to know which level.

**Polyhedral loop optimization.** The state-of-the-art for loop nest optimization. Represents iteration spaces as polytopes, applies affine transformations (tiling, skewing, fusion, interchange) to maximize data locality and parallelism.

Key transformations:
- **Loop tiling**: Partition iteration space into blocks that fit in cache
- **Loop fusion**: Merge loops that access the same data to improve temporal locality
- **Loop interchange**: Reorder nested loops to improve spatial locality (row-major access)
- **Loop skewing**: Enable parallelism and tiling for loops with dependencies

**2024-2025 developments:**
- LOOPerSet (2025): A dataset of 28 million labeled data points for ML-driven polyhedral optimization search
- Modeling tiling + fusion interactions in optimizing compilers (ACM TCS 2024)

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Loop tiling in LIR | Medium -- mainly for numeric code | High for numeric (2-10x) | High |
| Loop fusion | Medium -- list comprehension chains | Medium (10-30% for chains) | Medium |
| Loop interchange | Low -- Python rarely has deep loop nests | Low-Medium | Medium |
| Data layout optimization | Medium -- tuple-of-arrays vs array-of-tuples | Medium (2-5x for columnar) | Medium |
| Cache-oblivious data structures | Low -- most Python objects are small | Low | High |

**Recommendation**: Full polyhedral optimization is too complex for Molt's current stage. However, two targeted optimizations are high-value:

1. **Loop fusion for comprehension chains**: When Python code chains list comprehensions or generator expressions, fuse them into a single loop to avoid materializing intermediate lists. This is a Python-specific win that most compilers miss.

2. **Loop interchange for nested iterations**: When Molt detects row-major vs column-major access patterns in numeric loops, reorder iterations. This is only relevant for the numeric/dataframe lane.

---

## 10. SIMD Auto-Vectorization

### Key Sources

- [Auto-Vectorization in LLVM](https://llvm.org/docs/Vectorizers.html) -- Loop and SLP vectorizer docs
- [VW-SLP: Auto-vectorization with Adaptive Vector Width (PACT 2018)](https://dl.acm.org/doi/10.1145/3243176.3243189) -- variable-width SLP
- [Vectorization in GCC (Red Hat 2023)](https://developers.redhat.com/articles/2023/12/08/vectorization-optimization-gcc) -- practical guide
- [Compiler Auto-Vectorization with Imitation Learning (NeurIPS 2019)](https://charithmendis.com/assets/pdf/neurips19-vemal.pdf) -- ML-driven vectorization
- [Graph-Based Learning for Loop Auto-Vectorization (2024)](https://spj.science.org/doi/10.34133/icomputing.0113) -- GNN for SLP packing
- [Cranelift SIMD Progress Report (April 2024)](https://bjorn3.github.io/2024/04/06/progress-report-april-2024.html) -- Cranelift SIMD status
- [Prospero with Cranelift JIT and SIMD](https://whtwnd.com/aviva.gay/3ll5dbsng3v26) -- practical Cranelift SIMD usage

### Core Vectorization Approaches

**Loop vectorization.** Transforms scalar loops into vector loops operating on multiple elements per iteration. Requires:
- Independence analysis (no loop-carried dependencies)
- Cost model (is vectorization profitable at this width?)
- Remainder handling (scalar epilogue for non-divisible trip counts)

**SLP (Superword-Level Parallelism) vectorization.** Combines similar independent instructions into vector instructions within a basic block. Does not require loops -- works on straight-line code.

LLVM's SLP vectorizer:
1. Identifies seed groups (consecutive memory accesses)
2. Builds a dependency graph of operations
3. Packs independent isomorphic operations into vector instructions
4. Uses a cost model to decide if packing is profitable

**VW-SLP (Variable-Width SLP).** Adapts vector width at instruction granularity instead of fixing a single width for the entire block. This captures more parallelism when operations have mixed widths.

### Cranelift SIMD Status (2024-2025)

- x86-64 and aarch64 SIMD fully supported
- Core SIMD operations available but **no auto-vectorization**
- Missing: SVE (Arm Scalable Vector Extensions), auto-vectorization pass
- Platform-specific vendor intrinsics partially implemented

### Applicability to Molt

| Technique | Molt Relevance | Impact | Complexity |
|-----------|---------------|--------|------------|
| Pre-Cranelift loop vectorization in LIR | High -- Cranelift won't do it | Very High (2-8x for numeric) | Very High |
| SLP vectorization in LIR | Medium -- less common in Python | Medium (1.5-3x) | High |
| Explicit SIMD intrinsics in stdlib | High -- numpy-like operations | Very High (4-16x) | Medium |
| Cranelift SIMD type emission | High -- emit i32x4, f64x2 etc in LIR | High (2-8x) | Medium |

**Recommendation**: Since Cranelift has no auto-vectorizer, Molt must do vectorization _before_ emitting Cranelift IR. Two approaches:

1. **Explicit SIMD emission for known patterns** (P1): When Molt recognizes numeric patterns (sum of list, element-wise operations, dot product), emit SIMD Cranelift IR directly using i32x4, f64x2, etc. This is pattern-matching, not general auto-vectorization, and is much simpler.

2. **General loop vectorization in LIR** (P3): Full auto-vectorization is a massive engineering effort. Defer this until Molt has a numeric computing story. The explicit pattern approach covers 80% of cases with 10% of the effort.

---

## Summary: Priority-Ranked Optimization Opportunities

### P0 -- Highest Impact, Foundation Work

| Optimization | Source | Expected Impact | Complexity | Target Layer |
|-------------|--------|----------------|------------|-------------|
| Partial Escape Analysis | GraalVM (Section 4, 7) | 15-40% alloc reduction | High | TIR |
| Borrowing analysis for RC | Perceus/Lean 4 (Section 8) | 10-20% fewer RC ops | Medium | TIR/LIR |
| Linear allocation removal | PyPy (Section 3) | 10-40% on hot paths | Medium | TIR |

### P1 -- High Impact, Medium Effort

| Optimization | Source | Expected Impact | Complexity | Target Layer |
|-------------|--------|----------------|------------|-------------|
| Reuse analysis (malloc/free elision) | Perceus (Section 8) | 10-30% alloc cost | Medium | LIR/Runtime |
| Allocation sinking | LuaJIT (Section 6) | 10-100x on FP micro | High | LIR |
| Drop specialization | Perceus (Section 8) | 5-10% drop path | Low | Codegen |
| Explicit SIMD for known patterns | LLVM/Cranelift (Section 10) | 2-8x for numeric | Medium | LIR |
| Pre-Cranelift loop unrolling | Cranelift gap (Section 2) | 5-15% loop perf | Low | LIR |

### P2 -- Medium Impact, Strategic

| Optimization | Source | Expected Impact | Complexity | Target Layer |
|-------------|--------|----------------|------------|-------------|
| Integer range/bit analysis | PyPy (Section 3) | 5-10% bounds elim | Low | TIR |
| Loop fusion for comprehension chains | Cache-oblivious (Section 9) | 10-30% for chains | Medium | TIR |
| IPEA-guided inlining | GraalVM (Section 4) | 5-15% alloc reduction | High | TIR |
| Polymorphic inline caches | V8 (Section 5) | 5-15% for dynamic | Medium | Runtime |
| FBIP (in-place functional update) | Lean 4 (Section 8) | 10-40% for FP | High | TIR/Runtime |
| Biased reference counting | BRC (Section 8) | 22% for multithreaded | Medium | Runtime |

### P3 -- Long-Term / Research

| Optimization | Source | Expected Impact | Complexity | Target Layer |
|-------------|--------|----------------|------------|-------------|
| General loop auto-vectorization | LLVM (Section 10) | 2-8x numeric | Very High | LIR |
| Polyhedral loop optimization | Cache-oblivious (Section 9) | 2-10x numeric | Very High | LIR |
| MLIR adoption for GPU lane | MLIR (Section 1) | Enables GPU codegen | Very High | New backend |
| Transform dialect auto-tuning | MLIR (Section 1) | Better pass ordering | High | TIR/LIR |
| Luau backend co-optimization | Luau (Section 6) | Better Luau codegen | Medium | Luau backend |

---

## Cross-Cutting Themes

### 1. Allocation elimination is the single highest-impact optimization for Python

Every high-performance dynamic language runtime (PyPy, GraalVM, LuaJIT, V8) prioritizes allocation elimination. Python creates orders of magnitude more temporary objects than static languages. The combination of PEA + allocation sinking + reuse analysis could eliminate 50-80% of allocations in typical Python code.

### 2. Molt's AOT advantage

Unlike JIT compilers (which must compile quickly and only optimize hot code), Molt can spend more time on optimization. This means we can afford analyses that JITs skip: full interprocedural escape analysis, aggressive monomorphization, whole-program reuse analysis.

### 3. Pre-Cranelift optimization is the right strategy

Since Cranelift deliberately trades codegen quality for compile speed, the optimization burden falls on Molt's TIR and LIR passes. This is actually a good architecture: Molt can implement Python-specific optimizations (PEA for tuples, comprehension fusion, iterator devirtualization) that a general-purpose backend like Cranelift or LLVM would never implement.

### 4. Reference counting is not a disadvantage -- it's an optimization opportunity

With Perceus-style precise RC + borrowing + reuse analysis, RC becomes an advantage over tracing GC: deterministic deallocation enables reuse analysis (impossible with tracing GC), borrowing eliminates RC overhead for most operations, and there are no GC pauses.

### 5. The Luau backend needs co-optimization

Since Molt targets Luau, we should ensure emitted Luau code is amenable to Luau's optimizer: emit type annotations, use patterns the fastcall mechanism recognizes, and structure closures to benefit from upvalue optimization.
