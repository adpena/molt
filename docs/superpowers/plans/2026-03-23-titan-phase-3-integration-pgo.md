# Project TITAN Phase 3: Integration + PGO

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the TIR → LLVM IR lowering with full type information, add custom LLVM passes (AllocationRemover, BoxingEliminator, RefcountEliminator, TypeGuardHoister), wire PGO instrumentation, add class hierarchy analysis for devirtualization, and enable LTO. Target: match or beat Codon on word_count and taq benchmarks.

**Architecture:** TIR (optimized by Phase 2 passes) → LLVM IR (type-specialized) → Custom LLVM passes → Stock LLVM -O3 → Native binary. PGO adds profile data that feeds back into both TIR and LLVM optimization decisions.

**Tech Stack:** Rust, inkwell 0.8 (LLVM 18), LLVM C API for custom passes

**Spec Reference:** `docs/superpowers/specs/2026-03-23-project-titan-optimization-design.md` — Section 8

**Prerequisites:** Phase 2 complete. LLVM 18 installed at `/opt/homebrew/opt/llvm@18`.

**Environment:** `LLVM_SYS_181_PREFIX=/opt/homebrew/opt/llvm@18`

---

## Task 3.1: TIR → LLVM IR Lowering (Full)

**Files:**
- Create: `runtime/molt-backend/src/llvm_backend/lowering.rs`
- Create: `runtime/molt-backend/src/llvm_backend/runtime_imports.rs`
- Create: `runtime/molt-backend/src/llvm_backend/types.rs`
- Modify: `runtime/molt-backend/src/llvm_backend/mod.rs`

The core of Phase 3. Maps every TIR operation to LLVM IR with type-specialized code generation.

### Type mapping:
```
TIR I64        → LLVM i64 (bare register, no boxing)
TIR F64        → LLVM double
TIR Bool       → LLVM i1 (promoted to i8 at ABI boundaries)
TIR None       → LLVM i64 (sentinel constant: QNAN | TAG_NONE)
TIR DynBox     → LLVM i64 (NaN-boxed, runtime dispatch)
TIR Str        → LLVM ptr (to MoltString)
TIR List(T)    → LLVM ptr (to MoltList)
TIR Tuple(...) → LLVM struct type { field_types... }
TIR Ptr(T)     → LLVM ptr
```

### Operation lowering (type-specialized):
```
molt.add I64, I64     → llvm: add i64 %a, %b (with overflow → BigInt)
molt.add F64, F64     → llvm: fadd double %a, %b
molt.add DynBox, DynBox → llvm: call @molt_dyn_add(i64 %a, i64 %b)
molt.box I64          → llvm: or i64 %val, QNAN_TAG_INT
molt.unbox I64        → llvm: and i64 %val, INT_MASK
molt.alloc            → llvm: call @molt_alloc(i32 %size)
molt.stack_alloc      → llvm: alloca %Type, align 8
molt.call             → llvm: call (direct for devirtualized, indirect otherwise)
molt.inc_ref          → llvm: call @molt_inc_ref (or elided for NoEscape)
molt.dec_ref          → llvm: call @molt_dec_ref (or elided for NoEscape)
```

### Implementation approach:
- `types.rs`: Map TirType → inkwell BasicTypeEnum
- `runtime_imports.rs`: Declare all runtime functions as LLVM function declarations
- `lowering.rs`: Walk TIR blocks in RPO, emit LLVM IR for each op, handle terminators
- For DynBox operands: emit runtime function calls
- For concrete types (I64, F64): emit native LLVM instructions (add, fadd, etc.)
- For StackAlloc ops: emit `alloca` (escape analysis already classified these)
- Emit LLVM metadata for branch weights (from type_refine confidence)

### End-to-end milestone:
`bench_sum.py` compiles via TIR → LLVM → .o → linked binary → runs → correct output.

---

## Task 3.2: Custom LLVM Passes (as TIR Pre-Lowering)

