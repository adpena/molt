# Project TITAN Phase 1: Foundations (TIR + LLVM + Allocator)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Typed IR (TIR) layer, scaffold the LLVM backend, and improve the allocator — the three foundations that unlock all subsequent optimization phases.

**Architecture:** Three parallel tracks that converge in Phase 3:
- **Track A (TIR):** SimpleIR → CFG extraction → SSA conversion → Type refinement → Back-conversion to SimpleIR (so existing Cranelift/WASM backends can consume TIR output)
- **Track B (LLVM):** Inkwell integration → SimpleIR → LLVM IR lowering → Runtime imports → End-to-end compilation of bench_sum.py
- **Track C (Allocator):** Switch to mimalloc → Nursery bump allocator with write barrier

**Tech Stack:** Rust, inkwell (LLVM 18 bindings), mimalloc crate, rayon (for future parallel compilation)

**Spec Reference:** `docs/superpowers/specs/2026-03-23-project-titan-optimization-design.md` — Sections 4, 6

**Prerequisites:** Phase 0 complete (header split, IC, string interning all landed)

---

## File Structure

### Track A: TIR — New Files
| File | Responsibility |
|------|---------------|
| `runtime/molt-backend/src/tir/mod.rs` | TIR module root, re-exports |
| `runtime/molt-backend/src/tir/types.rs` | TirType enum, type lattice, meet/join ops |
| `runtime/molt-backend/src/tir/values.rs` | TirValue, ValueId, typed SSA values |
| `runtime/molt-backend/src/tir/ops.rs` | TirOp, OpCode, Dialect, AttrDict |
| `runtime/molt-backend/src/tir/blocks.rs` | TirBlock, Terminator, block arguments |
| `runtime/molt-backend/src/tir/function.rs` | TirFunction, TirModule, ClassHierarchy |
| `runtime/molt-backend/src/tir/cfg.rs` | CFG extraction from SimpleIR (dominator tree, loop detection) |
| `runtime/molt-backend/src/tir/ssa.rs` | SSA conversion (iterated dominance frontier, variable renaming) |
| `runtime/molt-backend/src/tir/type_refine.rs` | Type refinement pass (annotations + inference + narrowing) |
| `runtime/molt-backend/src/tir/lower_from_simple.rs` | SimpleIR → TIR construction |
| `runtime/molt-backend/src/tir/lower_to_simple.rs` | TIR → SimpleIR back-conversion |
| `runtime/molt-backend/src/tir/printer.rs` | Human-readable TIR dump (TIR_DUMP=1) |
| `runtime/molt-backend/src/tir/verify.rs` | SSA/type/terminator invariant checking |

### Track B: LLVM — New Files
| File | Responsibility |
|------|---------------|
| `runtime/molt-backend/src/llvm_backend/mod.rs` | LLVM backend root, module creation |
| `runtime/molt-backend/src/llvm_backend/lowering.rs` | SimpleIR → LLVM IR operation lowering |
| `runtime/molt-backend/src/llvm_backend/runtime_imports.rs` | Runtime function declarations (@molt_alloc, etc.) |
| `runtime/molt-backend/src/llvm_backend/types.rs` | Type mapping (TirType → LLVM types) |

### Track C: Allocator — Modified Files
| File | Responsibility |
|------|---------------|
| `runtime/molt-runtime/Cargo.toml` | Add mimalloc dependency |
| `runtime/molt-runtime/src/lib.rs` | Set #[global_allocator] |
| `runtime/molt-runtime/src/object/nursery.rs` | New: bump allocator with write barrier |
| `runtime/molt-runtime/src/object/mod.rs` | Wire nursery into allocation path |

---

## Track A: Typed IR (TIR)

### Task A1: TIR Data Structures

**Files:**
- Create: `runtime/molt-backend/src/tir/mod.rs`, `types.rs`, `values.rs`, `ops.rs`, `blocks.rs`, `function.rs`

