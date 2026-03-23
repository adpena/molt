# Project TITAN — Total Infrastructure for Transformative Acceleration of Numeric and General Python

**Date:** 2026-03-23
**Status:** Draft
**Scope:** Comprehensive optimization plan to beat CPython on all benchmarks, match/beat Nuitka, and approach/beat Codon

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current State Analysis](#2-current-state-analysis)
3. [Competitive Landscape](#3-competitive-landscape)
4. [Architecture: Typed IR (TIR)](#4-architecture-typed-ir-tir)
5. [Phase 0: Beat CPython on All Benchmarks](#5-phase-0-beat-cpython-on-all-benchmarks)
6. [Phase 1: Foundations](#6-phase-1-foundations)
7. [Phase 2: Optimization Core](#7-phase-2-optimization-core)
8. [Phase 3: Integration + PGO](#8-phase-3-integration--pgo)
9. [Phase 4: WASM + Parallelism + SIMD](#9-phase-4-wasm--parallelism--simd)
10. [Phase 5: GPU + MLIR Foundation](#10-phase-5-gpu--mlir-foundation)
11. [Phase 6: Frontier](#11-phase-6-frontier)
12. [Verification & Harness Engineering](#12-verification--harness-engineering)
13. [Dependency Graph](#13-dependency-graph)
14. [Risk Registry](#14-risk-registry)
15. [Success Criteria](#15-success-criteria)

---

## 1. Executive Summary

Molt is a Python-to-native/WASM/Luau AOT compiler written in Rust. It currently has a sophisticated frontend with SCCP, CSE, LICM, edge threading, and guard hoisting, plus three backends (Cranelift native, WASM via wasm-encoder, Luau transpilation), 2,193 runtime intrinsics, NaN-boxing with 47-bit inline ints, and 61+ benchmarks with differential testing against CPython.

**Current performance:**
- Near Codon parity on `sum.py` (0.0113s vs 0.0115s)
- 2.33x slower than Codon on `word_count.py` (string/dict hot paths)
- 3.93x slower than Codon on `taq.py` (data pipeline)
- Several benchmarks still slower than CPython

**Target performance:**
- Faster than CPython on ALL 61+ benchmarks
- Faster than Nuitka on ALL benchmarks (Nuitka achieves 2-4x over CPython typical)
- Match or beat Codon on compute-heavy benchmarks (Codon achieves 10-100x over CPython)
- GPU compute competitive with native Metal/CUDA implementations

**Strategy:** Approach 2 (Critical Path) with MLIR groundwork — maximize parallel workstreams, deliver wins early, build toward MLIR progressive lowering. Keep Cranelift for dev velocity, add LLVM for release-mode maximum optimization.

---

## 1.5 CPython Parity Contract and Optimization Boundaries

### 1.5.1 The Parity Contract

Molt guarantees deterministic behavioral parity with CPython >= 3.12 for all supported constructs. For any program in the supported subset, Molt's output must be byte-identical to CPython's, or the difference must be documented and covered by an explicit exception.

**Tier 1 — Full parity guaranteed (supported subset):**
- All arithmetic, comparison, boolean, bitwise operations (including arbitrary-precision int)
- All control flow: if/elif/else, for, while, break, continue, try/except/finally, with
- All data types: int, float, str, bytes, bytearray, bool, None, list, dict, set, tuple, range, slice, memoryview, frozenset
- All comprehensions: list, dict, set, generator expressions
- All function features: *args, **kwargs, defaults, closures, decorators, generators, async/await
- All class features: inheritance, MRO, descriptors, properties, staticmethod, classmethod, __slots__, dataclasses
- All string operations, formatting (f-strings, .format, %), encoding/decoding
- All supported stdlib modules (see stdlib coverage matrix)

**Tier 2 — Same observable behavior, different implementation (optimization-permitted deviations):**
- `id(x)` may differ (stack-allocated objects have different addresses)
- `sys.getrefcount()` may differ (biased refcounting, elided refcounts)
- GC timing may differ (cycle collector runs at different thresholds)
- `__del__` ordering may differ (ASAP destruction vs scope-end)
- `repr()` of objects may include different memory addresses
- Exception traceback line numbers may differ slightly (optimized code reordering)

**Tier 2.5 — Opt-in floating-point deviations (`@fast_math` only):**
- FMA contraction: `a*b + c` may have different rounding than separate mul+add
- Reassociation: `(a+b)+c` may differ from `a+(b+c)`
- NaN/Inf handling may be simplified
- These ONLY apply to functions explicitly decorated with `@fast_math`

**Tier 3 — Intentionally unsupported (known exclusions):**
- `exec()`, `eval()`, `compile()` — no runtime code generation
- Runtime monkeypatching of builtins or class methods after compilation
- Unrestricted reflection (`inspect.getsource`, `frame.f_locals` mutation)
- `__import__` hooks, import system customization beyond standard paths
- `ctypes` / `cffi` (C extension FFI)

### 1.5.2 Parity Enforcement Harness

Automated enforcement — no optimization may silently break parity:

```python
class ParityGate:
    """
    Runs on every PR. Blocks merge on any Tier 1 violation.

    Three tiers of comparison:
    1. STRICT (Tier 1): output must be byte-identical to CPython
    2. RELAXED (Tier 2): output semantically equivalent after normalization
       (strip object addresses, normalize refcount values)
    3. EXCLUDED (Tier 3): expected divergence, skip comparison
    """

    def run_parity_check(self, test_file: str) -> ParityResult:
        tier = self.classify_test(test_file)
        cpython_output = run_cpython(test_file)
        molt_output = run_molt(test_file)

        if tier == Tier.STRICT:
            if cpython_output != molt_output:
                return ParityResult.VIOLATION(
                    test=test_file, tier=tier,
                    diff=unified_diff(cpython_output, molt_output),
                )
        elif tier == Tier.RELAXED:
            if self.normalize(cpython_output) != self.normalize(molt_output):
                return ParityResult.VIOLATION(...)
        elif tier == Tier.EXCLUDED:
            return ParityResult.EXPECTED_DIVERGENCE(test=test_file)
        return ParityResult.PASS(test=test_file, tier=tier)

    def normalize(self, output: str) -> str:
        output = re.sub(r'0x[0-9a-f]+', '0xADDR', output)
        output = re.sub(r'refcount: \d+', 'refcount: N', output)
        return output

    def classify_test(self, test_file: str) -> Tier:
        content = open(test_file).read()
        if '# molt-parity: excluded' in content:
            return Tier.EXCLUDED
        elif '# molt-parity: relaxed' in content:
            return Tier.RELAXED
        return Tier.STRICT  # default: strictest parity
```

### 1.5.3 Optimization Boundary Markers

Every TIR optimization pass declares its safety level:

```rust
enum OptSafety {
    /// Cannot change observable output (e.g., dead code elimination)
    Transparent,
    /// May change non-semantic observables (e.g., object identity)
    SemanticallyEquivalent { deviations: Vec<Tier2Deviation> },
    /// May change FP results, only when @fast_math active
    FastMathOnly,
}
```

Integration with TIR passes — in debug builds, every pass is wrapped:
```rust
fn run_pass_with_parity_check(pass_impl: &dyn TirPass, func: &mut TirFunction) {
    let snapshot = func.clone();
    pass_impl.run(func);
    pass_impl.verify_invariants(func);
    let before = interpret_tir(&snapshot, &test_inputs());
    let after = interpret_tir(func, &test_inputs());
    assert_eq!(before, after, "Pass {} violated parity", pass_impl.name());
}
```

### 1.5.4 Parity Report

```bash
molt compile --parity-report app.py
# Parity Report for app.py:
# Tier 1 (full parity):      142 constructs  (98.6%)
# Tier 2 (semantic equiv):     2 constructs  (1.4%)
#   - Line 45: id() on stack-allocated object
#   - Line 89: sys.getrefcount() with biased refcount
# Tier 3 (unsupported):        0 constructs  (0.0%)
# Overall: COMPATIBLE
```

No silent degradation. Every deviation documented, enforced by CI, reported at compile time.

---

## 2. Current State Analysis

### 2.1 What Molt Has

**Frontend (Python → SimpleIR):**
- `src/molt/frontend/__init__.py` (36,665 lines)
- Mid-end passes: SCCP (queue-driven), CSE (alias-aware), LICM (affine reasoning), edge threading, guard hoisting/elimination, dead code elimination
- Tier classification: A (aggressive, 180ms), B (balanced, 110ms), C (light, 70ms)
- Type facts collection from annotations (`src/molt/type_facts.py`)

**Native Backend (Cranelift):**
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (13,669 lines)
- Cranelift 0.128 JIT to ARM64/x86-64
- NaN-boxing: `QNAN | TAG | payload` with 6 tags (INT, BOOL, NONE, PTR, PENDING, raw float)
- 47-bit inline ints, heap-backed for larger
- Cold block marking (36+ slow paths), `MemFlags::trusted()` on 34+ sites
- CPU feature auto-detection (AVX2/SSE4.2/NEON)
- SIMD expansion for 20+ string/bytes/float operations

**WASM Backend:**
- `runtime/molt-backend/src/wasm.rs` (13,021 lines)
- 35+ static types, native EH groundwork (`try_table`/`throw`/`catch`)
- Constant folding, local coalescing, box/unbox elimination, `br_table` dispatch
- Tree shaking (sentinel unused builtins), multi-value return type signatures (not wired)
- wasm-opt Oz/O3 pipelines, `memory.fill`/`memory.copy` intrinsics

**Luau Backend:**
- `runtime/molt-backend/src/luau.rs` (5,619 lines)
- 9 text-level optimization passes (constant propagation, copy propagation, dead stores, etc.)

**Runtime:**
- `runtime/molt-runtime/` — 40-byte object header, atomic u32 refcounts
- Object pool (3 types, max 1024 entries), weak references
- 2,193 intrinsics (1,838 Python-wired, 355 internal)
- SIMD-accelerated string ops, crypto primitives, ETL fused primitives

**IR Passes (backend level):**
- `elide_dead_struct_allocs` — removes unused class/struct allocations
- `inline_functions` — inlines functions ≤30 ops with no control flow
- `apply_profile_order` — PGO-based function ordering
- `tree_shake_luau` — removes unused functions for Luau

**Benchmark Infrastructure:**
- 61+ benchmarks in `tests/benchmarks/`
- `tools/bench.py` (native), `tools/bench_wasm.py` (WASM)
- Comparators: CPython, PyPy, Codon, Nuitka, Pyodide
- `tools/bench_diff.py` for regression detection
- Differential testing: `tests/molt_diff.py` with 100+ test files

### 2.2 Critical Gaps

| Gap | Impact | Root Cause |
|-----|--------|------------|
| No intermediate typed IR between SimpleIR and backends | Type hints collected but never used for codegen | Single IR level |
| No LLVM backend | Cannot access -O3, vectorization, PGO, BOLT | Only Cranelift |
| No escape analysis in backend | All objects heap-allocated, no stack allocation | No dataflow analysis post-frontend |
| No CFG-based loop optimization in backend | Cranelift can't do LICM across SimpleIR | SimpleIR is linear, not CFG |
| 40-byte object header | Cache pressure, cold fields bloat hot path | Monolithic header design |
| No cycle collection | Memory leaks on cyclic structures | Pure refcounting |
| Type hints not propagated to codegen | `type_hint` field in SimpleIR never populated from source annotations | Pipeline gap |
| No parallelism (compile or runtime) | Single-threaded everything | No threading infrastructure |
| No GPU support | Can't compete on compute workloads | No GPU backend |
| No auto-vectorization | Only manual SIMD intrinsics in runtime | No loop vectorizer |
| No deforestation/iterator fusion | Intermediate objects created for map/filter chains | No fusion pass |
| No class hierarchy analysis | Method calls go through dict lookup | No whole-program analysis |
| No bounds check elimination | Every list access checks bounds | No range analysis |
| No inline caching for attributes | `LOAD_ATTR` goes through full dict lookup | No IC infrastructure |
| Egraph simplification exists but not integrated | Dead code in `egraph_simplify.rs` | Feature-gated, not wired |
| No incremental compilation | Full recompile on every change | No caching |
| No string representation optimization | Single string type, always heap-allocated | No SSO, no interning |

---

## 3. Competitive Landscape

### 3.1 Nuitka

**Architecture:** Python AST → Nuitka node tree → C source → GCC/Clang → native binary.

**Key techniques:**
- SSA-based value tracing with iterative convergence
- Type shape inference (three-state slot knowledge)
- Language reformulation (for→while, with→try/finally, assert→if/raise)
- Built-in call prediction (bypass `PyObject_Call`)
- LTO + PGO pipeline (5-30% additional speedup)

**Performance:** 2-4x over CPython typical, up to 11.2x on numeric loops. Pystone: 3.35-3.72x.

**Fundamental limitation:** Trapped in `PyObject *` boxing — links against libpython, cannot truly unbox values. Reported to have elevated LLC miss rates on some benchmarks due to pointer indirection (source: empirical study on compiled Python performance, arxiv 2505.02346).

**What Molt can learn:** Language reformulation, iterative pass convergence, PGO pipeline.
**Where Molt should exceed:** Unboxed native types, SIMD, GPU, no libpython dependency.

### 3.2 Codon

**Architecture:** Python → custom PEG parser → Hindley-Milner type inference → CIR → 50+ optimization passes → LLVM IR → LLVM -O3 + custom passes → native binary.

**Key techniques:**
- Full monomorphization — every generic instantiated per-type
- All types resolved at compile time — `int` is bare `i64`, `float` is bare `f64`
- Tuples as LLVM structs passed by value (often optimized away entirely)
- Custom LLVM passes: AllocationRemover (heap→stack), CoroutineElider (generator inlining)
- Forked LLVM with enhanced coroutine escape analysis
- Boehm-Demers-Weiser conservative GC (no refcounting)
- OpenMP parallelism (no GIL), GPU via CUDA/PTX
- CIR preserves structured control flow (IfFlow, ForFlow) for high-level pattern matching

**Performance:** 10-100x over CPython. On par with C/C++ for many workloads. Near-parity on `sum.py` with Molt.

**Fundamental limitation:** Not a CPython drop-in replacement. No arbitrary-precision ints, no runtime polymorphism, no C extension support.

**What Molt can learn:** LLVM custom passes (AllocationRemover, CoroutineElider), monomorphization, CIR-level pattern matching, GPU programming model.
**Where Molt should exceed:** CPython compatibility, WASM deployment, broader stdlib support.

### 3.3 Mojo

**Architecture:** Mojo source → MLIR (custom Mojo dialects) → progressive lowering through Affine → SCF → CF → LLVM dialect → LLVM IR → native binary.

**Key techniques:**
- Built entirely on MLIR — enables higher-level optimization than LLVM alone
- Value semantics by default, ownership model (no GC)
- ASAP destruction (destroy after last use, not at scope end)
- SIMD as first-class citizen in type system
- Parametric metaprogramming for hardware specialization
- Portable GPU programming (NVIDIA + AMD) via MLIR
- Progressive lowering enables domain-specific optimization at each level

**Performance:** 46x over Python from static types + compilation alone. 68,000x with SIMD + parallelism.

**What Molt can learn:** MLIR progressive lowering architecture, value semantics for performance, SIMD-first design, ASAP destruction policy.

### 3.4 CPython 3.12-3.14

**Key optimizations to reference:**
- Specializing adaptive interpreter (PEP 659): `LOAD_GLOBAL_MODULE` (98% hit), `STORE_ATTR_INSTANCE_VALUE` (91%), `LOAD_ATTR_INSTANCE_VALUE` (81%)
- Inline caching: 16-bit entries embedded in bytecode array
- Copy-and-patch JIT (3.13+): LLVM stencils at build time, patched at runtime
- Free-threaded (no-GIL): biased refcounting, per-object locking
- Tier 2 micro-ops: type propagation, guard elimination, refcount elimination

**What Molt should match:** Every CPython specialization should have an equivalent or better fast path in compiled Molt code.

---

## 4. Architecture: Typed IR (TIR)

### 4.1 Purpose

TIR is the central intermediate representation that sits between the Python frontend's SimpleIR and all backends. It is:
- **Typed:** Every value carries a resolved or partially-resolved type
- **SSA form:** Static single assignment with basic block arguments (MLIR-style, no phi nodes)
- **CFG-based:** Explicit basic blocks with typed terminators
- **MLIR-compatible:** Every operation, type, and region maps 1:1 to MLIR concepts
- **Backend-agnostic:** Consumed by Cranelift, LLVM, WASM, and GPU backends identically

### 4.2 Data Structures

```rust
/// A TIR module — the top-level compilation unit
struct TirModule {
    name: String,
    functions: Vec<TirFunction>,
    globals: Vec<TirGlobal>,
    class_hierarchy: ClassHierarchy,     // whole-program class graph
    interned_strings: StringPool,         // shared interning table
    type_registry: TypeRegistry,          // all resolved types
}

/// A TIR function — one per Python function + specializations
struct TirFunction {
    name: String,
    signature: FuncSignature,             // parameter types + return type
    blocks: Vec<TirBlock>,               // basic blocks
    entry_block: BlockId,
    local_types: Vec<TirType>,           // resolved types for all locals
    escape_info: Option<EscapeInfo>,      // populated by escape analysis
    inline_cost: u32,                     // estimated inlining cost
    is_specialization: bool,              // true if monomorphized copy
    source_span: SourceSpan,             // for debug info / source maps
    pgo_data: Option<PgoFunctionData>,   // profile-guided data if available
}

/// A basic block with typed arguments (MLIR-style)
struct TirBlock {
    id: BlockId,
    args: Vec<TirValue>,                 // block arguments replace phi nodes
    ops: Vec<TirOp>,                     // operations in order
    terminator: Terminator,              // branch, cond_branch, return, switch, unreachable
    loop_depth: u32,                     // nesting depth (0 = not in loop)
    is_cold: bool,                       // exception handlers, error paths
    frequency: Option<f64>,              // PGO execution frequency
}

/// A single TIR operation (MLIR-compatible structure)
struct TirOp {
    dialect: Dialect,                    // molt, scf, llvm, gpu, par
    opcode: OpCode,                      // operation within the dialect
    operands: Vec<TirValue>,             // typed SSA input values
    results: Vec<TirValue>,             // typed SSA output values
    regions: Vec<TirRegion>,            // nested control flow (if/loop bodies)
    attrs: AttrDict,                     // metadata (alignment, noalias, fast_math, etc.)
    source_span: Option<SourceSpan>,    // Python source location
}

/// Typed SSA value
struct TirValue {
    id: ValueId,
    ty: TirType,
}

/// Type system — designed for progressive refinement
enum TirType {
    // Unboxed scalar types (register-resident, no heap allocation)
    I64,                                  // Python int (fits in 64 bits)
    F64,                                  // Python float
    Bool,                                 // Python bool
    None,                                 // Python None singleton

    // Unboxed aggregate types (stack-resident or register-decomposed)
    Tuple(Vec<TirType>),                  // fixed-length typed tuple
    Struct(StructId),                     // user-defined class with known layout

    // Reference types (heap-resident, refcounted)
    Str(StrRepr),                         // string with known representation
    List(Box<TirType>),                   // typed list (backing store is T[])
    Dict(Box<TirType>, Box<TirType>),    // typed dict
    Set(Box<TirType>),                    // typed set
    Bytes,                                // bytes object
    Ptr(Box<TirType>),                   // raw typed pointer

    // Boxed types (NaN-boxed, dynamic)
    Box(Box<TirType>),                   // NaN-boxed with known inner type
    DynBox,                               // NaN-boxed, type unknown at compile time

    // Function types
    Func(FuncSignature),                  // known function signature
    Closure(FuncSignature, Vec<TirType>), // closure with captured env types

    // Special
    BigInt,                               // arbitrary-precision integer (heap)
    Union(Vec<TirType>),                  // union of possible types
    Never,                                // bottom type (unreachable)
}

/// String representation variants
enum StrRepr {
    Inline,        // ≤23 bytes, stored in value slot
    OneByte,       // ASCII-only, 1 byte per char
    TwoByte,       // BMP, 2 bytes per char
    General,       // unknown representation
}

/// Escape analysis result per allocation
enum EscapeState {
    NoEscape,      // never leaves function → stack allocate
    ArgEscape,     // passed to callee but not stored → stack + callee lifetime
    GlobalEscape,  // stored in heap/global → must heap allocate
}

/// Region (MLIR-compatible nested control flow)
struct TirRegion {
    blocks: Vec<TirBlock>,
    entry_block: BlockId,
}

/// Terminators
enum Terminator {
    Branch(BlockId, Vec<TirValue>),                          // unconditional
    CondBranch(TirValue, BlockId, Vec<TirValue>, BlockId, Vec<TirValue>), // if/else
    Switch(TirValue, Vec<(i64, BlockId)>, BlockId),          // jump table
    Return(Vec<TirValue>),
    Unreachable,
    Deopt(DeoptInfo),                                         // speculative deoptimization
}

/// Deoptimization metadata (for speculative unboxing)
struct DeoptInfo {
    fallback_func: FuncId,               // generic version to transfer to
    live_values: Vec<(TirValue, VarId)>, // SSA values to materialize in fallback
    reason: DeoptReason,
}
```

### 4.3 Dialect Design (MLIR-Compatible)

```
molt dialect (core Python operations):
  molt.box          : (T) -> DynBox               // NaN-box a typed value
  molt.unbox        : (DynBox) -> T               // unbox with type assertion
  molt.type_guard   : (DynBox, TypeId) -> T       // guarded unbox (deopt on mismatch)
  molt.call         : (Func, args...) -> results   // function call
  molt.call_method  : (obj, method_name, args...) -> results
  molt.alloc        : (TypeId) -> Ptr<T>          // heap allocation
  molt.stack_alloc  : (TypeId) -> Ptr<T>          // stack allocation (escape=NoEscape)
  molt.load_attr    : (Ptr<T>, attr_name) -> V    // attribute load
  molt.store_attr   : (Ptr<T>, attr_name, V) -> () // attribute store
  molt.ic_lookup    : (obj, cache_id) -> (V, hit) // inline cache probe
  molt.inc_ref      : (Ptr<T>) -> ()              // reference count increment
  molt.dec_ref      : (Ptr<T>) -> ()              // reference count decrement
  molt.deopt        : (reason) -> Never           // deoptimize to generic version

molt.scf dialect (structured control flow, maps to MLIR scf):
  molt.scf.if       : (cond) -> results  { then_region, else_region }
  molt.scf.for      : (lb, ub, step, iter_args...) -> results { body_region }
  molt.scf.while    : (operands...) -> results { cond_region, body_region }
  molt.scf.yield    : (results...) -> ()         // region terminator

molt.gpu dialect (GPU compute):
  molt.gpu.launch   : (grid, block, args...) -> () { kernel_region }
  molt.gpu.thread_id: (dim: u32) -> I64
  molt.gpu.block_id : (dim: u32) -> I64
  molt.gpu.block_dim: (dim: u32) -> I64
  molt.gpu.grid_dim : (dim: u32) -> I64
  molt.gpu.barrier  : () -> ()                    // threadgroup sync
  molt.gpu.shared   : (size: I64) -> Ptr<T>      // threadgroup memory
  molt.gpu.atomic_add: (Ptr<T>, T) -> T

molt.par dialect (parallelism):
  molt.par.parallel_for : (lb, ub, step, schedule, args...) -> results { body }
  molt.par.reduce       : (init, args...) -> T { combiner_region }
  molt.par.critical     : () -> () { body_region }

molt.simd dialect (explicit SIMD):
  molt.simd.load    : (Ptr<T>, width) -> Vec<T>
  molt.simd.store   : (Vec<T>, Ptr<T>) -> ()
  molt.simd.splat   : (T, width) -> Vec<T>
  molt.simd.reduce_add : (Vec<T>) -> T
  molt.simd.fma     : (Vec<T>, Vec<T>, Vec<T>) -> Vec<T>
```

### 4.4 TIR Construction Pipeline

```
SimpleIR (linear, untyped ops)
    │
    ├─[4.4.1] CFG Extraction ──────────────────────────────────────────
    │   • Identify basic block boundaries (branch targets, exception handlers)
    │   • Build predecessor/successor maps
    │   • Compute dominator tree and dominance frontiers
    │   • Identify natural loops (back edges → loop headers)
    │   • Compute loop nesting tree with trip count estimates
    │
    ├─[4.4.2] SSA Conversion ──────────────────────────────────────────
    │   • Insert block arguments at join points (MLIR-style, no phi nodes)
    │   • Rename variables to SSA values with unique IDs
    │   • Thread block arguments through branch terminators
    │   • Verify SSA invariant: every use dominated by exactly one def
    │
    ├─[4.4.3] Type Refinement ─────────────────────────────────────────
    │   Sources (in priority order):
    │   1. Explicit annotations: def f(x: int) -> int
    │   2. Inferred from operations: x + 1 → x: I64
    │   3. Inferred from assignments: x = 0 → x: I64
    │   4. Inferred from control flow: isinstance(x, int) → x: I64 in true branch
    │   5. Inferred from containers: xs: list[int] → xs[i]: I64
    │   6. PGO profile data: 95% int → speculate I64 with guard
    │
    │   Algorithm: forward dataflow with type lattice meet:
    │     I64 ∧ I64 = I64
    │     I64 ∧ F64 = Union(I64, F64)
    │     I64 ∧ Str = Union(I64, Str)
    │     T ∧ Never = T
    │     T ∧ DynBox = DynBox
    │
    │   Union collapse policy (prevents specialization explosion):
    │     - Unions of ≤ 3 concrete types are preserved → guarded specialization
    │       e.g., Union(I64, F64) → emit: if I64 { fast } else if F64 { fast } else { slow }
    │     - Unions of > 3 types collapse to DynBox → generic dispatch
    │       e.g., Union(I64, F64, Str, Bytes, List) → DynBox
    │     - Unions containing only numeric types (I64, F64, Bool) always preserved
    │       (numeric operations are the highest-value fast paths)
    │     - Rationale: each union arm generates a specialized code path; beyond 3
    │       arms the code size / i-cache cost exceeds the dispatch savings
    │
    │   Iterates to fixpoint (max 20 iterations with conservative fallback)
    │
    ├─[4.4.4] Dialect Assignment ──────────────────────────────────────
    │   • Map SimpleIR ops to molt dialect operations
    │   • Identify structured control flow → molt.scf ops
    │   • Identify parallel-safe loops → molt.par candidates
    │   • Identify GPU-annotated functions → molt.gpu ops
    │   • Assign SIMD-eligible operations → molt.simd candidates
    │
    └─► TIR (typed, SSA, basic blocks, MLIR-compatible)
```

### 4.5 TIR Optimization Passes (in order)

```
Pass 1: Type Refinement (iterative to fixpoint)
    │   Input:  TIR with DynBox for most values
    │   Output: TIR with concrete types where provable
    │   Feeds:  All subsequent passes (types enable everything)
    │
Pass 2: Unboxing
    │   Input:  TIR with Box(I64), Box(F64) etc.
    │   Output: TIR with bare I64, F64 where all consumers accept unboxed
    │   Rule:   If all uses of molt.box(x: I64) extract via molt.unbox → I64,
    │           replace with direct I64 value, remove box/unbox pair
    │
Pass 3: Escape Analysis
    │   Input:  TIR with molt.alloc operations
    │   Output: TIR with EscapeState annotations on each allocation
    │   Algorithm: Interprocedural points-to analysis:
    │     - NoEscape: value not stored to heap, not returned, not passed to unknown
    │     - ArgEscape: passed to known callee that doesn't store it
    │     - GlobalEscape: stored to heap field, returned, or passed to unknown
    │   Action: NoEscape allocations → molt.stack_alloc
    │           NoEscape values → remove inc_ref/dec_ref
    │
Pass 4: Alias Analysis
    │   Input:  TIR with typed pointers
    │   Output: Alias sets per memory access
    │   Rules:  I64* cannot alias Str*
    │           List(I64)* data cannot alias Dict* data
    │           Stack allocations cannot alias heap allocations
    │   Feeds:  LICM (safe to hoist if no alias), GVN (safe to eliminate if no alias)
    │
Pass 5: SCCP (Sparse Conditional Constant Propagation)
    │   Input:  Typed SSA TIR
    │   Output: Constants propagated, dead branches eliminated
    │   Enhancement over frontend SCCP: operates on typed values,
    │   can fold typed operations (I64 + I64 = known I64 constant)
    │
Pass 6: GVN (Global Value Numbering)
    │   Input:  TIR with potential redundant loads/computations
    │   Output: Redundant values eliminated, loads CSE'd
    │   Uses:   Alias analysis to prove loads are redundant
    │
Pass 7: LICM (Loop-Invariant Code Motion)
    │   Input:  TIR with loop nesting tree
    │   Output: Invariant computations hoisted above loop headers
    │   Key:    Type guards hoisted (if type is loop-invariant)
    │           Attribute loads hoisted (if object not mutated in loop)
    │           Box/unbox hoisted (unbox before loop, box after)
    │
Pass 8: Bounds Check Elimination
    │   Input:  TIR with list/tuple access operations
    │   Output: Bounds checks removed where provably safe
    │   Techniques:
    │     - Range analysis: compute [min, max] of index expressions
    │     - Loop predication: hoist check before loop when trip count known
    │     - Pre/main/post splitting: safe iterations in check-free main loop
    │     - Dominator-based: if access at [i+1] succeeded, [i] is safe
    │
Pass 9: Deforestation / Iterator Fusion
    │   Input:  TIR with chains of generator/iterator operations
    │   Output: Fused single-loop equivalents
    │   Patterns:
    │     sum(x*x for x in data if x > 0)
    │       → for x in data: if x > 0: acc += x*x
    │     [f(x) for x in data]
    │       → preallocate list; for x in data: list.push(f(x))
    │     map(g, filter(f, data))
    │       → for x in data: if f(x): yield g(x)
    │   Eliminates: intermediate generator objects, per-element function calls,
    │               intermediate list allocations
    │
    │   **Purity precondition (critical for correctness):**
    │     Fusion is ONLY valid when the fused operations preserve observable behavior.
    │     Before fusing, verify:
    │     1. `is_pure(f)` — function has no side effects (no IO, no mutation of
    │        external state, no exceptions beyond what the unfused version would raise)
    │     2. Known-pure builtins: abs, len, min, max, int, float, str, bool, hash,
    │        isinstance, issubclass, id, type, ord, chr, hex, oct, bin, round
    │     3. User functions: conservatively assume impure UNLESS:
    │        a. Function body contains only pure operations (arithmetic, comparison,
    │           attribute reads on immutable objects)
    │        b. Function is annotated @pure (opt-in)
    │     4. If `f` may raise: fusion must preserve exception ordering — if unfused
    │        code would call f(x) for elements [0,1,2,...] and raise at element 5,
    │        fused code must also call f exactly for elements [0,1,2,3,4,5] before raising.
    │     5. If purity cannot be proven: do NOT fuse. Fall back to standard generator.
    │
Pass 10: Closure/Lambda Specialization
    │   Input:  TIR with closure values and indirect calls
    │   Output: Monomorphized callers, inlined lambdas
    │   Techniques:
    │     - Monomorphize: sorted(data, key=lambda x: x.name)
    │         → sorted_with_name_key(data)  // key function inlined
    │     - Lambda-lift: non-escaping closures become extra parameters
    │     - Defunctionalize: finite closure set → tagged union + switch
    │
Pass 11: Devirtualization (Class Hierarchy Analysis)
    │   Input:  TIR with molt.call_method operations, ClassHierarchy
    │   Output: Direct calls replacing indirect dispatch
    │   Algorithm:
    │     1. Build whole-program class hierarchy graph
    │     2. For each call site, determine receiver type
    │     3. If leaf class (no subclasses) → direct call
    │     4. If single implementor of method → direct call
    │     5. If PGO shows >95% one type → guarded direct call + deopt
    │     6. Inline small direct calls (≤30 TIR ops)
    │
Pass 12: Monomorphization
    │   Input:  TIR with generic (DynBox) function calls
    │   Output: Type-specialized function copies
    │   Rules:
    │     - Specialize when all argument types are concrete
    │     - Depth limit: 4 levels of nesting
    │     - Recursive functions: specialize first call, dynamic for recursive
    │     - Cache specializations: same type tuple → same function
    │     - Dead specializations eliminated in final DCE pass
    │
Pass 13: Refcount Elimination
    │   Input:  TIR with molt.inc_ref / molt.dec_ref operations
    │   Output: Redundant refcount ops removed
    │   Patterns:
    │     - inc(x); ...; dec(x) where x not shared → remove both
    │     - dec(x); inc(x) (ownership transfer) → remove both
    │     - NoEscape values → remove all refcount ops
    │     - SSA lifetime covers all uses → refcount provably redundant
    │     - Sink dec_ref to latest point (ASAP destruction variant)
    │
Pass 14: Loop Optimization
    │   Input:  TIR with loop nesting tree and trip count estimates
    │   Output: Optimized loops
    │   Techniques:
    │     - Unroll small loops (trip count ≤ 8, body ≤ 16 ops)
    │     - Vectorization hints (trip count, element type, stride)
    │     - Induction variable simplification
    │     - Loop unswitching (if invariant condition inside loop)
    │     - Pre/main/post splitting for bounds check elimination
    │
Pass 15: Container Specialization
    │   Input:  TIR with typed container operations
    │   Output: Specialized container implementations
    │   Transforms:
    │     list[int] → backing store is contiguous i64[]
    │     dict[str, int] → key comparison is pointer equality (interned)
    │     tuple[int, float, str] → LLVM struct {i64, f64, ptr}
    │     set[int] → hash-set with inline i64 entries
    │   Impact: eliminates per-element boxing, enables SIMD on containers
    │
Pass 16: Strength Reduction
    │   Input:  TIR with expensive operations
    │   Output: TIR with cheaper equivalent operations
    │   Transforms:
    │     x ** 2           → x * x
    │     x * 2            → x + x  (or x << 1 for I64)
    │     x * power_of_2   → x << k
    │     x // power_of_2  → x >> k  (for non-negative x)
    │     x % power_of_2   → x & (power_of_2 - 1)  (for non-negative x)
    │     len(s) == 0      → s is empty  (flag check, no length computation)
    │     x in {a, b, c}   → x == a or x == b or x == c  (for small sets)
    │     abs(x)            → (x ^ (x >> 63)) - (x >> 63)  (branchless for I64)
    │   Note: runs AFTER e-graph integration (Phase 3) is available, the e-graph
    │   subsumes many of these transforms. This pass provides the immediate wins
    │   that are available in Phase 2 before e-graphs are wired in.
    │
Pass 17: Final DCE + Cleanup
        Input:  Optimized TIR
        Output: Minimal TIR ready for backend lowering
        Actions: Remove dead code, unused specializations, unreachable blocks
```

### 4.6 TIR → Backend Lowering

```
TIR (optimized)
    │
    ├──→ TIR → Cranelift (dev mode)
    │      • TIR blocks → Cranelift blocks
    │      • TIR types → Cranelift types (I64 → i64, F64 → f64, Ptr → r64)
    │      • DynBox values → NaN-boxed i64
    │      • molt.alloc → call @molt_alloc
    │      • molt.stack_alloc → Cranelift stack_slot
    │      • molt.call → Cranelift call (direct or indirect)
    │
    ├──→ TIR → LLVM IR (release mode) [Section 6]
    │      • Full type information preserved in LLVM types
    │      • Custom LLVM passes exploit type info
    │      • PGO, LTO, BOLT applied
    │
    ├──→ TIR → WASM (edge/browser)
    │      • TIR types → WASM types (I64 → i64, F64 → f64)
    │      • DynBox → i64 with NaN-boxing
    │      • molt.stack_alloc → WASM stack pointer manipulation
    │      • molt.scf.for → WASM loop/block/br structure
    │
    └──→ TIR → GPU (Metal/WebGPU/CUDA/AMD) [Section 10]
           • molt.gpu dialect → target shading language
           • Exception handling stripped
           • Math functions replaced with GPU equivalents
```

---

## 5. Phase 0: Beat CPython on All Benchmarks

**Duration:** Weeks 1-2
**Prerequisites:** None
**Deliverable:** Molt faster than CPython on all 61+ benchmarks

### 5.1 Benchmark Audit & Profiling

**[0.1] Full benchmark triage:**
- Run all 61 benchmarks against CPython 3.12 baseline
- Classify: Green (faster), Yellow (within 2x), Red (slower than 2x)
- For Yellow/Red, collect:
  - `perf stat -e instructions,cache-misses,branch-misses,LLC-load-misses` (native)
  - Allocation counts via `MOLT_ALLOC_TRACE=1`
  - Hot function identification via sampling profiler

**Acceptance criteria:** Every benchmark has a classification and a profiling artifact.

### 5.2 Inline Cache System

**[0.2] Monomorphic inline cache for attribute access:**

```rust
/// Inline cache entry — 8 bytes, fits in a cache line with the instruction
struct InlineCache {
    cached_type_id: u32,       // type_id of last successful lookup
    cached_slot_offset: u16,   // offset into object's slot array
    miss_count: u8,            // saturating counter (deopt after 4)
    _pad: u8,
}

/// IC lookup sequence (native codegen):
/// 1. Load object's type_id (1 load)
/// 2. Compare with cached_type_id (1 cmp + branch)
/// 3. Hit: load value at cached_slot_offset (1 load) — total: 3 instructions
/// 4. Miss: full dict lookup → update cache → retry
///
/// WASM: IC table in linear memory at known offset
/// Native: IC embedded adjacent to call site for cache locality
```

**Sites:** Every `LOAD_ATTR`, `STORE_ATTR`, `CALL_METHOD` in compiled code.

**Acceptance criteria:**
- `bench_attr_access.py` speedup ≥ 2x over current
- `bench_class_hierarchy.py` speedup ≥ 1.5x
- IC hit rate ≥ 80% on OO benchmarks (measured via `MOLT_IC_TRACE=1`)

### 5.3 Dictionary Optimization

**[0.3] Compact dict layout + string-key fast path:**

```
Current dict layout: hash table with PyObject* keys and values

New layout:
┌─────────────────────────────────────┐
│ Indices: [u8|u16|u32] hash→slot     │  (sized by table capacity)
├─────────────────────────────────────┤
│ Keys:    [Key0, Key1, Key2, ...]    │  (compact array, in insertion order)
├─────────────────────────────────────┤
│ Values:  [Val0, Val1, Val2, ...]    │  (parallel array)
├─────────────────────────────────────┤
│ Hashes:  [H0, H1, H2, ...]         │  (precomputed, for resize)
├─────────────────────────────────────┤
│ Metadata: version_tag, size, cap    │
└─────────────────────────────────────┘

Key-sharing for instances of the same class:
  All instances share the Keys array (read-only).
  Each instance has its own Values array.
  Saves ~40 bytes per instance for a 5-attribute class.

Small dict (≤8 entries):
  Linear probe with no hash table — fits in 1-2 cache lines.
  Key comparison: pointer equality for interned strings.

String-key fast path:
  When all keys are interned strings:
    lookup = hash(key) → index → keys[index] == key (pointer eq) → values[index]
    Total: hash + 1 load + 1 compare + 1 load = ~4 instructions
```

**Acceptance criteria:**
- `bench_dict_ops.py` speedup ≥ 1.5x
- `bench_counter_words.py` speedup ≥ 2x (dict-heavy workload)
- Memory usage for dict-of-dicts patterns reduced ≥ 30%

### 5.4 String Representation Overhaul

**[0.7] Tagged union string representation:**

```rust
enum MoltString {
    /// Inline: ≤23 bytes stored directly in the NaN-boxed value
    /// Layout: [len: u8][data: u8 * 23] — no heap allocation
    /// Covers: ~80% of strings in typical Python code
    Inline { len: u8, data: [u8; 23] },

    /// OneByte: ASCII-only, 1 byte per character
    /// Half the memory of TwoByte for pure-ASCII strings
    OneByte { ptr: *const u8, len: u32, cap: u32, hash: u64 },

    /// TwoByte: BMP characters, 2 bytes per character
    TwoByte { ptr: *const u16, len: u32, cap: u32, hash: u64 },

    /// Interned: deduplicated in global pool, pointer equality for comparison
    /// All identifier-like strings auto-interned (matches CPython behavior)
    Interned { ptr: *const u8, len: u32, intern_id: u32 },

    /// Cons: deferred concatenation — avoids O(n²) for repeated str + str
    /// Flattened on first access to content (lazy)
    Cons { left: *const MoltString, right: *const MoltString, total_len: u32 },
}
```

**String interning strategy:**
- All string literals interned at compile time
- All attribute names / dict keys interned at compile time
- Runtime: strings matching `[a-zA-Z_][a-zA-Z0-9_]*` auto-interned on first use
- Interned string comparison: `ptr_a == ptr_b` (1 instruction)
- Dict key lookup with interned keys: pointer equality, no hash needed for probe

**SIMD string operations:**
- `str.find` / `str.count` / `str.split`: NEON (ARM 128-bit), AVX2 (x86 256-bit), WASM SIMD v128
- Architecture dispatch via compile-time feature detection (Cranelift/LLVM) or runtime detection

**Acceptance criteria:**
- `bench_str_split.py` speedup ≥ 1.5x
- `bench_str_find.py` speedup ≥ 2x
- `bench_str_join.py` heap corruption bug fixed AND speedup ≥ 1.5x
- Memory usage for string-heavy workloads reduced ≥ 30%
- `bench_counter_words.py` (string+dict) speedup ≥ 2x (compounding with dict optimization)

### 5.5 Fast Path Completeness

**[0.5] Ensure every CPython specialization has a Molt equivalent:**

| CPython Specialization | Molt Fast Path | Action |
|---|---|---|
| `LOAD_GLOBAL_MODULE` (98%) | Module attr cache | Audit: ensure hit rate ≥ 95% |
| `LOAD_GLOBAL_BUILTIN` (98%) | Builtin dispatch table | Audit: ensure O(1) |
| `LOAD_ATTR_INSTANCE_VALUE` (81%) | Inline cache [0.2] | Implement |
| `STORE_ATTR_INSTANCE_VALUE` (91%) | Inline cache [0.2] | Implement |
| `BINARY_OP_ADD_INT` (~80%) | `fast_int` in SimpleIR | Audit: ensure always triggered for int+int |
| `BINARY_OP_ADD_FLOAT` (~80%) | `fast_float` in SimpleIR | Audit: ensure always triggered for float+float |
| `BINARY_OP_ADD_UNICODE` (~80%) | String concat intrinsic | Audit: ensure using SIMD |
| `BINARY_SUBSCR_LIST_INT` (54%) | List index intrinsic | Implement: bounds check + direct load |
| `BINARY_SUBSCR_DICT` (54%) | Dict lookup intrinsic | Audit: ensure using compact dict |
| `CALL_BUILTIN_FAST` (72%) | Direct intrinsic dispatch | Audit: coverage of top-100 builtins |
| `FOR_ITER_LIST` | List iterator fast path | Implement: pointer increment, no bounds check for main loop |
| `FOR_ITER_RANGE` | Range loop lowering | Audit: ensure lowered to counter loop |
| `UNPACK_SEQUENCE_TWO_TUPLE` | Tuple unpack intrinsic | Audit: ensure struct decomposition |
| `COMPARE_OP_INT` | Integer comparison fast path | Implement: direct i64 compare for `fast_int` |
| `COMPARE_OP_STR` | String comparison fast path | Implement: interned → ptr eq, else SIMD |
| `STORE_SUBSCR_LIST_INT` | List store fast path | Implement: bounds check + direct store |
| `STORE_SUBSCR_DICT` | Dict store fast path | Audit: ensure compact dict store |

**Acceptance criteria:** All 61+ benchmarks faster than CPython 3.12.

### 5.6 Hot/Cold Header Split

**[0.6] Object header redesign:**

```
BEFORE (40 bytes per object):
┌──────────┬───────────┬──────────┬───────┬─────┬──────┬───────┐
│ type_id  │ ref_count │ poll_fn  │ state │ pad │ size │ flags │
│ u32      │ u32       │ *fn (8B) │ u8    │ 7B  │ u32  │ u32   │
└──────────┴───────────┴──────────┴───────┴─────┴──────┴───────┘

AFTER:
Hot header (16 bytes, inline in allocation):
┌──────────┬───────────┬────────────┬──────────────┐
│ type_id  │ ref_count │ flags_u16  │ size_class   │
│ u32      │ u32       │ u16        │ u16           │
└──────────┴───────────┴────────────┴──────────────┘

Rationale for 16 bytes (not 8):
  - `flags_u16` stays hot because `HEADER_FLAG_IMMORTAL` is checked on every
    refcount operation. Compressed from u64 to u16 (only 6 flags used).
  - `size_class` stays hot because `dec_ref` must know allocation size for
    deallocation. Instead of raw `usize`, use a size class index (u16) that
    maps to actual byte size via a 256-entry lookup table. This covers
    allocations up to 64KB in 256 granularities; larger objects use cold header.
  - `type_id` (u32) can derive exact size for known types via
    `TYPE_SIZE_TABLE[type_id]`, but `size_class` handles dynamically-sized
    objects (variable-length strings, lists with capacity).

Cold header (24 bytes, separate pool, linked via type_id flag):
┌──────────┬───────┬─────┬──────────────┐
│ poll_fn  │ state │ pad │ cold_next     │
│ *fn (8B) │ i64   │ --  │ *ptr (8B)     │
└──────────┴───────┴─────┴──────────────┘

Cold header allocated only when: generator (needs poll_fn), async object
(needs state), or object with `__del__` (needs destructor tracking).
Most objects (int, str, list, dict, tuple, range, set, bytes): hot header only.

Detection: `flags_u16 & FLAG_HAS_COLD_HEADER` indicates cold header exists.
Cold header lookup: `cold_header_pool[object_address]` (hash map, amortized O(1)).
```

**Acceptance criteria:**
- Object allocation size reduced from 40+ bytes to 16+ bytes for common types
- `bench_gc_pressure.py` speedup ≥ 1.5x
- No regressions on any benchmark

---

## 6. Phase 1: Foundations

**Duration:** Weeks 3-5
**Prerequisites:** Phase 0 complete
**Parallel tracks:** A (TIR), B (LLVM scaffold), C (allocator)

### Track A: TIR Construction

**[1.1] TIR data structures:**
Implement all structs from Section 4.2 in `runtime/molt-backend/src/tir/mod.rs`.
- `TirModule`, `TirFunction`, `TirBlock`, `TirOp`, `TirValue`, `TirType`
- Type registry with interning (same type → same TypeId)
- Pretty-printer for debugging: `TIR_DUMP=1 molt compile app.py` emits human-readable TIR
- Round-trip serialization (binary format) for incremental compilation cache

**[1.2] SimpleIR → CFG extraction:**
- Identify basic block boundaries: branch targets, exception handlers, function entry
- Build predecessor/successor adjacency lists
- Compute dominator tree (Lengauer-Tarjan algorithm, O(n α(n)))
- Compute dominance frontiers (for SSA construction)
- Identify natural loops via back-edge detection
- Build loop nesting tree with depth annotations

**[1.3] CFG → SSA conversion:**
- Iterated dominance frontier algorithm for block argument insertion
- Variable renaming pass (walk dominator tree)
- Block arguments at join points (MLIR-style) — NOT phi nodes
- Critical edge splitting where needed
- Verify SSA: every use dominated by its definition

**[1.4] Type refinement pass:**
- Forward dataflow analysis with type lattice
- Source 1: explicit annotations (from `type_facts.py` output)
- Source 2: operation inference (arithmetic → numeric, indexing → int)
- Source 3: assignment inference (literal type propagation)
- Source 4: isinstance narrowing (branch-sensitive type refinement)
- Source 5: container element types (List[int] access → int)
- Iterate to fixpoint (max 20 rounds, conservative on timeout)
- Emit type refinement statistics: `TIR_TYPE_STATS=1`

**[1.5] TIR → SimpleIR back-conversion:**
- For Cranelift/WASM backends to consume while LLVM is being built
- Preserve type information as annotations (for future consumption)
- Ensure no regressions: all existing benchmarks must produce identical results

**Acceptance criteria:**
- `TIR_DUMP=1` produces readable output for all 61 benchmarks
- Round-trip: SimpleIR → TIR → SimpleIR produces semantically identical code
- Type refinement resolves ≥60% of DynBox values to concrete types on annotated code
- No regressions on any benchmark

### Track B: LLVM Backend Scaffold

**[1.6] Inkwell integration:**
- Add `inkwell` crate dependency (pinned to LLVM 18 feature via `inkwell = { version = "0.5", features = ["llvm18-0"] }`)
- Build system: detect LLVM 18 installation, fail gracefully if missing
- Feature flag: `--features llvm` for optional LLVM support
- CI: add LLVM 18 to build matrix
- **Pass manager note:** Inkwell exposes the legacy pass manager by default. Custom LLVM
  passes (Phase 2 Track E) require the new pass manager for proper integration.
  Use `inkwell`'s `PassBuilderOptions` API (available in `llvm18-0` feature) to construct
  the new PM pipeline and register custom passes via extension point callbacks.
  If `PassBuilderOptions` is insufficient, use raw LLVM C API (`LLVMPassBuilderOptionsCreate`,
  `LLVMRunPasses`) as an escape hatch — document each usage.

**[1.7] SimpleIR → LLVM IR lowering (basic):**
- New file: `runtime/molt-backend/src/llvm_backend/mod.rs`
- Module structure: one LLVM module per Python module
- Function lowering: parameters, locals, basic blocks, returns
- Value types: `i64` for ints, `double` for floats, `i64` for NaN-boxed DynBox
- Basic operations: arithmetic, comparison, branch, call
- Runtime imports: `@molt_alloc`, `@molt_call`, `@molt_box_*`, `@molt_unbox_*`
- Exception handling: Itanium C++ ABI (`invoke`/`landingpad`) for native targets
  - **EH model rationale:** Itanium ABI is the de facto standard on Linux/macOS ARM64/x86-64.
    Zero-cost when no exception is thrown (no overhead on `invoke` happy path beyond LSDA tables).
    For Python code where most operations CAN raise but FEW actually DO, zero-cost EH is optimal.
    Windows targets may use WinEH (SEH-based) if cross-compilation is needed.
  - **WASM EH interop:** The WASM backend uses `try_table`/`throw`/`catch` (WASM EH proposal),
    which has different stack unwinding semantics. The deopt framework (Section 8.3) works
    at TIR level BEFORE backend lowering, so it is EH-model-agnostic — deopt points
    lower to the appropriate EH mechanism per backend.

**[1.8] Runtime function imports:**
- Generate LLVM function declarations for all 2,193 intrinsics
- Calling convention: C calling convention for FFI with Rust runtime
- String table: embed interned string literals as LLVM global constants

**[1.9] End-to-end test:**
- `bench_sum.py` compiles and runs correctly via LLVM backend
- Output matches CPython exactly
- Compare performance: LLVM -O3 vs Cranelift

**Acceptance criteria:**
- `molt compile --backend llvm bench_sum.py` produces correct output
- LLVM -O3 binary is ≥ 1.2x faster than Cranelift binary
- Build system supports `--features llvm` as optional

### Track C: Allocator Improvements

**[1.10] Switch to mimalloc:**
- Add `mimalloc` crate as global allocator: `#[global_allocator] static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;`
- Thread-local free lists (no lock contention)
- Superior small-object performance (most Python objects <128 bytes)
- Benchmark: allocation-heavy benchmarks should show 5-10% improvement

**[1.11] Nursery bump allocator:**
```rust
/// Per-function nursery for short-lived objects
struct Nursery {
    base: *mut u8,           // arena start
    cursor: *mut u8,         // next allocation point
    limit: *mut u8,          // arena end (base + 64KB)
    promoted: Vec<*mut u8>,  // objects that survived past function scope
}

impl Nursery {
    /// Allocation: 2 instructions (compare + increment)
    fn alloc(&mut self, size: usize) -> *mut u8 {
        let ptr = self.cursor;
        let new_cursor = ptr.add(size);
        if new_cursor <= self.limit {
            self.cursor = new_cursor;
            ptr
        } else {
            self.alloc_slow(size)  // promote to main heap
        }
    }

    /// Reset: 1 instruction (reset cursor to base)
    fn reset(&mut self) {
        self.cursor = self.base;
        // promoted objects remain on main heap
    }
}
```
- Integrate with function prologues/epilogues
- Objects that escape (detected by escape analysis in Phase 2) get promoted
- For Phase 1: conservative — only use for known-temporary allocations (string formatting, tuple unpacking)

**Write barrier (critical for soundness):**
```rust
/// When storing a nursery pointer into a heap-resident container,
/// the nursery object MUST be promoted to the main heap first.
/// This prevents use-after-free when nursery.reset() reclaims memory.
///
/// Write barrier fires on every store to a heap object's field/element:
///   list.append(val), dict[key] = val, obj.attr = val, etc.
impl WriteBarrier {
    #[inline(always)]
    fn barrier_store(&self, target: *mut MoltObject, value: *mut MoltObject) {
        if unlikely(self.is_nursery_ptr(value) && !self.is_nursery_ptr(target)) {
            // Storing nursery pointer into heap object → promote
            self.promote_to_heap(value);
        }
    }

    #[inline(always)]
    fn is_nursery_ptr(&self, ptr: *mut MoltObject) -> bool {
        let addr = ptr as usize;
        addr >= self.nursery_base && addr < self.nursery_limit
    }

    fn promote_to_heap(&self, ptr: *mut MoltObject) {
        // 1. Allocate on main heap
        let heap_ptr = molt_alloc(object_size(ptr));
        // 2. Copy object data
        std::ptr::copy_nonoverlapping(ptr, heap_ptr, object_size(ptr));
        // 3. Update all references (forwarding pointer)
        self.install_forwarding_ptr(ptr, heap_ptr);
        // 4. Add to promoted list (don't reset this memory on nursery.reset())
        self.nursery.promoted.push(heap_ptr);
    }
}
```

**Remembered set (for generational correctness):**
- Maintain a card table: 1 bit per 512-byte region of the heap
- When a heap object stores a reference, mark its card
- On nursery collection: only scan marked cards for nursery pointers
- Card marking overhead: ~1 instruction per store (bit-set on store address >> 9)

**Acceptance criteria:**
- `bench_gc_pressure.py` speedup ≥ 1.3x from mimalloc alone
- Nursery handles ≥ 40% of allocations in typical benchmarks
- No memory leaks (valgrind/ASAN clean)
- Write barrier verified: no use-after-free under ASAN for all benchmarks
- Forwarding pointer correctness: all references updated after promotion

---

## 7. Phase 2: Optimization Core

**Duration:** Weeks 6-8
**Prerequisites:** Phase 1 TIR exists, LLVM compiles basic programs
**Parallel tracks:** D (TIR passes), E (LLVM passes), F (memory)

### Track D: TIR Optimization Passes

Implement all passes from Section 4.5. Key details for the non-trivial ones:

**[2.1] Unboxing pass:**
- Walk each `molt.box` operation
- Trace all consumers of the boxed value
- If ALL consumers are `molt.unbox` to the same type → eliminate pair
- If some consumers are DynBox → keep boxing, but unbox on the hot path with a guard
- Metric: `TIR_UNBOX_STATS=1` reports unboxing rate per function

**[2.2] Escape analysis:**
- Lattice: `NoEscape < ArgEscape < GlobalEscape`
- Interprocedural: analyze callee summaries for `ArgEscape` determination
- Handle: function calls, returns, stores to heap fields, exceptions (thrown values escape)
- Conservative for unknown callees (external functions, polymorphic calls)
- Metric: `TIR_ESCAPE_STATS=1` reports escape classification per allocation

**[2.7] Monomorphization:**
- Build call graph with argument type tuples
- For each call site with concrete argument types, check if specialization exists
- Generate specialized copy with concrete types replacing DynBox
- Inline small specializations (≤ 30 TIR ops, no loops) at call site
- Specialization cache: `(func_id, type_tuple) → specialized_func_id`
- Depth limit: 4 levels (prevents exponential blowup with deeply generic code)
- Metric: `TIR_MONO_STATS=1` reports specializations generated per function

**[2.15] Bounds check elimination:**
```
Example:
    for i in range(len(lst)):
        x = lst[i]          # bounds check: 0 <= i < len(lst)

After analysis:
    i ∈ [0, len(lst))       # range analysis proves bounds
    → eliminate check

    for i in range(n):
        x = lst[i]          # can't prove n <= len(lst)

After pre/main/post splitting:
    pre:  for i in range(0, min(n, len(lst))): x = lst[i]  # checked
    main: (empty if n > len(lst))
    post: for i in range(len(lst), n): raise IndexError    # separated
```

**[2.16] Deforestation / iterator fusion:**
```
Pattern detection at TIR level:

    # Before fusion:
    gen1 = (x*x for x in data)           # generator alloc + yield
    gen2 = (y for y in gen1 if y > 0)    # generator alloc + yield + filter
    result = sum(gen2)                     # iteration + accumulation

    # After fusion:
    acc = 0
    for x in data:
        tmp = x * x
        if tmp > 0:
            acc += tmp
    result = acc

Fusion rules:
    map(f, iter)     → fused: apply f in loop body
    filter(pred, it) → fused: if pred(x) in loop body
    sum(iter)        → fused: acc += x in loop body
    list(iter)       → fused: result.append(x) in loop body
    any(iter)        → fused: if x: return True
    all(iter)        → fused: if not x: return False
    min/max(iter)    → fused: if x < best: best = x

Composition fuses transitively:
    sum(map(f, filter(g, data))) → single loop with f, g, accumulation
```

**[2.17] Closure/lambda specialization:**
```
Before:
    sorted(data, key=lambda x: x.name)
    # → indirect call to anonymous closure at each comparison

After monomorphization:
    sorted_specialized_by_name(data)
    # → direct attr load in comparison, no closure allocation

Before:
    def make_adder(n):
        return lambda x: x + n

    add5 = make_adder(5)
    result = add5(10)

After lambda lifting + constant propagation:
    # lambda x: x + 5 → direct: x + 5
    result = 10 + 5  # constant folded to 15
```

**Acceptance criteria:**
- Type refinement resolves ≥ 80% of values to concrete types on annotated code
- Escape analysis classifies ≥ 40% of allocations as NoEscape
- Bounds checks eliminated in ≥ 90% of `range(len(x))` patterns
- Iterator fusion fires on all map/filter/reduce/comprehension chains
- Monomorphization generates specializations for ≥ 70% of hot call sites

### Track E: Custom LLVM Passes

**[2.8] AllocationRemover:**
```
Input LLVM IR:
    %obj = call ptr @molt_alloc(i32 16)     ; heap allocation
    store i64 42, ptr %obj                   ; initialize
    %val = load i64, ptr %obj                ; use
    call void @molt_dec_ref(ptr %obj)        ; free

After AllocationRemover (when %obj doesn't escape):
    %obj = alloca i64, align 8               ; stack allocation (free at function exit)
    store i64 42, ptr %obj
    %val = load i64, ptr %obj
    ; molt_dec_ref removed (stack object, no refcount needed)
```

Implementation:
- Walk all `call @molt_alloc` instructions
- Compute escape: does the pointer get stored to a global, returned, or passed to an unknown callee?
- NoEscape: replace with `alloca`, remove refcount ops
- Fixed-size check: allocation size must be compile-time constant
- Register as LLVM pass via `PassBuilder::registerPipelineStartEPCallback`

**[2.9] RefcountEliminator:**
```
Patterns eliminated:
    inc_ref(x); dec_ref(x)        → nothing (cancel out)
    inc_ref(x); f(x); dec_ref(x)  → f(x) if f doesn't store x
    dec_ref(x); inc_ref(x)        → nothing (ownership transfer)
    inc_ref(x); ... return x      → nothing if caller takes ownership

SSA-based analysis:
    If SSA value x has exactly N uses, and N inc_refs are paired with N dec_refs,
    and no use stores x to heap → all refcount ops are redundant.
```

**[2.10] BoxingEliminator:**
```
Input LLVM IR:
    %boxed = call i64 @molt_box_i64(i64 %x)     ; NaN-box
    %unboxed = call i64 @molt_unbox_i64(i64 %boxed) ; unbox
    %result = add i64 %unboxed, 1

After BoxingEliminator:
    %result = add i64 %x, 1                       ; direct, no boxing

When partial:
    %boxed = call i64 @molt_box_i64(i64 %x)
    call void @some_dyn_func(i64 %boxed)           ; needs boxed
    %unboxed = call i64 @molt_unbox_i64(i64 %boxed)
    %result = add i64 %unboxed, 1

    → %result = add i64 %x, 1                      ; unboxed path optimized
    → call void @some_dyn_func(i64 %boxed)          ; boxed path kept
```

**[2.11] TypeGuardHoister:**
```
Before:
    loop:
        %tag = and i64 %val, TAG_MASK              ; type check
        %is_int = icmp eq i64 %tag, TAG_INT
        br i1 %is_int, label %fast, label %slow
    fast:
        %raw = and i64 %val, INT_MASK              ; unbox
        ; use %raw
        br label %loop

After hoisting (when %val type is loop-invariant):
    %tag = and i64 %val, TAG_MASK                  ; hoisted before loop
    %is_int = icmp eq i64 %tag, TAG_INT
    br i1 %is_int, label %loop_fast, label %loop_slow

    loop_fast:                                      ; specialized loop (no checks)
        %raw = and i64 %val, INT_MASK
        ; use %raw
        br label %loop_fast

    loop_slow:                                      ; fallback loop (with checks)
        ; generic path
```

**Acceptance criteria:**
- AllocationRemover eliminates ≥ 30% of heap allocations on typical benchmarks
- RefcountEliminator removes ≥ 50% of refcount operations
- BoxingEliminator removes ≥ 80% of box/unbox pairs when TIR types are concrete
- TypeGuardHoister hoists guards out of ≥ 90% of loops with invariant types

### Track F: Memory System

**[2.12] Biased reference counting:**

Design follows CPython 3.13 PEP 703 approach — thread ownership tracked separately
from the refcount word, avoiding the 16-bit overflow problem:

```rust
struct MoltObjectHeader {
    type_id: u32,
    /// Full 32-bit local refcount (non-atomic, owned by creating thread).
    /// Supports refcounts up to 2^32 — no overflow for large lists or shared types.
    local_ref_count: u32,
    // flags_u16 and size_class from hot header (Section 5.6)
}

/// Thread ownership model (inspired by CPython 3.13 PEP 703):
///
/// Each object has a "biased" (owning) thread — the thread that allocated it.
/// - Owning thread: uses `local_ref_count` (non-atomic, single instruction)
/// - Non-owning thread: uses a per-object atomic `shared_ref_count` stored in a
///   side table (thread-safe, but slower)
/// - When `shared_ref_count` reaches zero AND object is being decremented by a
///   non-owning thread, the decrement is DEFERRED to the owning thread's queue
///   to avoid races on the local count.
///
/// Ownership determination: thread-local hash set of owned object addresses.
///   Lookup: O(1) amortized. Memory: ~8 bytes per owned object.
///   On object creation: insert into current thread's ownership set.
///   On thread exit: merge all owned objects into a global orphan set.
///
/// Side table for shared counts:
///   ConcurrentHashMap<*mut MoltObject, AtomicU32>
///   Only entries for objects accessed cross-thread (sparse in typical code).

impl MoltObjectHeader {
    #[inline(always)]
    fn inc_ref(&mut self, obj_ptr: *mut MoltObject) {
        if likely(is_owned_by_current_thread(obj_ptr)) {
            self.local_ref_count += 1;  // non-atomic, 1 instruction
        } else {
            shared_ref_table().fetch_add(obj_ptr, 1, Ordering::Relaxed);
        }
    }
}
```

**[2.13] Cycle detection (trial deletion):**
```rust
/// Bacon-Rajan cycle collector
struct CycleCollector {
    possible_roots: Vec<*mut MoltObjectHeader>,  // objects with decremented refcount > 0
    threshold: usize,                             // trigger collection at 1024 roots

    /// Collection algorithm:
    /// 1. Mark gray: trial-decrement reachable objects from each root
    /// 2. Scan: if trial refcount == 0, mark white (garbage)
    /// 3. Collect white: free all white objects
    /// 4. Restore: increment refcounts of non-white objects back to original
    ///
    /// Incremental: process N roots per step (default: 256)
    /// Total pause: < 1ms for typical Python programs
}
```

**[2.14] Container specialization runtime support:**
```rust
/// Specialized list for known element types
enum MoltList {
    /// Generic: NaN-boxed values, any type per element
    Generic { data: Vec<u64>, len: usize },

    /// Int-specialized: contiguous i64 array, no boxing
    IntList { data: Vec<i64>, len: usize },

    /// Float-specialized: contiguous f64 array, no boxing
    FloatList { data: Vec<f64>, len: usize },

    /// String-specialized: contiguous string pointers
    StrList { data: Vec<*const MoltString>, len: usize },
}

impl MoltList {
    /// sum() on IntList: SIMD horizontal add
    fn sum_int(data: &[i64]) -> i64 {
        // NEON: ld1 + addv (128-bit lanes)
        // AVX2: vpaddd across 256-bit lanes
        // Fallback: scalar accumulation
        simd_reduce_add(data)
    }
}
```

**Acceptance criteria:**
- Biased refcount: single-threaded overhead < 2% (vs current atomic)
- Cycle collector: no memory leaks on cyclic structure benchmarks
- Container specialization: `sum(list[int])` uses SIMD reduction

---

## 8. Phase 3: Integration + PGO

**Duration:** Weeks 9-11
**Prerequisites:** TIR passes working, LLVM passes working
**Deliverable:** Full optimizing pipeline, Codon-competitive performance

### 8.1 TIR → LLVM IR Lowering (Full)

**[3.1] Type-preserving lowering:**

```
TIR Type        → LLVM Type
─────────────────────────────
I64             → i64
F64             → double
Bool            → i1 (promoted to i8 at ABI boundary)
None            → void (or i64 sentinel for DynBox context)
Tuple(I64, F64) → { i64, double } (LLVM struct, pass by value)
Struct(id)      → %StructName = type { field1_ty, field2_ty, ... }
List(I64)       → ptr (to MoltListInt runtime type)
Dict(Str, I64)  → ptr (to MoltDictStrInt runtime type)
DynBox          → i64 (NaN-boxed, tag in high bits)
Func(sig)       → ptr (function pointer with known signature)
Closure(sig, env) → { ptr, %env_type } (function pointer + environment struct)
```

**[3.1] Operation lowering:**
```
molt.add I64, I64        → add i64 %a, %b (with overflow check → BigInt fallback)
molt.add F64, F64        → fadd double %a, %b
molt.add DynBox, DynBox  → call @molt_dyn_add(i64 %a, i64 %b)
molt.box I64             → or i64 %val, (QNAN | TAG_INT)
molt.unbox I64           → and i64 %val, INT_MASK
molt.type_guard DynBox→I64 → %tag = and; icmp eq; br [fast, deopt]
molt.alloc               → call ptr @molt_alloc(i32 %size)
molt.stack_alloc         → alloca %Type, align 8
molt.call                → call (direct or indirect based on devirtualization)
molt.ic_lookup           → load cached_type_id; cmp; br [hit, miss]
molt.inc_ref             → call @molt_inc_ref (or eliminated by RefcountEliminator)
molt.dec_ref             → call @molt_dec_ref (or eliminated)
molt.deopt               → call @molt_deopt_transfer(%state)
```

### 8.2 PGO Pipeline

**[3.4] Instrumentation mode:**
```bash
# Step 1: Compile with instrumentation
molt compile --release --pgo-instrument app.py -o app_instrumented

# Step 2: Run with representative workload
./app_instrumented < training_data.txt
# Produces: app.profraw

# Step 3: Convert profile
llvm-profdata merge app.profraw -o app.profdata

# Step 4: Compile with profile
molt compile --release --pgo-use app.profdata app.py -o app_optimized
```

**Instrumentation collects:**
- Function entry counts
- Basic block execution frequencies
- Branch edge weights (taken/not-taken counts)
- Indirect call target frequencies (type_id distribution per call site)
- Loop trip counts

**[3.5] PGO-guided optimizations:**
- **Indirect call promotion:** If 95%+ of calls at a site target the same function, emit guarded direct call
- **Branch weight metadata:** LLVM uses weights for code layout and `__builtin_expect`-style optimization
- **Loop trip counts:** Inform unrolling decisions (unroll if median trip count ≤ 16)
- **Hot/cold splitting:** Move cold blocks (< 1% execution frequency) to `.cold` section
- **Function ordering:** Hot functions placed adjacent for i-cache locality

### 8.3 Deoptimization Framework

**[3.6] Speculative unboxing with deopt:**

```
High-level:

    def process(x):       # x is DynBox (type unknown)
        return x + 1

PGO shows: x is int 97% of the time.

Compiled (speculative):
    process_speculative:
        %tag = extract_tag(%x)
        %is_int = cmp %tag, TAG_INT
        br %is_int, fast_path, deopt_path

    fast_path:                           # 97% of executions
        %raw = unbox_int(%x)
        %result = add i64 %raw, 1
        %boxed = box_int(%result)
        ret %boxed

    deopt_path:                          # 3% of executions
        ; Transfer state to generic version
        call @molt_deopt_transfer(
            func = @process_generic,
            live_values = [%x],
            var_mapping = [(x, 0)]
        )

    process_generic:                     # fallback, handles any type
        %result = call @molt_dyn_add(%x, @molt_box_int(1))
        ret %result
```

**Deopt state transfer:**
- Save live SSA values to a materialization buffer
- Map SSA values to variable slots in the generic version
- Jump to the generic version's entry (or mid-function resume point)
- Generic version reads materialized values and continues

### 8.4 Advanced Passes

**[3.7] LTO / ThinLTO:**
```bash
# Full LTO: maximum optimization, slowest link
molt compile --release --lto=full app.py

# ThinLTO: per-module optimization with cross-module info
molt compile --release --lto=thin app.py   # default for --release
```
- Emit LLVM bitcode per module
- ThinLTO: build cross-module summary, import hot functions, devirtualize cross-module
- Full LTO: merge all modules, full interprocedural optimization
- Estimated impact: 5-15% additional speedup

**[3.8] CoroutineElider:**
- Generators compiled to LLVM coroutines: `@llvm.coro.id`, `@llvm.coro.begin`, `@llvm.coro.suspend`
- LLVM's coroutine passes: CoroEarly → CoroSplit → CoroElide → CoroCleanup
- Custom enhancement: when generator is consumed in same function, escape analysis proves handle is NoEscape → heap allocation elided, generator fully inlined

- **Applicability analysis:** LLVM's `CoroElide` only applies when the coroutine handle
  provably does not escape the caller. Python generators commonly escape:
  - `return (x for x in data)` — generator returned to caller (ESCAPES)
  - `yield from gen()` — generator passed to yield-from machinery (ESCAPES)
  - `list(x for x in data)` — generator consumed by list() in same scope (ELIGIBLE)
  - `sum(x*x for x in data)` — generator consumed by builtin in same scope (ELIGIBLE)
  - `for item in gen(): ...` — generator consumed in same scope (ELIGIBLE)

  Estimated eligibility: ~40-60% of generator usage in typical Python code.
  The deforestation pass (Pass 9) handles many of the eligible cases at TIR level
  BEFORE coroutine lowering, so CoroElider catches the remaining cases.
  For non-eligible generators, standard heap-allocated coroutine frame is used (no regression).

- Impact: `bench_generator_iter.py` speedup 2-10x (for eligible patterns)

**[3.9] Interprocedural optimization:**
- Build whole-program call graph from TIR
- Cross-function constant propagation: if `f` always called with `x=5`, specialize
- Cross-function escape analysis: if `f(obj)` doesn't store `obj`, caller can stack-allocate
- Cross-module inlining: inline small stdlib functions (≤ 20 TIR ops)
- Dead function elimination: remove unreachable functions from final binary

**[3.10] E-graph integration:**
- Activate `egraph_simplify.rs` (currently feature-gated)
- E-graphs represent multiple equivalent expressions simultaneously
- Extraction finds the lowest-cost equivalent using a cost model:
  ```
  cost(i64 add)     = 1
  cost(i64 mul)     = 3
  cost(i64 div)     = 10
  cost(call)         = 50
  cost(heap alloc)   = 100
  cost(NaN-box)      = 5
  cost(NaN-unbox)    = 3
  ```
- Enables combined optimizations: `x * 2` → `x << 1` → `x + x` (choose cheapest)

**[3.11] Class hierarchy analysis:**
- Build `ClassHierarchy` in `TirModule` during construction
- Track: parent class, child classes, method definitions, overrides
- Leaf class detection: class with no subclasses → all method calls are devirtualizable
- Single-implementor detection: abstract method with one concrete implementation
- Wire into devirtualization pass (Pass 11 in Section 4.5)

**[3.12] BOLT post-link optimization:**
```bash
# After LLVM produces the binary:
# 1. Profile
perf record -e cycles:u -j any,u -- ./app < workload.txt
# 2. Convert
perf2bolt -p perf.data -o perf.fdata ./app
# 3. Optimize
llvm-bolt ./app -o ./app.bolt \
    -data=perf.fdata \
    -reorder-blocks=ext-tsp \
    -reorder-functions=cdsort \
    -split-functions \
    -split-all-cold \
    -split-eh \
    -dyno-stats
```
- Requires linking with `--emit-relocs` (add to LLVM linker flags)
- Automate as: `molt compile --release --bolt app.py`
- Expected impact: 7-20% on top of PGO+LTO

- **Platform limitation:** BOLT requires ELF binaries and Linux `perf` for profiling.
  Not available on macOS (Mach-O binaries). Platform-specific alternatives:
  - **Linux:** Full BOLT pipeline as described above
  - **macOS:** Use Instruments for profiling + linker order files (`-order_file`) for
    function reordering. Apple's `ld64` supports section ordering via order files,
    which achieves a subset of BOLT's function-reordering benefit (~5-10% vs BOLT's 7-20%).
    Hot/cold function splitting via `__attribute__((cold))` on cold functions.
  - **CI:** Add a Linux CI leg specifically for BOLT benchmarking. macOS development
    uses the order-file approach; Linux release builds use full BOLT.

**[3.13] Software prefetching:**
- For dict hash probing: `prefetch(buckets[hash + stride])` during comparison
- For large list iteration: `prefetch(data[i + 16])` in loop body
- For linked structure traversal: `prefetch(node->next->next)`
- Emit via `@llvm.prefetch(ptr, rw=0, locality=1, cache_type=1)`
- Guard with size check: only prefetch if container size > L1 cache line count

**Acceptance criteria:**
- LLVM release binary ≥ 1.5x faster than Cranelift dev binary on all benchmarks
- PGO adds ≥ 10% on polymorphic benchmarks
- BOLT adds ≥ 5% on top of PGO+LTO
- Match or beat Codon on `word_count.py` and `taq.py`
- Beat Nuitka on all benchmarks by ≥ 2x

---

## 9. Phase 4: WASM + Parallelism + SIMD

**Duration:** Weeks 12-14
**Prerequisites:** Phase 3 complete (TIR → LLVM working)
**Parallel tracks:** G (WASM), H (parallelism), I (SIMD)

### Track G: WASM Optimization

**[4.1] Native exception handling (complete linking):**
- Wire `try_table`/`throw`/`catch` in linked WASM output
- Remove 20-40 `exception_pending` poll checks per function
- wasm-ld relocation support for EH sections
- Impact: 20-40% on exception-heavy code, 5-10% binary size reduction

**[4.2] Multi-value returns (complete wiring):**
- Lower tuple-returning builtins to WASM multi-value: `divmod` → `(i64, i64)` on stack
- Eliminate `alloc` + `field_get` per multi-return site
- Wire call-site lowering in TIR → WASM backend

**[4.3] TIR → WASM lowering:**
- Direct TIR → WASM bypassing SimpleIR for optimized output
- TIR type information drives WASM type selection
- Unboxed I64/F64 values stay as WASM i64/f64 (no NaN-boxing for typed code)
- `molt.stack_alloc` → WASM stack pointer manipulation

**[4.4] WASM Component Model:**
- Migrate from raw WASI P1 imports to Component Model
- WIT (WebAssembly Interface Types) for typed interfaces
- Canonical ABI for string/list passing
- Foundation for code splitting

**[4.5] Code splitting:**
```
Runtime core component (~200KB):
    - Object model, refcounting, NaN-boxing
    - Nursery allocator, cycle collector
    - Core builtins (print, len, range, etc.)

Stdlib stub components (loaded on demand):
    - Each stdlib module: type signatures + import declaration
    - Implementation loaded lazily on first use
    - Individual components: 5-50KB each

User code component:
    - Compiled user Python code
    - Type-specialized functions
    - Application-specific data
```

**[4.6] Streaming compilation (browser target):**
- Split WASM module into hot section (entry point + main loop) and cold section (stdlib, error handling)
- Browser starts executing hot section via `WebAssembly.compileStreaming`
- Cold section streams in background
- Impact: 50-80% reduction in time-to-first-execution

**[4.7] Binary size target: < 1MB:**
- Function-level dead code elimination: trace from entry point, emit only reachable
- Type section deduplication: merge identical function types
- Import deduplication: merge identical import signatures
- Brotli compression: ~70% size reduction for network delivery
- Debug info stripping in release mode

**Acceptance criteria:**
- WASM binary size < 1MB for typical applications (after Brotli)
- Native EH: 20%+ speedup on exception-heavy benchmarks
- Multi-value returns: measurable improvement on tuple-heavy code
- Streaming compilation: < 100ms time-to-first-execution for browser targets

### Track H: Parallelism Infrastructure

**[4.8] Parallel compilation:**
```rust
use rayon::prelude::*;

/// Compile functions in parallel using rayon work-stealing
fn compile_module(tir_module: &TirModule) -> CompiledModule {
    let compiled_functions: Vec<CompiledFunction> = tir_module.functions
        .par_iter()
        .map(|func| {
            let optimized = run_tir_passes(func);
            lower_to_backend(optimized)
        })
        .collect();

    link_functions(compiled_functions)
}
```
- Each function's TIR optimization + backend lowering is independent
- Rayon work-stealing distributes across cores
- Shared read-only state: type registry, class hierarchy, interned strings
- Per-function mutable state: optimization passes, codegen
- Expected impact: 3-5x compilation speedup on multi-core machines

**[4.9] Auto-parallel loop detection:**
```
TIR analysis for parallel safety:

    for i in range(n):          # candidate
        result[i] = f(data[i])  # parallel-safe if:
                                 #   1. No write to shared mutable state
                                 #   2. No loop-carried dependency
                                 #   3. No side effects (IO, print)
                                 #   4. f is pure (no global mutation)

    Detection algorithm:
    1. Identify loop body's read/write sets via alias analysis
    2. Check: write_set ∩ read_set_other_iterations = ∅
    3. Check: no calls to impure functions
    4. Check: reduction operations recognized (sum += x)
    5. If safe: annotate loop with molt.par.parallel_for
```

**[4.10] @par decorator implementation:**
```python
from molt import par

@par(num_threads=4, schedule="dynamic", chunk_size=64)
def process(data: list[int]) -> list[int]:
    return [transform(x) for x in data]
```

Lowering:
- Parse decorator at frontend level
- Emit `molt.par.parallel_for` in TIR
- Native backend: lower to rayon `par_iter` or pthreads
- WASM backend: lower to Web Workers (SharedArrayBuffer)
- Automatic private/shared/reduction inference

**[4.11] Parallel reduction patterns:**
```python
# Detected and parallelized:
total = sum(expensive(x) for x in data)

# Lowered to:
#   1. Partition data into chunks
#   2. Each thread computes partial sum
#   3. Final reduction combines partial sums
# Uses: molt.par.reduce with addition combiner
```

**[4.12] GIL removal Phase 1:**
- Biased refcounting already implemented in [2.12]
- Remove `gil_assert()` calls in critical paths
- Add per-object mutex for mutable containers (list, dict, set)
- `@par` functions execute without GIL

**[4.18] Incremental compilation cache:**
```
Cache structure: .molt-cache/
    ├── functions/
    │   ├── {content_hash}.tir       # cached TIR
    │   ├── {content_hash}.obj       # cached object code
    │   └── {content_hash}.meta      # dependencies, types
    ├── modules/
    │   ├── {module_hash}.summary    # type signatures, exports
    │   └── {module_hash}.deps       # import dependencies
    └── index.json                    # hash → artifact mapping

Invalidation:
    - Content hash = SHA256(function AST + resolved type environment)
    - If function body changes → recompile function
    - If function signature changes → recompile function + all callers
    - If import changes → recompile dependent functions
    - Transitive closure computed via dependency graph

Target: <100ms recompilation for single-function change
```

**Acceptance criteria:**
- Parallel compilation: 3x+ speedup on 4-core machine
- Auto-parallel detection fires on `[f(x) for x in data]` patterns
- `@par` decorator works end-to-end with measurable speedup
- Incremental compilation: < 100ms for single-function change
- No data races (ThreadSanitizer clean)

### Track I: SIMD Comprehensive

**[4.13] TIR vectorization hints:**
```rust
/// Vectorization attributes on molt.scf.for loops
struct VectorizationInfo {
    vectorizable: bool,           // safe to vectorize?
    element_type: TirType,        // I64, F64, etc.
    trip_count: Option<u64>,      // known trip count (for unrolling)
    stride: i64,                  // memory access stride (1 = contiguous)
    reduction: Option<ReductionOp>, // sum, product, min, max, and, or
    simd_width: Option<u32>,      // target width (4 for f64, 8 for f32, etc.)
}
```

**[4.14] Auto-vectorization for typed loops:**
```
Detection:
    for i in range(n):
        result[i] = data[i] * 2.0   # contiguous access, known F64

TIR transformation:
    vectorize(width=4):              # f64x4
        for i in range(0, n, 4):
            %v = molt.simd.load(data, i, f64x4)
            %s = molt.simd.splat(2.0, f64x4)
            %r = molt.simd.mul(%v, %s)
            molt.simd.store(%r, result, i)
        for i in range(n - n%4, n):  # scalar remainder
            result[i] = data[i] * 2.0

Lowering:
    Cranelift: SIMD immediates for target architecture
    LLVM: auto-vectorizer handles it (TIR hints guide decisions)
    WASM: v128 operations
```

**[4.15] SIMD intrinsic library:**
```python
from molt.simd import f64x4, i32x8, f32x8

# Explicit SIMD: zero-overhead, maps to hardware instructions
a = f64x4.load(data, offset)
b = f64x4.splat(2.0)
c = a * b                          # element-wise
c.store(result, offset)

# Reductions
total = f64x4.load(data, 0).reduce_add()

# FMA (fused multiply-add)
result = f64x4.fma(a, b, c)       # a*b + c, single rounding
```

**[4.16] Platform-specific dispatch:**
```rust
/// Compile-time architecture selection
#[cfg(target_arch = "aarch64")]
fn simd_sum_f64(data: &[f64]) -> f64 {
    // NEON: 128-bit, process 2 f64 per cycle
    neon_reduce_add_f64(data)
}

#[cfg(target_arch = "x86_64")]
fn simd_sum_f64(data: &[f64]) -> f64 {
    if is_x86_feature_detected!("avx512f") {
        avx512_reduce_add_f64(data)   // 512-bit, 8 f64 per cycle
    } else if is_x86_feature_detected!("avx2") {
        avx2_reduce_add_f64(data)     // 256-bit, 4 f64 per cycle
    } else {
        sse2_reduce_add_f64(data)     // 128-bit, 2 f64 per cycle
    }
}

#[cfg(target_arch = "wasm32")]
fn simd_sum_f64(data: &[f64]) -> f64 {
    wasm_simd128_reduce_add_f64(data) // v128, 2 f64 per cycle
}
```

**[4.17] Fast-math mode:**
```python
from molt import fast_math

@fast_math
def dot_product(a: list[float], b: list[float]) -> float:
    return sum(x * y for x, y in zip(a, b))

# Compiled with:
#   - FMA: fused multiply-add (a*b + c in one instruction)
#   - Reassociation: enables SIMD reduction of sum()
#   - Reciprocal approximation: a/b → a * approx_recip(b)
#   - No NaN/Inf checks: branch-free arithmetic
#   - DAZ/FTZ: denormals flushed to zero
```

- Opt-in only (decorator or `--fast-math` flag)
- LLVM IR: `fast` flag on fadd/fmul/fdiv instructions
- Default path: strict IEEE 754 for CPython parity

**Acceptance criteria:**
- Auto-vectorization fires on typed array loops
- SIMD intrinsic library usable from Python
- `@fast_math` dot_product matches hand-written SIMD performance
- Platform dispatch covers NEON, AVX2, AVX-512, WASM SIMD

---

## 10. Phase 5: GPU + MLIR Foundation

**Duration:** Weeks 15-18
**Prerequisites:** Phase 4 complete (parallelism infra exists)
**Parallel tracks:** J (Metal), K (WebGPU), L (MLIR)

### Track J: Metal GPU Backend

**[5.1] molt.gpu TIR dialect operations:**
Implementation of all operations defined in Section 4.3 `molt.gpu` dialect.

**[5.2] TIR → MSL code generation:**
```
TIR:
    molt.gpu.launch(grid=[256,1,1], block=[256,1,1], args=[a,b,c,n]) {
        %tid = molt.gpu.thread_id(0)
        %cond = arith.cmpi slt, %tid, %n
        molt.scf.if(%cond) {
            %a_val = molt.load(%a, %tid)   // a[tid]
            %b_val = molt.load(%b, %tid)   // b[tid]
            %sum = arith.addf %a_val, %b_val
            molt.store(%c, %tid, %sum)     // c[tid] = sum
        }
    }

Generated MSL:
    #include <metal_stdlib>
    using namespace metal;

    kernel void vector_add(
        device const float* a [[buffer(0)]],
        device const float* b [[buffer(1)]],
        device float* c [[buffer(2)]],
        constant uint& n [[buffer(3)]],
        uint tid [[thread_position_in_grid]]
    ) {
        if (tid < n) {
            c[tid] = a[tid] + b[tid];
        }
    }
```

**[5.3] Metal runtime integration:**
```rust
use metal::*;

struct MoltGPU {
    device: Device,
    command_queue: CommandQueue,
    library_cache: HashMap<String, Library>,  // cached compiled kernels
}

impl MoltGPU {
    fn launch_kernel(
        &self,
        kernel_name: &str,
        msl_source: &str,
        grid_size: MTLSize,
        threadgroup_size: MTLSize,
        buffers: &[Buffer],
    ) {
        let library = self.library_cache.entry(kernel_name.into())
            .or_insert_with(|| {
                self.device.new_library_with_source(msl_source, &CompileOptions::new())
                    .expect("MSL compilation failed")
            });

        let function = library.get_function(kernel_name, None).unwrap();
        let pipeline = self.device.new_compute_pipeline_state_with_function(&function).unwrap();

        let command_buffer = self.command_queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&pipeline);

        for (i, buf) in buffers.iter().enumerate() {
            encoder.set_buffer(i as u64, Some(buf), 0);
        }

        encoder.dispatch_threads(grid_size, threadgroup_size);
        encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();
    }
}
```

**[5.4] Data transfer API:**
```python
from molt import gpu

# Explicit data transfer
a_gpu = gpu.to_device(a_host)           # host → GPU (copies to Metal buffer)
b_gpu = gpu.to_device(b_host)
c_gpu = gpu.alloc(n, float)             # GPU-only allocation

# Launch kernel
vector_add[grid=256, threads=256](a_gpu, b_gpu, c_gpu, n)

# Transfer back
result = gpu.from_device(c_gpu)         # GPU → host
```

**[5.5] @gpu.kernel decorator:**
```python
@gpu.kernel
def mandelbrot(output: gpu.Buffer[float], width: int, height: int,
               x_min: float, x_max: float, y_min: float, y_max: float,
               max_iter: int):
    tid = gpu.thread_id()
    if tid >= width * height:
        return

    px = tid % width
    py = tid // width
    x0 = x_min + (x_max - x_min) * px / width
    y0 = y_min + (y_max - y_min) * py / height

    x, y, iteration = 0.0, 0.0, 0
    while x*x + y*y <= 4.0 and iteration < max_iter:
        xtemp = x*x - y*y + x0
        y = 2*x*y + y0
        x = xtemp
        iteration += 1

    output[tid] = float(iteration) / float(max_iter)
```

Compilation: TIR → MSL → Metal compiler → GPU binary (embedded in app).

**[5.6] Auto-GPU offloading:**
```
Heuristic: GPU wins when compute_flops > K * data_transfer_bytes
    K ≈ 1000 for Metal (PCIe/Unified Memory bandwidth)
    K ≈ 100 for Apple Silicon Unified Memory (lower transfer cost)

For @par(gpu=True) loops:
    1. Estimate compute FLOPs from loop body (mul/add/div counts)
    2. Estimate transfer bytes (input + output buffer sizes)
    3. If ratio > K: generate GPU kernel, insert transfers
    4. If ratio <= K: fall back to CPU parallel
    5. Cache decision: same loop shape → same offload decision
```

### Track K: WebGPU Backend

**[5.7] TIR → WGSL code generation:**
Same `molt.gpu` dialect, different lowering target:
```wgsl
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;

@compute @workgroup_size(256)
fn vector_add(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tid = gid.x;
    if (tid < arrayLength(&a)) {
        c[tid] = a[tid] + b[tid];
    }
}
```

**[5.8] wgpu-native runtime integration:**
- Use `wgpu` crate for cross-platform WebGPU
- Works natively (Metal/Vulkan/DX12) and in browser (WebGPU API)
- Unified programming model across WASM and native targets

**[5.9] Fallback to CPU parallel:**
- If WebGPU unavailable (older browsers, no GPU), fall back to `@par` CPU threading
- Detection: `navigator.gpu` availability check in WASM, `wgpu::Instance::enumerate_adapters` native

### Track L: MLIR Groundwork

**[5.10] TIR ↔ MLIR compatibility validation:**
```rust
/// Automated test: verify every TIR operation can be serialized to MLIR textual format
#[test]
fn tir_to_mlir_roundtrip() {
    for benchmark in all_benchmarks() {
        let tir = compile_to_tir(benchmark);
        let mlir_text = tir_to_mlir_text(&tir);      // serialize
        let tir_back = mlir_text_to_tir(&mlir_text);  // deserialize
        assert_eq!(tir, tir_back);                     // round-trip
    }
}
```

**[5.11] Prototype: TIR → MLIR serialization:**
- Use `melior` crate (safe MLIR bindings for Rust)
- Serialize TIR modules to MLIR textual format
- Verify MLIR tools (`mlir-opt`, `mlir-translate`) can parse the output
- This does NOT yet use MLIR for optimization — just proves the serialization works

**[5.12] MLIR dialect specification (paper design):**
Document the complete `molt`, `molt.gpu`, `molt.par`, `molt.simd` dialect specs in MLIR ODS (Operation Definition Specification) format. This becomes the blueprint for Phase 6 MLIR migration.

**[5.13] Evaluation: MLIR optimization vs TIR-native:**
- Compare TIR-native LICM/SCCP/GVN performance against MLIR equivalent passes
- Identify cases where MLIR's polyhedral optimization (`affine` dialect) provides wins TIR cannot
- Decision point: which passes migrate to MLIR first?

**Acceptance criteria:**
- Metal GPU: `mandelbrot` kernel runs on Apple Silicon, matches CUDA performance
- WebGPU: same kernel runs in browser via WASM
- TIR → MLIR round-trip passes for all 61 benchmarks
- MLIR dialect spec document complete

---

## 11. Phase 6: Frontier

**Duration:** Weeks 19-24
**Prerequisites:** Phase 5 complete

**[6.1] CUDA backend:**
- TIR → PTX via LLVM NVPTX backend
- CUDA driver API for kernel dispatch
- libdevice math functions
- Same `@gpu.kernel` programming model

**[6.2] AMD ROCm backend:**
- TIR → AMDGPU IR via LLVM AMDGPU backend
- HIP runtime API
- Same `@gpu.kernel` programming model

**[6.3] Full GIL removal:**
- Phase 1 (done in [2.12]): biased refcounting
- Phase 2: per-object fine-grained locking (mutex per mutable container)
- Phase 3: epoch-based memory reclamation (safe deferred deallocation)
- Phase 4: full GIL removal — `@par` functions run truly concurrently
- Verification: ThreadSanitizer + stress tests

**[6.4] Polyhedral optimization:**
- Integration with LLVM Polly or MLIR `affine` dialect
- Automatic loop tiling for cache locality
- Loop interchange for stride-1 access
- Loop fusion for producer-consumer patterns
- Automatic parallelism detection

**[6.5] MLIR migration begins:**
- Replace TIR-in-Rust passes with MLIR-native passes one at a time
- Start with: SCCP, GVN (well-understood, MLIR equivalents exist)
- Then: loop optimization (MLIR affine provides superior loop analysis)
- Then: GPU lowering (MLIR gpu dialect → nvvm/rocdl/spirv)
- Cranelift backend can consume MLIR LLVM dialect output via translation layer

**[6.6] Continuous benchmarking infrastructure:**
- CI pipeline: every commit runs smoke benchmarks (< 30s)
- PR pipeline: full benchmark suite with regression detection (threshold: 3% degradation)
- Weekly: extended suite with Codon/Nuitka/CPython comparisons
- Artifact storage: JSON results in `benchmarks/results/` with git rev tags
- Visualization: HTML dashboard with sparklines, trend detection, alerts

**[6.7] Competitive tracking dashboard:**
```
┌─────────────────┬─────────┬─────────┬──────────┬─────────┬─────────┐
│ Benchmark       │ Molt    │ CPython │ Codon    │ Nuitka  │ PyPy    │
├─────────────────┼─────────┼─────────┼──────────┼─────────┼─────────┤
│ bench_sum       │  ✓ 14x  │  1.0x   │  13x     │  3.5x   │  7.6x   │
│ bench_word_count│  ✓ 8x   │  1.0x   │  6x      │  2.1x   │  5.2x   │
│ bench_taq       │  ✓ 12x  │  1.0x   │  11x     │  1.8x   │  4.1x   │
│ bench_fib       │  ✓ 50x  │  1.0x   │  70x     │  3.0x   │  15x    │
│ bench_class_hier│  ✓ 6x   │  1.0x   │  N/A     │  2.5x   │  8x     │
│ ...             │         │         │          │         │         │
└─────────────────┴─────────┴─────────┴──────────┴─────────┴─────────┘
   ✓ = Molt is fastest
```

---

## 12. Verification & Harness Engineering

### 12.1 Correctness Verification

**Principle:** Every optimization must be provably correct. An optimization that produces wrong results is worse than no optimization at all. Verification is not optional — it gates every phase.

**12.1.1 Differential Testing (existing + enhancements)**

```
Existing:
    tests/molt_diff.py — runs same program through Molt and CPython, diffs output
    100+ test files in tests/differential/

Enhancements:
    [V1] Add TIR-level differential testing:
        For each optimization pass, verify:
            execute(tir_before_pass) == execute(tir_after_pass)
        This catches passes that change program semantics.

    [V2] Add type-annotated differential tests:
        Tests with full type annotations to exercise type specialization.
        Verify: specialized code produces same output as generic code.

    [V3] Add parallel execution differential tests:
        Run same program with @par and without.
        Verify: identical output (modulo ordering where expected).

    [V4] Add GPU differential tests:
        Run same computation on CPU and GPU.
        Verify: output matches within floating-point tolerance (1e-10 for f64).
```

**12.1.2 Fuzzing**

```
Existing:
    tests/fuzz/test_fuzz_differential.py — generative differential testing

Enhancements:
    [V5] TIR fuzzing:
        Generate random valid TIR programs.
        Run all optimization passes.
        Verify: output unchanged after optimization.
        Use: AFL/LibFuzzer via cargo-fuzz

    [V6] Type inference fuzzing:
        Generate Python programs with known types.
        Verify: type inference infers the correct types.
        Edge cases: union types, isinstance narrowing, container element types.

    [V7] NaN-boxing fuzzing:
        Generate random values across all 6 tag types.
        Verify: box → unbox round-trip preserves value exactly.
        Edge cases: i64 max/min, negative zero, NaN, infinity.

    [V8] LLVM pass fuzzing:
        Generate LLVM IR with Molt patterns (box/unbox, refcount, alloc).
        Run custom passes.
        Verify: LLVM verifier passes + output unchanged for correctness tests.
```

**12.1.3 Property-Based Testing**

```rust
/// For each TIR optimization pass:
#[proptest]
fn optimization_preserves_semantics(program: ArbitraryTirProgram) {
    let before = interpret_tir(&program);
    let after_opt = run_pass(&program);
    let after = interpret_tir(&after_opt);
    assert_eq!(before, after);
}

/// For type inference:
#[proptest]
fn type_inference_is_sound(program: TypedPythonProgram) {
    let inferred = infer_types(&program);
    for (var, inferred_type) in inferred {
        let actual_type = runtime_type_of(var, &program);
        assert!(actual_type.is_subtype_of(inferred_type));  // soundness
    }
}

/// For escape analysis:
#[proptest]
fn escape_analysis_is_conservative(program: ArbitraryTirProgram) {
    let escape_info = analyze_escapes(&program);
    for (alloc, state) in escape_info {
        if state == EscapeState::NoEscape {
            // Verify: value truly doesn't escape (via runtime tracking)
            assert!(!runtime_escapes(alloc, &program));
        }
        // NoEscape is conservative: false negatives OK, false positives not
    }
}

/// For bounds check elimination:
#[proptest]
fn bce_does_not_remove_needed_checks(
    data: Vec<i64>,
    indices: Vec<usize>,
) {
    let program = make_indexing_program(&data, &indices);
    let optimized = run_bce(&program);
    // If original would raise IndexError, optimized must too
    match (run_original(&program), run_optimized(&optimized)) {
        (Err(IndexError), Err(IndexError)) => {},  // both raise: correct
        (Ok(v1), Ok(v2)) => assert_eq!(v1, v2),    // both succeed: correct
        (Err(IndexError), Ok(_)) => panic!("BCE removed a needed check!"),
        (Ok(_), Err(IndexError)) => {},  // conservative: acceptable
    }
}
```

### 12.2 Performance Verification

**12.2.1 Benchmark Harness (enhanced)**

```python
# tools/bench.py enhancements

class BenchmarkResult:
    """Complete result for a single benchmark run."""
    benchmark: str
    git_rev: str
    timestamp: str

    # Timing (existing)
    build_time_s: float
    runtime_s: float           # median of N samples
    runtime_stddev: float      # standard deviation
    samples: list[float]       # all N samples

    # Performance counters (new)
    instructions: int          # retired instructions
    cache_misses_l1: int       # L1 data cache misses
    cache_misses_llc: int      # last-level cache misses
    branch_misses: int         # branch mispredictions
    tlb_misses: int            # TLB misses
    ipc: float                 # instructions per cycle

    # Allocation metrics (new)
    total_allocs: int          # total heap allocations
    peak_rss_kb: int           # peak resident set size
    nursery_allocs: int        # nursery allocations (Phase 1+)
    nursery_promotions: int    # objects promoted from nursery to heap
    stack_allocs: int          # escape-analysis stack allocations (Phase 2+)

    # Code quality metrics (new)
    binary_size_kb: int        # output binary size
    instruction_count: int     # total instructions in compiled code
    simd_instruction_pct: float # percentage of SIMD instructions
    branch_count: int          # total branch instructions
    ic_hit_rate: float         # inline cache hit rate (Phase 0+)
    unbox_rate: float          # percentage of values unboxed (Phase 2+)

    # Compilation breakdown (new)
    frontend_time_ms: float    # Python parsing + AST lowering
    tir_construction_ms: float # SimpleIR → TIR
    tir_optimization_ms: float # TIR optimization passes
    backend_lowering_ms: float # TIR → backend IR
    codegen_ms: float          # backend → machine code
    linking_ms: float          # linking + relocation

    # Competitive comparison
    cpython_runtime_s: float
    speedup_vs_cpython: float
    codon_runtime_s: Optional[float]
    nuitka_runtime_s: Optional[float]
```

**12.2.2 Regression Detection**

```python
# tools/bench_regression.py

class RegressionDetector:
    """Statistical regression detection for benchmark results."""

    # Thresholds
    REGRESSION_THRESHOLD = 0.03    # 3% slowdown triggers warning
    SEVERE_THRESHOLD = 0.10        # 10% slowdown blocks merge
    IMPROVEMENT_THRESHOLD = 0.05   # 5% speedup is noteworthy

    def detect(self, baseline: list[BenchmarkResult],
               current: list[BenchmarkResult]) -> RegressionReport:
        """
        Compare baseline and current results using Welch's t-test.

        Statistical approach:
        1. Welch's t-test (unequal variances) on sample distributions
        2. Effect size (Cohen's d) to quantify magnitude
        3. p-value < 0.05 for statistical significance
        4. Only flag if BOTH statistically significant AND exceeds threshold

        This prevents false positives from noise while catching real regressions.
        """
        report = RegressionReport()
        for bench in benchmarks:
            baseline_samples = get_samples(baseline, bench)
            current_samples = get_samples(current, bench)

            t_stat, p_value = welch_t_test(baseline_samples, current_samples)
            effect_size = cohens_d(baseline_samples, current_samples)
            delta_pct = (mean(current_samples) - mean(baseline_samples)) / mean(baseline_samples)

            if p_value < 0.05 and abs(delta_pct) > self.REGRESSION_THRESHOLD:
                if delta_pct > self.SEVERE_THRESHOLD:
                    report.add_blocker(bench, delta_pct, p_value, effect_size)
                elif delta_pct > self.REGRESSION_THRESHOLD:
                    report.add_warning(bench, delta_pct, p_value, effect_size)
            elif p_value < 0.05 and delta_pct < -self.IMPROVEMENT_THRESHOLD:
                report.add_improvement(bench, delta_pct, p_value, effect_size)

        return report
```

**12.2.3 Per-Pass Performance Attribution**

```python
# Measure the impact of each TIR optimization pass independently

class PassAttributor:
    """Measures performance impact of each optimization pass."""

    def attribute(self, benchmark: str) -> dict[str, PassImpact]:
        """
        For each pass P in [unboxing, escape, alias, sccp, gvn, licm, bce, fusion, ...]:
            1. Run all passes EXCEPT P
            2. Measure runtime
            3. Compare with all-passes-enabled runtime
            4. Delta = impact of pass P

        This isolates the contribution of each pass, accounting for interactions.
        """
        all_passes_time = run_with_all_passes(benchmark)
        impacts = {}

        for pass_name in TIR_PASSES:
            without_time = run_without_pass(benchmark, pass_name)
            delta = (without_time - all_passes_time) / all_passes_time
            impacts[pass_name] = PassImpact(
                pass_name=pass_name,
                delta_pct=delta,
                absolute_ms=without_time - all_passes_time,
            )

        return impacts
```

**12.2.4 Continuous Benchmarking Pipeline**

```yaml
# CI configuration for benchmark runs

# On every commit to main:
smoke_benchmarks:
  benchmarks: [bench_sum, bench_bytes_find, bench_fib]
  samples: 5
  timeout: 30s
  fail_on: regression > 10%

# On every PR:
full_benchmarks:
  benchmarks: all
  samples: 10
  timeout: 300s
  fail_on: regression > 5%
  compare_with: main branch HEAD
  output: pr_comment with delta table

# Weekly (scheduled):
competitive_benchmarks:
  benchmarks: all
  compilers: [molt, cpython, codon, nuitka, pypy]
  samples: 20
  timeout: 3600s
  output: HTML dashboard + JSON artifacts
  alert_on: any benchmark where Molt is not fastest

# Nightly (scheduled):
stress_benchmarks:
  benchmarks: all
  flags: [--release, --release --pgo, --release --pgo --bolt]
  samples: 50
  timeout: 7200s
  output: trend report with confidence intervals
```

### 12.3 Optimization Pass Verification

**12.3.1 Pass Invariant Checking**

```rust
/// Every TIR optimization pass MUST maintain these invariants:
trait TirPass {
    fn run(&self, func: &mut TirFunction);

    /// Called automatically in debug builds after every pass
    fn verify_invariants(func: &TirFunction) {
        verify_ssa(func);              // every use dominated by def
        verify_types(func);            // all operations type-correct
        verify_terminators(func);      // every block ends with terminator
        verify_block_args(func);       // branch args match block params
        verify_no_orphan_values(func); // no values without uses (unless side-effecting)
        verify_escape_annotations(func); // escape info consistent with actual escapes
    }
}

/// SSA verification
fn verify_ssa(func: &TirFunction) {
    let dom_tree = compute_dominators(func);
    for block in &func.blocks {
        for op in &block.ops {
            for operand in &op.operands {
                let def_block = get_defining_block(operand);
                assert!(
                    dom_tree.dominates(def_block, block.id),
                    "Use of {:?} in block {} not dominated by def in block {}",
                    operand, block.id, def_block
                );
            }
        }
    }
}

/// Type verification
fn verify_types(func: &TirFunction) {
    for block in &func.blocks {
        for op in &block.ops {
            match op.opcode {
                OpCode::Add => {
                    assert_eq!(op.operands[0].ty, op.operands[1].ty,
                        "Add operands must have same type");
                    assert_eq!(op.results[0].ty, op.operands[0].ty,
                        "Add result must match operand type");
                }
                OpCode::Unbox => {
                    assert!(matches!(op.operands[0].ty, TirType::DynBox | TirType::Box(_)),
                        "Unbox input must be boxed");
                    assert!(!matches!(op.results[0].ty, TirType::DynBox | TirType::Box(_)),
                        "Unbox output must be unboxed");
                }
                // ... for every opcode
            }
        }
    }
}
```

**12.3.2 Pass Bisection Tool**

```bash
# When a benchmark regresses, bisect which pass caused it:
molt debug --bisect-passes bench_word_count.py

# Output:
# Pass 1 (type_refinement): 0.031s → 0.031s (no change)
# Pass 2 (unboxing):        0.031s → 0.028s (improvement: -9.7%)
# Pass 3 (escape_analysis): 0.028s → 0.028s (no change)
# Pass 4 (alias_analysis):  0.028s → 0.028s (no change)
# Pass 5 (sccp):            0.028s → 0.027s (improvement: -3.6%)
# Pass 6 (gvn):             0.027s → 0.027s (no change)
# Pass 7 (licm):            0.027s → 0.025s (improvement: -7.4%)
# Pass 8 (bce):             0.025s → 0.025s (no change)
# Pass 9 (fusion):          0.025s → 0.018s (improvement: -28.0%) ← biggest win
# ...
```

### 12.4 Memory Safety Verification

**12.4.1 Address Sanitizer (ASAN)**

```bash
# Build with ASAN for all phases:
RUSTFLAGS="-Z sanitizer=address" molt compile --dev app.py

# Run all benchmarks under ASAN:
tools/bench.py --sanitizer=asan --benchmarks=all

# Catches:
# - Use-after-free (especially with nursery allocator)
# - Buffer overflows (especially with container specialization)
# - Stack buffer overflow (with stack allocation from escape analysis)
# - Double-free (with refcount elimination)
```

**12.4.2 Thread Sanitizer (TSAN)**

```bash
# For parallel execution verification:
RUSTFLAGS="-Z sanitizer=thread" molt compile --dev --par app.py

# Catches:
# - Data races in parallel loops
# - Lock order violations
# - Use of non-thread-safe operations across threads
# - Biased refcount races
```

**12.4.3 Memory Leak Detection**

```bash
# Valgrind for cycle collection verification:
valgrind --leak-check=full --show-leak-kinds=all ./app

# Custom leak detector for nursery:
MOLT_NURSERY_LEAK_CHECK=1 ./app
# Verifies: no nursery references escape without promotion
```

**12.4.4 Miri for Unsafe Code**

```bash
# For Rust runtime unsafe blocks:
cargo +nightly miri test -p molt-runtime

# Catches:
# - Undefined behavior in NaN-boxing operations
# - Invalid pointer arithmetic in object layout
# - Violation of Rust aliasing rules in intrinsics
```

### 12.5 GPU Verification

```python
# GPU correctness verification framework

class GPUVerifier:
    """Verify GPU kernel results against CPU reference implementation."""

    FLOAT_TOLERANCE = 1e-6   # absolute tolerance for f32
    DOUBLE_TOLERANCE = 1e-10  # absolute tolerance for f64

    def verify_kernel(self, kernel_func, test_inputs, expected_outputs):
        """
        1. Run kernel on GPU
        2. Run same computation on CPU (reference)
        3. Compare within floating-point tolerance
        4. Handle: NaN propagation, denormal behavior, reduction ordering
        """
        gpu_results = run_on_gpu(kernel_func, test_inputs)
        cpu_results = run_on_cpu(kernel_func, test_inputs)

        for i, (gpu_val, cpu_val) in enumerate(zip(gpu_results, cpu_results)):
            if is_nan(gpu_val) and is_nan(cpu_val):
                continue  # both NaN: OK
            assert abs(gpu_val - cpu_val) < self.tolerance_for_type(gpu_val), \
                f"GPU/CPU mismatch at index {i}: gpu={gpu_val}, cpu={cpu_val}"

    def verify_determinism(self, kernel_func, test_inputs, runs=10):
        """Verify GPU produces deterministic results across runs."""
        results = [run_on_gpu(kernel_func, test_inputs) for _ in range(runs)]
        for i in range(1, runs):
            assert results[i] == results[0], \
                f"Non-deterministic GPU result on run {i}"
```

### 12.6 WASM Verification

```bash
# WASM-specific verification

# 1. Validate WASM binary:
wasm-validate output.wasm

# 2. Verify in multiple runtimes:
wasmtime run output.wasm          # Wasmtime
wasmer run output.wasm            # Wasmer
node --experimental-wasm-eh output.js  # V8 (Node.js)

# 3. Verify binary size budget:
wasm-size output.wasm              # custom tool: report per-section sizes
assert size < 1MB                  # after Brotli compression

# 4. Verify component model compliance:
wasm-tools component validate output.wasm

# 5. Differential test: WASM vs native output
molt compile --native app.py -o app_native
molt compile --wasm app.py -o app.wasm
diff <(./app_native) <(wasmtime run app.wasm)
```

### 12.7 Additional Verification Requirements

**[VG1] PGO profile format verification:**
```bash
# After PGO instrumentation run:
# 1. Verify profile data is well-formed
llvm-profdata show app.profraw  # must parse without errors
# 2. Verify counter regions match compiled functions
llvm-profdata merge app.profraw -o app.profdata
llvm-profdata show --all-functions app.profdata | grep -c "Function"
# Must match number of compiled functions ± 5%
# 3. Verify profile-use compilation succeeds
molt compile --release --pgo-use app.profdata app.py  # must not error
```

**[VG2] Cross-backend consistency (Cranelift vs LLVM):**
```bash
# Both backends consume the same TIR, must produce identical output
for bench in tests/benchmarks/*.py; do
    molt compile --backend cranelift $bench -o /tmp/cranelift_out
    molt compile --backend llvm $bench -o /tmp/llvm_out
    diff <(/tmp/cranelift_out) <(/tmp/llvm_out) || echo "MISMATCH: $bench"
done
# Any mismatch is a P0 bug — indicates a lowering correctness issue
```

**[VG3] Mutation testing:**
```bash
# Verify test suite catches real bugs (not just "tests pass"):
cargo install cargo-mutants
cargo mutants -p molt-backend --timeout 120 -- --test tir_passes
# Target: ≥ 70% mutation kill rate on TIR pass code
# Low kill rate indicates insufficient test coverage for an optimization pass
```

**[VG4] NaN-boxing boundary verification:**
```rust
/// Exhaustive tests for the 47-bit inline int boundary:
#[test]
fn nanbox_int_boundary() {
    let max_inline = (1i64 << 47) - 1;   // 140,737,488,355,327
    let min_inline = -(1i64 << 47);       // -140,737,488,355,328

    // These must use inline representation (fast path)
    assert!(is_inline_int(box_int(max_inline)));
    assert!(is_inline_int(box_int(min_inline)));
    assert_eq!(unbox_int(box_int(max_inline)), max_inline);
    assert_eq!(unbox_int(box_int(min_inline)), min_inline);

    // These must use BigInt representation (heap path)
    assert!(!is_inline_int(box_int(max_inline + 1)));
    assert!(!is_inline_int(box_int(min_inline - 1)));

    // Arithmetic at boundary: inline + inline = BigInt
    let a = box_int(max_inline);
    let b = box_int(1);
    let result = molt_add(a, b);
    assert!(!is_inline_int(result));
    assert_eq!(unbox_bigint(result), max_inline + 1);

    // Overflow in both directions
    let neg_result = molt_add(box_int(min_inline), box_int(-1));
    assert!(!is_inline_int(neg_result));
    assert_eq!(unbox_bigint(neg_result), min_inline - 1);

    // Multiplication overflow
    let mul_result = molt_mul(box_int(1 << 24), box_int(1 << 24));
    assert!(!is_inline_int(mul_result));  // 2^48 > 2^47-1
}
```

**[VG5] TIR interpreter (for property-based testing):**

The property-based tests in Section 12.1.3 require a TIR interpreter to verify
optimization correctness. This is a separate, simple, reference implementation:

```rust
/// Simple tree-walking interpreter for TIR programs.
/// NOT performance-optimized — correctness is the only goal.
/// Used exclusively for testing (never in production compilation).
struct TirInterpreter {
    values: HashMap<ValueId, RuntimeValue>,  // SSA value store
    heap: Vec<RuntimeObject>,                // simulated heap
    call_stack: Vec<StackFrame>,
}

impl TirInterpreter {
    /// Execute a TIR function and return its result.
    /// Supports all TIR operations including control flow, allocation, boxing.
    /// Does NOT support GPU or parallel operations (test those separately).
    fn execute(&mut self, func: &TirFunction, args: &[RuntimeValue]) -> RuntimeValue {
        // Walk blocks, execute ops, follow terminators
        // This must be implemented in Phase 1 alongside TIR data structures
    }
}
```

Build in Phase 1 alongside [1.1] TIR data structures. Required before any
optimization pass can be property-tested.

**[VG6] Type verifier mixed-type arithmetic rules:**
```rust
/// The verify_types function must handle Python's mixed-type arithmetic:
fn verify_add_types(op: &TirOp) {
    let lhs = op.operands[0].ty;
    let rhs = op.operands[1].ty;
    let result = op.results[0].ty;

    match (lhs, rhs) {
        // Same type: result matches
        (I64, I64) => assert_eq!(result, I64),
        (F64, F64) => assert_eq!(result, F64),
        (Str, Str) => assert_eq!(result, Str),

        // Mixed numeric: Python promotes to float
        (I64, F64) | (F64, I64) => assert_eq!(result, F64),
        (Bool, I64) | (I64, Bool) => assert_eq!(result, I64),
        (Bool, F64) | (F64, Bool) => assert_eq!(result, F64),

        // DynBox: result is DynBox (can't know at compile time)
        (DynBox, _) | (_, DynBox) => assert_eq!(result, DynBox),

        // List + List: concatenation → same list type
        (List(t1), List(t2)) if t1 == t2 => assert_eq!(result, List(t1)),

        // Other combinations: must be DynBox or Union
        _ => assert!(matches!(result, DynBox | Union(_))),
    }
}
```

### 12.8 Optimization Gate Criteria

**No optimization pass is merged without:**

1. **Correctness proof:** Differential test suite passes (100% of existing tests)
2. **No regressions:** Full benchmark suite shows no regression > 3% (Welch's t-test, p < 0.05)
3. **Measurable improvement:** At least one benchmark improves > 5% (statistically significant)
4. **Memory safety:** ASAN clean on full benchmark suite
5. **TIR invariants:** Pass invariant checker passes on all benchmarks
6. **Property tests:** 10,000 proptest iterations pass
7. **Fuzzing:** 1 hour of fuzzing with no crashes

**For LLVM passes additionally:**
8. **LLVM verifier:** `llvm::verifyFunction` passes after pass execution
9. **Alive2 verification:** Critical patterns verified with Alive2 (LLVM's translation validation tool)

**For GPU passes additionally:**
10. **GPU/CPU parity:** All test inputs produce matching results (within tolerance)
11. **Determinism:** 10 consecutive runs produce identical results

---

## 13. Dependency Graph

```
PHASE 0 (W1-2): BEAT CPYTHON
├── [0.1] Benchmark audit + profiling ─────────────────── no deps
├── [0.2] Inline cache system ─────────────────────────── no deps
├── [0.3] Dictionary optimization ─────────────────────── no deps
├── [0.4] String SSO + interning → [0.7] ─────────────── no deps
├── [0.5] Fast path completeness ──────────────────────── depends on [0.1] audit results
├── [0.6] Hot/cold header split ───────────────────────── no deps
└── [0.7] String representation overhaul ──────────────── depends on [0.4]

PHASE 1 (W3-5): FOUNDATIONS
├── Track A: TIR
│   ├── [1.1] TIR data structures ─────────────────────── no deps
│   ├── [1.2] CFG extraction ──────────────────────────── depends on [1.1]
│   ├── [1.3] SSA conversion ─────────────────────────── depends on [1.2]
│   ├── [1.4] Type refinement ────────────────────────── depends on [1.3]
│   └── [1.5] TIR → SimpleIR back-conversion ─────────── depends on [1.4]
│
├── Track B: LLVM (parallel with Track A)
│   ├── [1.6] Inkwell integration ─────────────────────── no deps
│   ├── [1.7] SimpleIR → LLVM IR ─────────────────────── depends on [1.6]
│   ├── [1.8] Runtime imports ─────────────────────────── depends on [1.6]
│   └── [1.9] End-to-end test ────────────────────────── depends on [1.7, 1.8]
│
└── Track C: Allocator (parallel with A and B)
    ├── [1.10] mimalloc ───────────────────────────────── no deps
    └── [1.11] Nursery allocator ──────────────────────── depends on [1.10]

PHASE 2 (W6-8): OPTIMIZATION CORE
├── Track D: TIR Passes (depends on Track A complete)
│   ├── [2.1] Unboxing ───────────────────────────────── depends on [1.4]
│   ├── [2.2] Escape analysis ────────────────────────── depends on [1.3]
│   ├── [2.3] Alias analysis ─────────────────────────── depends on [1.4]
│   ├── [2.4] SCCP ───────────────────────────────────── depends on [1.3]
│   ├── [2.5] GVN ────────────────────────────────────── depends on [2.3]
│   ├── [2.6] LICM ───────────────────────────────────── depends on [2.3, 1.2 loop tree]
│   ├── [2.7] Monomorphization ───────────────────────── depends on [1.4]
│   ├── [2.15] Bounds check elimination ──────────────── depends on [1.2, 1.4]
│   ├── [2.16] Deforestation / iterator fusion ───────── depends on [1.3, 1.4]
│   └── [2.17] Closure/lambda specialization ─────────── depends on [2.2, 2.7]
│
├── Track E: LLVM Passes (depends on Track B complete)
│   ├── [2.8] AllocationRemover ──────────────────────── depends on [1.9]
│   ├── [2.9] RefcountEliminator ─────────────────────── depends on [1.9]
│   ├── [2.10] BoxingEliminator ──────────────────────── depends on [1.9]
│   └── [2.11] TypeGuardHoister ──────────────────────── depends on [1.9]
│
└── Track F: Memory (parallel with D and E)
    ├── [2.12] Biased refcounting ────────────────────── depends on [0.6 hot/cold split]
    ├── [2.13] Cycle detection ───────────────────────── no deps
    └── [2.14] Container specialization ──────────────── depends on [1.4 type refinement]

PHASE 3 (W9-11): INTEGRATION (depends on Phase 2 complete)
├── [3.1] TIR → LLVM full lowering ──────────────────── depends on [Track D, Track E]
├── [3.2] Escape → AllocationRemover wiring ─────────── depends on [2.2, 2.8]
├── [3.3] Types → BoxingEliminator wiring ───────────── depends on [2.1, 2.10]
├── [3.4] PGO instrumentation ───────────────────────── depends on [3.1]
├── [3.5] PGO-guided optimization ───────────────────── depends on [3.4]
├── [3.6] Deoptimization framework ──────────────────── depends on [3.1, 3.5]
├── [3.7] LTO / ThinLTO ────────────────────────────── depends on [3.1]
├── [3.8] CoroutineElider ──────────────────────────── depends on [3.1]
├── [3.9] Interprocedural optimization ──────────────── depends on [3.1, 3.7]
├── [3.10] E-graph integration ─────────────────────── depends on [1.1]
├── [3.11] Class hierarchy analysis ─────────────────── depends on [1.1, 3.1]
├── [3.12] BOLT post-link ──────────────────────────── depends on [3.5]
└── [3.13] Software prefetching ─────────────────────── depends on [3.1]

PHASE 4 (W12-14): WASM + PARALLELISM (depends on Phase 3 complete)
├── Track G: WASM ──────────────────────────── parallel with H and I
├── Track H: Parallelism ──────────────────── parallel with G and I
└── Track I: SIMD ─────────────────────────── parallel with G and H

PHASE 5 (W15-18): GPU + MLIR (depends on Phase 4)
├── Track J: Metal ────────────────────────── depends on [Track H parallelism infra]
├── Track K: WebGPU ───────────────────────── depends on [Track J Metal patterns]
└── Track L: MLIR ─────────────────────────── parallel with J and K

PHASE 6 (W19-24): FRONTIER (depends on Phase 5)
├── [6.1] CUDA ─────────────────────────────── depends on [Track J patterns]
├── [6.2] AMD ──────────────────────────────── depends on [Track J patterns]
├── [6.3] GIL removal ─────────────────────── depends on [2.12, 4.12]
├── [6.4] Polyhedral ──────────────────────── depends on [Track L MLIR]
├── [6.5] MLIR migration ─────────────────── depends on [Track L validation]
├── [6.6] CI benchmarking ─────────────────── no deps (can start anytime)
└── [6.7] Competitive dashboard ───────────── depends on [6.6]
```

### Parallelism Summary

| Phase | Parallel Tracks | Total Calendar Weeks |
|-------|----------------|---------------------|
| 0 | 7 tasks, all independent except [0.5] depends on [0.1] | 2 |
| 1 | 3 tracks (A, B, C) fully parallel | 3 |
| 2 | 3 tracks (D, E, F) fully parallel | 3 |
| 3 | ~13 tasks, mostly sequential (integration) | 3 |
| 4 | 3 tracks (G, H, I) fully parallel | 3 |
| 5 | 3 tracks (J, K, L) mostly parallel | 4 |
| 6 | 7 tasks, some parallel | 6 |
| **Total** | | **24 weeks** |

Without parallelism this would be ~50+ weeks of sequential work. The critical path design saves ~50% calendar time.

---

## 14. Risk Registry

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| LLVM compilation speed too slow for dev mode | Medium | Low | Keep Cranelift as dev backend; LLVM only for `--release` |
| Type inference unsound (infers wrong type) | Medium | Critical | Property-based testing, differential testing, conservative fallback to DynBox |
| Escape analysis too conservative (nothing classified NoEscape) | Medium | Medium | Start with simple patterns (no-arg functions, list comprehensions), expand |
| NaN-boxing incompatible with unboxed paths | Low | High | Clean separation: unboxed values never enter NaN-boxing codepath; type guards at boundary |
| LLVM custom passes break on LLVM version upgrades | Medium | Medium | Pin LLVM version; comprehensive pass tests; Alive2 verification for critical patterns |
| Nursery allocator introduces use-after-free | Medium | Critical | ASAN on all benchmarks; nursery references tracked; conservative promotion on uncertainty |
| GPU results don't match CPU (floating-point) | High | Medium | Explicit tolerance thresholds; document differences; `@fast_math` opt-in only |
| WASM binary size exceeds target | Medium | Medium | Aggressive tree shaking; lazy module loading; Brotli compression |
| Cycle collector pauses too long | Low | Medium | Incremental collection (256 roots/step); concurrent mark phase |
| Monomorphization explosion (too many specializations) | Medium | Medium | Depth limit (4); cache size limit; fallback to generic |
| BOLT not available on all platforms | Medium | Low | Optional pass; `--bolt` flag; graceful fallback |
| MLIR integration harder than expected | Medium | Low | MLIR is Phase 5-6; TIR-in-Rust works standalone; migration is optional/incremental |

---

## 15. Success Criteria

### Phase 0 (Weeks 1-2)
- [ ] All 61+ benchmarks faster than CPython 3.12
- [ ] Inline cache hit rate ≥ 80% on OO benchmarks
- [ ] String benchmarks improved ≥ 1.5x
- [ ] Dict benchmarks improved ≥ 1.5x
- [ ] No regressions on any existing test

### Phase 1 (Weeks 3-5)
- [ ] TIR_DUMP=1 produces readable output for all benchmarks
- [ ] TIR round-trip (SimpleIR → TIR → SimpleIR) is semantically identical
- [ ] LLVM backend compiles and runs bench_sum.py correctly
- [ ] LLVM -O3 binary ≥ 1.2x faster than Cranelift
- [ ] mimalloc shows measurable allocation improvement

### Phase 2 (Weeks 6-8)
- [ ] Type refinement resolves ≥ 80% of values on annotated code
- [ ] Escape analysis: ≥ 40% of allocations classified NoEscape
- [ ] Unboxing eliminates ≥ 80% of box/unbox pairs on typed code
- [ ] Bounds checks eliminated in ≥ 90% of range(len(x)) patterns
- [ ] Iterator fusion fires on all map/filter/reduce chains
- [ ] All custom LLVM passes pass correctness verification

### Phase 3 (Weeks 9-11)
- [ ] LLVM release binary ≥ 1.5x faster than Cranelift dev on all benchmarks
- [ ] PGO adds ≥ 10% on polymorphic benchmarks
- [ ] BOLT adds ≥ 5% on top of PGO+LTO
- [ ] **Match or beat Codon on `word_count.py`**
- [ ] **Match or beat Codon on `taq.py`**
- [ ] **Beat Nuitka on all benchmarks by ≥ 2x**

### Phase 4 (Weeks 12-14)
- [ ] WASM native EH: 20%+ speedup on exception benchmarks
- [ ] WASM binary size < 1MB (Brotli compressed)
- [ ] Parallel compilation: 3x+ speedup on 4-core
- [ ] Auto-vectorization fires on typed array loops
- [ ] @par decorator works end-to-end
- [ ] Incremental compilation: < 100ms for single-function change

### Phase 5 (Weeks 15-18)
- [ ] Metal GPU: mandelbrot kernel runs on Apple Silicon
- [ ] WebGPU: same kernel runs in browser
- [ ] TIR → MLIR round-trip validates for all benchmarks
- [ ] MLIR dialect specification complete

### Phase 6 (Weeks 19-24)
- [ ] CUDA and AMD backends functional
- [ ] GIL-free parallel execution working
- [ ] Continuous benchmarking pipeline operational
- [ ] **Molt is the fastest Python compiler on ≥ 90% of benchmarks**
- [ ] **Molt is within 20% of Codon on the remaining 10%**

### Ultimate Target
```
For every benchmark B in the suite:
    molt_time(B) < cpython_time(B)     — ALWAYS
    molt_time(B) < nuitka_time(B)      — ALWAYS
    molt_time(B) ≈ codon_time(B)       — WITHIN 20% (most benchmarks)
    molt_time(B) < codon_time(B)       — ON MANY BENCHMARKS (string, dict, OO)

For GPU compute:
    molt_gpu_time(B) competitive with native Metal/CUDA

For WASM:
    molt_wasm_size(B) < 1MB compressed
    molt_wasm_time(B) < cpython_time(B) — ALWAYS
```

---

*This document defines the complete optimization roadmap for Project TITAN. Each phase is gated by its success criteria before proceeding to the next. The verification harness ensures correctness is never sacrificed for performance.*
