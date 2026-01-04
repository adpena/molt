# Molt Architecture Spec

## 1. Overview
Molt is a tiered AOT compiler for Python. It transforms a statically-analyzable subset of Python (Tier 0) into highly optimized native machine code using a Rust-based runtime and the Cranelift backend.

## 2. IR Stack & Invariants

### 2.1 Molt HIR (High-level IR)
- **Source**: Python AST (via `ruff-ast` for speed).
- **Form**: Tree-based, but desugared.
- **Invariants**:
    - No `with` statements (lowered to `try/finally`).
    - No `for` loops (lowered to `while` with explicit iterators).
    - All imports are resolved and flattened into a module graph.

### 2.2 Molt TIR (Typed IR)
- **Form**: SSA-based CFG.
- **Typing**: Every `Value` has a `MoltType`. Types can be primitives (`Int`, `Float`, `Bool`), objects (`Class(User)`), or refined unions (`Union(Int, None)`).
- **Invariants**:
    - **Soundness**: If a value is typed as `Int`, it MUST be a 64-bit integer at runtime or the compiler must insert a guard.
    - **Single Definition**: Every variable is defined exactly once.
- **Passes**:
    - **Type Inference**: Global fixed-point iteration.
    - **Monomorphization**: Clones functions for specific call-site types to eliminate dynamic dispatch.
    - **Constant Folding**: Aggressive folding of Python constants and builtins.

### 2.3 Molt LIR (Low-level IR)
- **Form**: SSA-based CFG, close to machine abstractions.
- **Abstraction**: Operates on "Slots" and "Registers". Handles explicit memory management operations.
- **Invariants**:
    - All object allocations are explicit `Alloc` instructions.
    - Reference counting increments/decrements are explicit (before optimization).
- **Passes**:
    - **Escape Analysis**: Promotes heap allocations to stack or registers if the object doesn't escape the function.
    - **RC Elision**: Removes redundant `IncRef`/`DecRef` pairs.
    - **Structification**: Lowers class access (`obj.field`) to fixed-offset memory loads (`load [reg + 16]`).

## 3. The Compilation Pipeline

1.  **Discovery Phase**:
    - Walk the entry point and all reachable modules.
    - Build a "Call Graph" and "Type Graph".
2.  **Invariant Mining**:
    - Identify "Stable Classes": Classes whose `__dict__` is never modified dynamically and whose bases are fixed.
    - Identify "Pure Functions": Functions with no side effects, candidates for aggressive folding.
3.  **Specialization**:
    - For every call site, decide whether to use:
        - **Static Dispatch**: If the target is known and stable.
        - **Guarded Dispatch**: If the target is likely but not certain.
        - **Dynamic Dispatch**: Fallback to `molt_dispatch` (Tier 1).
4.  **Lowering & Codegen**:
    - Emit Cranelift IR.
    - Link with `molt-runtime` (static library).

## 4. Invariant Mining Details
Molt relies on "Whole Program Knowledge".
- **Closed World Assumption**: By default, Molt assumes it sees the entire application. Any dynamic interaction from "outside" (e.g., calling into a non-Molt compiled SO) must be explicitly declared.
- **Shape Inference**: For dictionaries used as records, Molt infers a "Shape". If a dict always has keys `{"a", "b"}`, it is lowered to a struct-like layout.

## 5. Backend: Cranelift vs MLIR
- **Cranelift**: Primary backend for MVP. Fast, Rust-native, supports AOT and JIT.
- **MLIR**: Planned for Milestone 3. Will be used for high-level "Data Pipeline" optimizations (e.g., fusing Map/Reduce kernels) before lowering to LLVM or Cranelift.