- [ ] **Step 1: Create tir module directory and mod.rs**

```bash
mkdir -p runtime/molt-backend/src/tir
```

Create `runtime/molt-backend/src/tir/mod.rs`:
```rust
//! Typed IR (TIR) — MLIR-compatible intermediate representation.
//! Sits between SimpleIR and all backends (Cranelift, LLVM, WASM, GPU).

pub mod types;
pub mod values;
pub mod ops;
pub mod blocks;
pub mod function;

pub use types::TirType;
pub use values::{TirValue, ValueId};
pub use ops::{TirOp, OpCode, Dialect};
pub use blocks::{TirBlock, BlockId, Terminator};
pub use function::{TirFunction, TirModule};
```

- [ ] **Step 2: Implement TirType (type lattice)**

Create `types.rs` with the type enum from spec Section 4.2:
- `I64`, `F64`, `Bool`, `None` — unboxed scalars
- `Str`, `Bytes`, `List(Box<TirType>)`, `Dict(Box<TirType>, Box<TirType>)` — reference types
- `Tuple(Vec<TirType>)`, `Set(Box<TirType>)` — containers
- `Box(Box<TirType>)` — NaN-boxed with known inner type
- `DynBox` — NaN-boxed, type unknown
- `Func(FuncSignature)`, `Closure(FuncSignature, Vec<TirType>)` — callables
- `BigInt`, `Union(Vec<TirType>)`, `Never` — special
- `Ptr(Box<TirType>)` — typed pointer

Include the meet operation (type lattice join for SSA merge points):
```rust
impl TirType {
    pub fn meet(&self, other: &TirType) -> TirType {
        if self == other { return self.clone(); }
        match (self, other) {
            (TirType::Never, t) | (t, TirType::Never) => t.clone(),
            (TirType::DynBox, _) | (_, TirType::DynBox) => TirType::DynBox,
            // Union collapse: ≤3 concrete types preserved, >3 → DynBox
            _ => {
                let union_types = collect_union_members(self, other);
                if union_types.len() <= 3 { TirType::Union(union_types) }
                else { TirType::DynBox }
            }
        }
    }
}
```

- [ ] **Step 3: Implement TirValue and ValueId**

Create `values.rs`:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Clone)]
pub struct TirValue {
    pub id: ValueId,
    pub ty: TirType,
}
```

- [ ] **Step 4: Implement TirOp and Dialect**

Create `ops.rs` with MLIR-compatible operation structure:
```rust
pub enum Dialect { Molt, Scf, Gpu, Par, Simd }

pub enum OpCode {
    // molt dialect
    Add, Sub, Mul, Div, Mod, Pow, Neg,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or, Not, BitAnd, BitOr, BitXor, Shl, Shr,
    Call, CallMethod,
    Alloc, StackAlloc, Free,
    LoadAttr, StoreAttr, IcLookup,
    Box, Unbox, TypeGuard,
    IncRef, DecRef,
    Index, StoreIndex,
    BuildList, BuildDict, BuildTuple, BuildSet,
    Iter, IterNext,
    Yield, YieldFrom,
    Raise, Deopt,
    Const, // constant value
    Copy, // SSA copy
    // scf dialect
    ScfIf, ScfFor, ScfWhile, ScfYield,
    // More to come in later phases: gpu, par, simd
}

pub struct TirOp {
    pub dialect: Dialect,
    pub opcode: OpCode,
    pub operands: Vec<TirValue>,
    pub results: Vec<TirValue>,
    pub attrs: AttrDict,
    pub source_span: Option<(u32, u32)>, // (line, col)
}
```

- [ ] **Step 5: Implement TirBlock and Terminator**

Create `blocks.rs`:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

pub struct TirBlock {
    pub id: BlockId,
    pub args: Vec<TirValue>,      // MLIR-style block arguments (no phi nodes)
    pub ops: Vec<TirOp>,
    pub terminator: Terminator,
    pub loop_depth: u32,
    pub is_cold: bool,
}

pub enum Terminator {
    Branch(BlockId, Vec<TirValue>),
    CondBranch {
        cond: TirValue,
        true_dest: BlockId, true_args: Vec<TirValue>,
        false_dest: BlockId, false_args: Vec<TirValue>,
    },
    Switch(TirValue, Vec<(i64, BlockId)>, BlockId),
    Return(Vec<TirValue>),
    Unreachable,
}
```