**Files:**
- Create: `runtime/molt-backend/src/tir/passes/refcount_elim.rs`
- Create: `runtime/molt-backend/src/tir/passes/type_guard_hoist.rs`
- Modify: `runtime/molt-backend/src/tir/passes/mod.rs`

Rather than writing raw LLVM C++ passes, we implement the "custom LLVM pass" optimizations at TIR level (before LLVM lowering). The LLVM lowering then emits already-optimized IR, and LLVM's stock -O3 handles the rest.

### Refcount Elimination (spec [3.2/2.9]):
```rust
/// Eliminate redundant IncRef/DecRef pairs.
/// Patterns:
///   IncRef(x); DecRef(x)              → both removed
///   IncRef(x); f(x); DecRef(x)       → both removed if f doesn't store x
///   NoEscape values                    → all refcount ops removed (already done by escape analysis)
///   SSA value with lifetime covering all uses → refcount provably redundant
pub fn run(func: &mut TirFunction) -> PassStats
```

### Type Guard Hoisting (spec [3.3/2.11]):
```rust
/// Hoist TypeGuard ops out of loops when the guarded value's type is loop-invariant.
/// Before: loop { guard(x, INT); use_as_int(x) }
/// After:  guard(x, INT); loop { use_as_int(x) }
pub fn run(func: &mut TirFunction) -> PassStats
```

Uses loop depth from CFG to identify loop bodies, then checks if the TypeGuard's operand is defined outside the loop (loop-invariant).

---

## Task 3.3: Class Hierarchy Analysis

**Files:**
- Create: `runtime/molt-backend/src/tir/passes/cha.rs`
- Modify: `runtime/molt-backend/src/tir/function.rs` (add ClassHierarchy to TirModule)

### What it does:
Build the whole-program class hierarchy from TIR, then devirtualize method calls on leaf classes (classes with no subclasses).

```rust
pub struct ClassHierarchy {
    /// parent_class → set of child classes
    children: HashMap<String, HashSet<String>>,
    /// class → set of methods defined directly (not inherited)
    methods: HashMap<String, HashSet<String>>,
}

impl ClassHierarchy {
    /// Returns true if the class has no subclasses in the whole program.
    pub fn is_leaf_class(&self, class_name: &str) -> bool

    /// For a method call on a known type, returns the concrete function name
    /// if the call can be devirtualized.
    pub fn resolve_method(&self, class_name: &str, method_name: &str) -> Option<String>
}
```

### Devirtualization pass:
```rust
/// Replace CallMethod ops with direct Call ops when CHA proves single target.
pub fn run(func: &mut TirFunction, hierarchy: &ClassHierarchy) -> PassStats
```

---

## Task 3.4: PGO Instrumentation

**Files:**
- Create: `runtime/molt-backend/src/llvm_backend/pgo.rs`
- Modify: `runtime/molt-backend/src/llvm_backend/lowering.rs`

### Instrumentation mode:
When `--pgo-instrument` is passed:
1. Insert LLVM's `@llvm.instrprof.increment` intrinsic at function entries and branch points
2. Compile and link with `-fprofile-generate` equivalent flags
3. Running the instrumented binary produces a `.profraw` file

### Profile-use mode:
When `--pgo-use <profile>` is passed:
1. Load profile data via `llvm-profdata merge`
2. Attach branch weight metadata to CondBranch terminators
3. Pass profile to LLVM's PGO-aware optimization passes

### Implementation via inkwell:
```rust
/// Add PGO instrumentation to the LLVM module.
pub fn add_pgo_instrumentation(module: &Module, func_name: &str) {
    // Insert @llvm.instrprof.increment at function entry
    // Insert at each branch point
}

/// Load PGO profile and attach metadata.
pub fn apply_pgo_profile(module: &Module, profile_path: &str) {
    // Use LLVM C API: LLVMSetModuleProfileData or equivalent
}
```

---