- [ ] **Step 6: Implement TirFunction and TirModule**

Create `function.rs`:
```rust
pub struct TirFunction {
    pub name: String,
    pub params: Vec<TirValue>,
    pub return_type: TirType,
    pub blocks: Vec<TirBlock>,
    pub entry_block: BlockId,
    pub value_counter: u32,     // for generating fresh ValueIds
    pub block_counter: u32,     // for generating fresh BlockIds
}

pub struct TirModule {
    pub name: String,
    pub functions: Vec<TirFunction>,
}
```

- [ ] **Step 7: Register tir module in molt-backend**

Add `pub mod tir;` to `runtime/molt-backend/src/lib.rs` (or wherever modules are declared).

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p molt-backend`
Expected: Compiles with no errors.

- [ ] **Step 9: Commit**

```bash
git commit -m "feat(titan-p1): add TIR data structures (types, values, ops, blocks, functions)"
```

---

### Task A2: CFG Extraction

**Files:**
- Create: `runtime/molt-backend/src/tir/cfg.rs`

Build the control-flow graph extractor that converts SimpleIR's linear op stream into basic blocks with explicit control flow.

- [ ] **Step 1: Implement CFG extraction**

The CFG extractor must:
1. Identify basic block boundaries from SimpleIR (branch targets, exception handlers, function entry)
2. Build predecessor/successor adjacency lists
3. Compute dominator tree (Lengauer-Tarjan or simple iterative algorithm)
4. Identify natural loops via back-edge detection
5. Compute loop nesting depth

Read `runtime/molt-backend/src/ir.rs` to understand SimpleIR's `OpIR` structure — specifically how control flow is represented (look for `if`, `else`, `loop_start`, `loop_end`, `jump`, `branch` opcodes).

```rust
pub struct CFG {
    pub blocks: Vec<BasicBlock>,
    pub entry: usize,
    pub predecessors: Vec<Vec<usize>>,
    pub successors: Vec<Vec<usize>>,
    pub dominators: Vec<usize>,           // idom[block] = immediate dominator
    pub loop_headers: Vec<bool>,          // true if block is a loop header
    pub loop_depth: Vec<u32>,             // nesting depth
}

pub struct BasicBlock {
    pub id: usize,
    pub ops: Vec<usize>,                  // indices into the original SimpleIR op list
    pub start_op: usize,
    pub end_op: usize,
}
```

- [ ] **Step 2: Add unit tests**

Test with simple control flow patterns:
- Straight-line code → single block
- If/else → 3 blocks (entry, true, false) + join block
- Simple loop → loop header with back edge
- Nested loop → correct nesting depth

- [ ] **Step 3: Verify and commit**

Run: `cargo test -p molt-backend --lib -- tir::cfg`
Commit: `git commit -m "feat(titan-p1): add CFG extraction from SimpleIR"`

---

### Task A3: SSA Conversion

**Files:**
- Create: `runtime/molt-backend/src/tir/ssa.rs`

Convert the CFG into SSA form with block arguments (MLIR-style, no phi nodes).

- [ ] **Step 1: Implement SSA construction**

Algorithm: iterated dominance frontier for block argument placement, then variable renaming via dominator tree walk.

```rust
/// Convert a CFG with variable assignments into SSA form.
/// Variables at join points become block arguments (MLIR-style).
pub fn convert_to_ssa(cfg: &CFG, ops: &[OpIR]) -> Vec<TirBlock> {
    // 1. Compute dominance frontiers
    // 2. For each variable assigned in a block, insert block arguments at DF blocks
    // 3. Walk the dominator tree, renaming variables to fresh ValueIds
    // 4. Thread block arguments through branch terminators
}
```

- [ ] **Step 2: Add unit tests**

- Simple assignment: `x = 1; y = x + 1` → two SSA values, no block args
- Join point: `if c: x = 1 else: x = 2; use(x)` → block arg at join for x
- Loop: `x = 0; while x < 10: x = x + 1` → block arg at loop header for x

- [ ] **Step 3: Verify and commit**

Run: `cargo test -p molt-backend --lib -- tir::ssa`
Commit: `git commit -m "feat(titan-p1): add SSA conversion with MLIR-style block arguments"`

---

### Task A4: SimpleIR → TIR Construction

**Files:**
- Create: `runtime/molt-backend/src/tir/lower_from_simple.rs`

The full pipeline: SimpleIR → CFG → SSA → typed TIR.

- [ ] **Step 1: Implement the lowering pipeline**

```rust
/// Convert a SimpleIR function into TIR.
pub fn lower_simple_to_tir(ir: &FunctionIR) -> TirFunction {
    // 1. Extract CFG from linear op stream
    let cfg = extract_cfg(&ir.ops);
    // 2. Convert to SSA with block arguments
    let ssa_blocks = convert_to_ssa(&cfg, &ir.ops);
    // 3. Map SimpleIR ops to TIR ops with dialect assignment
    let tir_blocks = map_ops_to_tir(&ssa_blocks, &ir.ops);
    // 4. Assign initial types (DynBox for most, concrete for fast_int/fast_float)
    let typed_blocks = assign_initial_types(tir_blocks, &ir.ops);

    TirFunction {
        name: ir.name.clone(),
        blocks: typed_blocks,
        entry_block: BlockId(0),
        ..
    }
}
```

Each SimpleIR op maps to a TIR op. Read `ir.rs` to understand all `OpIR.kind` variants and map them to `OpCode` values.

- [ ] **Step 2: Add integration test**

Test with a real SimpleIR function (e.g., from bench_sum.py compilation). Verify the TIR output has correct block structure, SSA form, and initial types.

- [ ] **Step 3: Verify and commit**

Commit: `git commit -m "feat(titan-p1): add SimpleIR → TIR construction pipeline"`

---

### Task A5: Type Refinement Pass

**Files:**
- Create: `runtime/molt-backend/src/tir/type_refine.rs`

Forward dataflow analysis that refines types from DynBox to concrete types.

- [ ] **Step 1: Implement type refinement**

```rust
/// Refine types in a TIR function using available type information.
/// Sources: explicit annotations, operation inference, assignment inference.
pub fn refine_types(func: &mut TirFunction) {
    // Iterate to fixpoint (max 20 rounds)
    for _ in 0..20 {
        let changed = false;
        for block in &mut func.blocks {
            for op in &mut block.ops {
                changed |= refine_op_types(op);
            }
        }
        if !changed { break; }
    }
}