## Task 3.5: LTO / ThinLTO

**Files:**
- Modify: `runtime/molt-backend/src/llvm_backend/mod.rs`

### Implementation:
```rust
pub enum LtoMode {
    None,
    Thin,   // default for --release
    Full,   // maximum optimization
}

impl LlvmBackend {
    /// Emit LLVM bitcode for LTO.
    pub fn emit_bitcode(&self, path: &Path) {
        self.module.write_bitcode_to_path(path);
    }

    /// Run LLVM optimization pipeline with LTO.
    pub fn optimize(&self, opt_level: OptimizationLevel, lto: LtoMode) {
        // Configure pass manager
        // For ThinLTO: emit module summary
        // For Full LTO: merge all modules
    }
}
```

---

## Task 3.6: Interprocedural Optimization

**Files:**
- Create: `runtime/molt-backend/src/tir/passes/interprocedural.rs`

### What it does:
1. Build call graph from TirModule (which functions call which)
2. Cross-function constant propagation: if f(x) always called with x=5, specialize
3. Cross-function inlining: inline small functions (≤30 TIR ops, no loops) at call sites
4. Dead function elimination: remove unreachable functions

```rust
pub struct CallGraph {
    /// caller → list of (callee, call_site_count)
    edges: HashMap<String, Vec<(String, usize)>>,
}

/// Build call graph from a TIR module.
pub fn build_call_graph(module: &TirModule) -> CallGraph

/// Inline small callees into callers.
pub fn inline_small_functions(module: &mut TirModule, graph: &CallGraph) -> PassStats

/// Remove functions not reachable from entry points.
pub fn eliminate_dead_functions(module: &mut TirModule, graph: &CallGraph) -> PassStats
```

---

## Task 3.7: Pipeline Integration

**Files:**
- Modify: `runtime/molt-backend/src/tir/passes/mod.rs`
- Create: `runtime/molt-backend/src/llvm_backend/pipeline.rs`

Wire everything into a single compilation pipeline:

```rust
/// Full release-mode compilation pipeline.
pub fn compile_release(ir: &FunctionIR, opts: &CompileOptions) -> Vec<u8> {
    // 1. SimpleIR → TIR
    let mut tir = lower_to_tir(ir);

    // 2. Type refinement
    type_refine::refine_types(&mut tir);

    // 3. TIR optimization passes
    passes::run_pipeline(&mut tir);

    // 4. Additional Phase 3 passes
    refcount_elim::run(&mut tir);
    type_guard_hoist::run(&mut tir);

    // 5. TIR → LLVM IR
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, &tir.name);
    lowering::lower_tir_to_llvm(&tir, &backend);

    // 6. LLVM optimization (-O3)
    backend.optimize(OptimizationLevel::Aggressive, opts.lto);

    // 7. Emit object code
    backend.emit_object_code()
}
```

---

## Execution Order

```
Task 3.1 (LLVM lowering) ─────────────────────── FIRST (everything depends on this)
    │
    ├──→ Task 3.2 (refcount elim + type guard hoist) ── parallel with 3.3
    ├──→ Task 3.3 (CHA devirtualization) ────────────── parallel with 3.2
    │
    ├──→ Task 3.4 (PGO instrumentation) ─────────────── after 3.1
    ├──→ Task 3.5 (LTO) ────────────────────────────── after 3.1
    │
    └──→ Task 3.6 (interprocedural) ─────────────────── after 3.3 (needs call graph)
         │
         └──→ Task 3.7 (pipeline integration) ───────── LAST (wires everything)
```

## Constraints

- `LLVM_SYS_181_PREFIX=/opt/homebrew/opt/llvm@18` required for all builds
- HashMap/HashSet only (no BTree)
- No O(N²)
- All `unsafe` documented with safety invariants
- TIR verifier gate after every TIR pass
- Every LLVM IR function verified with `verify_function()` from inkwell
- CPython parity maintained (parity gate runs on all changes)