fn refine_op_types(op: &mut TirOp) -> bool {
    match op.opcode {
        OpCode::Add => {
            // If both operands are I64, result is I64
            if op.operands[0].ty == TirType::I64 && op.operands[1].ty == TirType::I64 {
                if op.results[0].ty != TirType::I64 {
                    op.results[0].ty = TirType::I64;
                    return true;
                }
            }
            // I64 + F64 → F64 (Python promotion)
            // etc.
            false
        }
        OpCode::Const => {
            // Constants have known types
            // ...
            false
        }
        _ => false,
    }
}
```

- [ ] **Step 2: Add tests and commit**

Commit: `git commit -m "feat(titan-p1): add type refinement pass for TIR"`

---

### Task A6: TIR → SimpleIR Back-Conversion

**Files:**
- Create: `runtime/molt-backend/src/tir/lower_to_simple.rs`

Convert TIR back to SimpleIR so existing Cranelift/WASM backends can consume it. This is the bridge that lets us use TIR optimizations immediately without rewriting backends.

- [ ] **Step 1: Implement back-conversion**

```rust
/// Convert a TIR function back to SimpleIR ops.
/// This allows existing backends (Cranelift, WASM) to consume TIR-optimized code.
pub fn lower_tir_to_simple(func: &TirFunction) -> Vec<OpIR> {
    // 1. Linearize blocks in reverse-postorder
    // 2. Convert block arguments back to variable assignments
    // 3. Map TIR ops back to SimpleIR ops
    // 4. Reconstruct control flow markers (if/else/loop_start/loop_end)
}
```

- [ ] **Step 2: Add round-trip test**

SimpleIR → TIR → SimpleIR → execute. Verify output matches original.

- [ ] **Step 3: Commit**

Commit: `git commit -m "feat(titan-p1): add TIR → SimpleIR back-conversion for existing backends"`

---

### Task A7: TIR Pretty-Printer and Verifier

**Files:**
- Create: `runtime/molt-backend/src/tir/printer.rs`, `verify.rs`

- [ ] **Step 1: Implement TIR printer**

Triggered by `TIR_DUMP=1` environment variable. Outputs human-readable TIR matching MLIR-like syntax:

```
func @add(%0: i64, %1: i64) -> i64 {
  ^bb0(%0: i64, %1: i64):
    %2 = molt.add %0, %1 : i64
    return %2
}
```

- [ ] **Step 2: Implement TIR verifier**

Checks SSA invariants (every use dominated by def), type consistency, terminator completeness, block argument matching.

- [ ] **Step 3: Commit**

Commit: `git commit -m "feat(titan-p1): add TIR pretty-printer and SSA/type verifier"`

---

## Track B: LLVM Backend Scaffold

### Task B1: Inkwell Integration

**Files:**
- Modify: `runtime/molt-backend/Cargo.toml`
- Create: `runtime/molt-backend/src/llvm_backend/mod.rs`

- [ ] **Step 1: Add inkwell dependency**

In `runtime/molt-backend/Cargo.toml`, add under `[dependencies]`:
```toml
[features]
llvm = ["inkwell"]

[dependencies]
inkwell = { version = "0.5", features = ["llvm18-0"], optional = true }
```

This makes LLVM support optional — `cargo build` works without LLVM, `cargo build --features llvm` enables it.

- [ ] **Step 2: Create llvm_backend module skeleton**

Create `runtime/molt-backend/src/llvm_backend/mod.rs`:
```rust
//! LLVM backend for release-mode maximum optimization.
//! Requires: cargo build --features llvm

#[cfg(feature = "llvm")]
pub mod lowering;
#[cfg(feature = "llvm")]
pub mod runtime_imports;
#[cfg(feature = "llvm")]
pub mod types;

#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;

#[cfg(feature = "llvm")]
pub struct LlvmBackend<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        Self { context, module }
    }
}
```

- [ ] **Step 3: Register module and verify**

Add `#[cfg(feature = "llvm")] pub mod llvm_backend;` to `runtime/molt-backend/src/lib.rs`.

Run: `cargo check -p molt-backend` (without LLVM feature — should still compile)
Run: `cargo check -p molt-backend --features llvm` (if LLVM 18 is installed)

- [ ] **Step 4: Commit**

Commit: `git commit -m "feat(titan-p1): add inkwell/LLVM 18 integration scaffold (feature-gated)"`

---

### Task B2: SimpleIR → LLVM IR Lowering

**Files:**
- Create: `runtime/molt-backend/src/llvm_backend/lowering.rs`
- Create: `runtime/molt-backend/src/llvm_backend/runtime_imports.rs`
- Create: `runtime/molt-backend/src/llvm_backend/types.rs`

- [ ] **Step 1: Implement type mapping**

Create `types.rs`:
```rust
use inkwell::types::{BasicTypeEnum, IntType, FloatType};

/// Map TIR/SimpleIR types to LLVM types.
/// For now: everything is i64 (NaN-boxed). Type specialization comes in Phase 2.
pub fn value_type<'ctx>(context: &'ctx Context) -> IntType<'ctx> {
    context.i64_type()  // All Molt values are NaN-boxed i64
}

pub fn bool_type<'ctx>(context: &'ctx Context) -> IntType<'ctx> {
    context.bool_type()
}
```

- [ ] **Step 2: Implement runtime function declarations**

Create `runtime_imports.rs`:
```rust
/// Declare all runtime functions that compiled code calls into.
/// These are resolved at link time against molt-runtime.
pub fn declare_runtime_functions(module: &Module) {
    let i64_type = module.get_context().i64_type();
    let void_type = module.get_context().void_type();

    // Core operations
    module.add_function("molt_add", i64_type.fn_type(&[i64_type.into(), i64_type.into()], false), None);
    module.add_function("molt_sub", i64_type.fn_type(&[i64_type.into(), i64_type.into()], false), None);
    // ... all arithmetic, comparison, call, alloc, etc.

    // Object model
    module.add_function("molt_alloc", i64_type.fn_type(&[i64_type.into()], false), None);
    module.add_function("molt_inc_ref", void_type.fn_type(&[i64_type.into()], false), None);
    module.add_function("molt_dec_ref", void_type.fn_type(&[i64_type.into()], false), None);

    // Attribute access (with IC)
    module.add_function("molt_getattr_ic", i64_type.fn_type(&[i64_type.into(), i64_type.into(), i64_type.into(), i64_type.into()], false), None);
}
```

- [ ] **Step 3: Implement basic lowering**

Create `lowering.rs` — convert SimpleIR ops to LLVM IR:
```rust
pub fn lower_function(backend: &LlvmBackend, ir: &FunctionIR) -> FunctionValue {
    let fn_type = i64_type.fn_type(&param_types, false);
    let function = backend.module.add_function(&ir.name, fn_type, None);
    let entry = backend.context.append_basic_block(function, "entry");
    let builder = backend.context.create_builder();
    builder.position_at_end(entry);

    for op in &ir.ops {
        match op.kind.as_str() {
            "add" => {
                let lhs = get_value(op.args[0]);
                let rhs = get_value(op.args[1]);
                let result = builder.build_call(
                    backend.module.get_function("molt_add").unwrap(),
                    &[lhs.into(), rhs.into()],
                    "add_result"
                );
                store_value(op.out, result);
            }
            "return" => {
                let val = get_value(op.args[0]);
                builder.build_return(Some(&val));
            }
            // ... map each SimpleIR op kind to LLVM IR
        }
    }

    function
}
```

Start with only the ops needed for `bench_sum.py` — arithmetic, comparison, branch, return, function call. Add more ops incrementally.

- [ ] **Step 4: End-to-end test with bench_sum.py**

Compile bench_sum.py's SimpleIR through the LLVM backend, write to object file, link, and run. Verify output matches CPython.

This is the milestone: first Python program compiled through LLVM.

- [ ] **Step 5: Commit**

Commit: `git commit -m "feat(titan-p1): LLVM backend compiles bench_sum.py end-to-end"`

---

## Track C: Allocator Improvements

### Task C1: Switch to mimalloc

**Files:**
- Modify: `runtime/molt-runtime/Cargo.toml`
- Modify: `runtime/molt-runtime/src/lib.rs`

- [ ] **Step 1: Add mimalloc dependency**

In `runtime/molt-runtime/Cargo.toml`:
```toml
[dependencies]
mimalloc = { version = "0.1", default-features = false }
```

- [ ] **Step 2: Set as global allocator**

In `runtime/molt-runtime/src/lib.rs`, add at the top:
```rust
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

(Only on native targets — WASM has its own allocator.)

- [ ] **Step 3: Run benchmarks to measure impact**

Run: `cargo bench` or `python3 tools/bench.py --benchmarks bench_gc_pressure,bench_sum --samples 10`
Expected: 5-10% improvement on allocation-heavy benchmarks.

- [ ] **Step 4: Commit**

Commit: `git commit -m "feat(titan-p1): switch to mimalloc global allocator (native only)"`

---

### Task C2: Nursery Bump Allocator

**Files:**
- Create: `runtime/molt-runtime/src/object/nursery.rs`
- Modify: `runtime/molt-runtime/src/object/mod.rs`

- [ ] **Step 1: Implement nursery allocator with write barrier**

Create `nursery.rs`:
```rust
//! Per-function nursery for short-lived objects.
//! Bump allocation: 2 instructions per alloc (compare + increment).
//! Reset: 1 instruction (reset cursor).
//! Write barrier: promotes nursery objects stored into heap containers.

const NURSERY_SIZE: usize = 64 * 1024; // 64KB per nursery

pub struct Nursery {
    base: *mut u8,
    cursor: *mut u8,
    limit: *mut u8,
    promoted: Vec<*mut u8>,
}

impl Nursery {
    pub fn new() -> Self { /* mmap or Vec<u8> allocation */ }

    #[inline(always)]
    pub fn alloc(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        let aligned = (self.cursor as usize + align - 1) & !(align - 1);
        let new_cursor = aligned + size;
        if new_cursor <= self.limit as usize {
            let ptr = aligned as *mut u8;
            self.cursor = new_cursor as *mut u8;
            Some(ptr)
        } else {
            None // Nursery full — caller falls back to heap
        }
    }

    #[inline(always)]
    pub fn is_nursery_ptr(&self, ptr: *const u8) -> bool {
        let addr = ptr as usize;
        addr >= self.base as usize && addr < self.limit as usize
    }

    /// Write barrier: if storing a nursery pointer into a heap object, promote it.
    #[inline(always)]
    pub fn write_barrier(&mut self, target: *mut u8, value: *mut u8) {
        if self.is_nursery_ptr(value) && !self.is_nursery_ptr(target) {
            self.promote(value);
        }
    }

    fn promote(&mut self, ptr: *mut u8) {
        // Copy object to heap, install forwarding pointer
        // Add to promoted list
    }

    pub fn reset(&mut self) {
        self.cursor = self.base;
        self.promoted.clear();
    }
}
```

- [ ] **Step 2: Wire into allocation path (conservative)**

For Phase 1, only use the nursery for known-temporary allocations:
- Tuple unpacking intermediates
- String formatting intermediates
- Comprehension accumulation

The caller explicitly opts in: `nursery.alloc()` instead of `molt_alloc()`.

- [ ] **Step 3: Add tests**

Test allocation, reset, write barrier promotion, nursery-full fallback.

- [ ] **Step 4: Commit**

Commit: `git commit -m "feat(titan-p1): add nursery bump allocator with write barrier"`

---

## Task Dependencies

```
Track A (TIR):
  A1 (data structures) → A2 (CFG) → A3 (SSA) → A4 (lowering) → A5 (type refine) → A6 (back-conv) → A7 (printer/verifier)

Track B (LLVM):
  B1 (inkwell) → B2 (lowering + e2e test)

Track C (Allocator):
  C1 (mimalloc) → C2 (nursery)

All tracks are independent and can run in parallel.
A6 must complete before TIR can be used by existing backends.
B2 must complete before Phase 2 LLVM passes can be added.
```

## Success Criteria

- [ ] TIR_DUMP=1 produces readable output for bench_sum.py
- [ ] TIR round-trip (SimpleIR → TIR → SimpleIR) is semantically identical
- [ ] Type refinement resolves ≥60% of values on annotated code
- [ ] LLVM backend compiles and runs bench_sum.py correctly (output matches CPython)
- [ ] mimalloc shows measurable allocation improvement
- [ ] Nursery handles known-temporary allocations safely (ASAN clean)
- [ ] No regressions on any Phase 0 benchmark
