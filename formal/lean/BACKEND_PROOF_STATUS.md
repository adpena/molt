# Backend Proof Status

Generated: 2026-03-16
Updated: 2026-04-23 (Luau backend proof/code correspondence and `LuauCorrect.lean` sorry closure)

## Sorry Count by File

| File | Sorrys | Axioms | Status |
|------|--------|--------|--------|
| `Backend/LuauCorrect.lean` | 0 | 0 | Complete |
| `Backend/LuauTargetSemantics.lean` | 0 | 0 | Complete |
| `Backend/RustCorrect.lean` | 0 | 0 | Complete |
| `Backend/CrossBackend.lean` | 0 | 0 | Complete |
| `Backend/BackendDeterminism.lean` | 0 | 0 | Complete |
| `Backend/TargetIndependence.lean` | 0 | 0 | Complete |
| `Runtime/WasmNativeCorrect.lean` | 0 | 0 | Complete |
| `Runtime/WasmABI.lean` | 0 | 0 | Complete |
| `Determinism/CrossPlatform.lean` | 0 | 1 | Complete (1 axiom) |
| `Backend/RustSyntax.lean` | 0 | 0 | Complete |
| `Backend/RustSemantics.lean` | 0 | 0 | Complete |
| `Backend/RustEmit.lean` | 0 | 0 | Complete |

**Total sorrys across all 12 files: 0**
**Total axioms across backend files: 1** (`ieee754_basic_ops_deterministic` in CrossPlatform.lean)

## Codebase-Wide Axiom Count

The full Lean codebase contains **68 trust axioms** across 6 files. None are in the
backend proof files themselves (the 1 axiom above is in the Determinism layer).
See `AXIOM_INVENTORY.md` for the complete enumeration.

## New: LuauTargetSemantics.lean

Added to the backend inventory. This file provides a deep formalization of Luau target
semantics, extending the evaluation model with:

- Extended value model (closures, userdata, full table semantics)
- Luau-specific operations (# length, table.insert/remove, nil propagation)
- String semantics (immutable byte sequences, ASCII subset)
- Type coercion rules
- Python-Luau correspondence theorems for the Molt-supported subset

All theorems are sorry-free.

## What Is Proven

### Luau Backend (LuauCorrect.lean)
- Index adjustment correctness (0-based IR to 1-based Luau)
- Expression emission structural preservation (val, var, bin, un)
- Instruction emission produces exactly one local declaration
- Builtin mapping completeness (print, len, str, abs + unknown)
- Unpack calling convention structure
- Operator mapping totality (BinOp, UnOp)
- **Full semantic correctness**: `emitExpr_correct` -- structural induction proving that for every IR expression, if IR evaluation succeeds, Luau evaluation of the emitted expression succeeds with the corresponding value
- **Environment preservation**: `emitInstr_preserves_env` -- instruction emission maintains environment correspondence between IR and Luau
- Semantic index adjustment evaluation
- Operator-level semantic correspondence (add, sub, mul, mod, eq, lt, neg, not, abs)

### Luau Target Semantics (LuauTargetSemantics.lean)
- Extended Luau value model (base values, closures, userdata)
- Luau table semantics (array part, hash part, insert/remove)
- String immutability and byte-sequence semantics
- Type coercion rules (number-to-string, string-to-number, nil propagation)
- Python-Luau value correspondence for the Molt-supported subset

### Rust Backend (RustCorrect.lean)
- Environment correspondence (`RustEnvCorresponds`) with injectivity
- Empty and extended environment correspondence preservation
- Type mapping totality and faithfulness (int->i64, float->f64, bool->bool, str->String, none->unit)
- Copy type identification (int, float, bool are Copy)
- Expression emission structural preservation (val, var, bin, un)
- Instruction emission produces exactly one let binding
- Operator mapping totality (BinOp, UnOp)
- **Full semantic correctness**: `emitRustExpr_correct` -- structural induction, parallel to the Luau proof
- **Environment preservation**: `emitRustInstr_preserves_env`
- Builtin mapping completeness (print->println!, len->molt_len, abs->i64::abs)
- Value correspondence preserves int/float distinction (unlike Luau which conflates as number)
- SSA ownership safety: all bound values are accessible (no use-after-move)

### Cross-Backend Equivalence (CrossBackend.lean)
- `all_backends_equiv` -- all 4 backends (Native, WASM, Luau, Rust) produce identical observable behavior
- All 6 pairwise equivalences (Luau-Native, WASM-Native, Rust-Native, Luau-WASM, Rust-WASM, Luau-Rust)
- Integration with optimization pipeline: `pipeline_backend_equiv`
- `optimized_equiv_unoptimized_any_backend` -- optimization preserves observable behavior on any backend
- Observable behavior respects behavioral equivalence

### Backend Determinism (BackendDeterminism.lean)
- Per-backend emission determinism (all 4 backends)
- Observable behavior determinism (return value, exit code, output trace)
- Cross-compilation determinism (different hosts, same result)
- Full pipeline determinism (optimize + emit)
- Artifact-level determinism (no timestamps, content-based)
- Agreement for terminating, stuck, and divergent programs across backends

### Target Independence (TargetIndependence.lean)
- Type safety is target-independent
- Determinism is target-independent
- Termination is target-independent
- Memory safety is target-independent
- Value/exit code/output trace correspondence across all backends
- **Lift-once-use-everywhere meta-theorem**: any TIR-level property with a valid bridge automatically holds for all backends
- Pipeline composition with the lift theorem

### WASM/Native Correctness (WasmNativeCorrect.lean)
- Integer arithmetic operations (add, sub, mul, eq) are target-independent
- Tag preservation for all integer arithmetic ops
- NaN-boxing encode/decode is target-independent
- Boundary value validation (0+0, 1+1, 42+(-42), 10+20, -5+5, 100-42, 6*7, etc.)
- String operations are target-independent
- Memory layout agreement (identical 16-byte header, field offsets)
- Function call convention agreement (8-byte NaN-boxed args, left-to-right)
- Target-parameterized operations with universal agreement

### WASM ABI (WasmABI.lean)
- WASM value types are well-defined and disjoint (i32, i64, f32, f64)
- Molt NaN-boxed values fit in a single WASM i64
- WASM32 addresses fit in NaN-box pointer payload
- Object header field layout (refcount at 0, type tag at 8, disjoint, within header)
- Pointer boxing for WASM32 addresses (`boxWasm32Ptr_isPtr`)
- WASM well-typedness for produced values
- All NaN-boxing tags fit in i64
- ABI consistency summary theorem

### Cross-Platform Determinism (CrossPlatform.lean)
- NaN-boxing is platform-independent (always 64-bit)
- Integer operations are platform-independent
- Object layout is platform-independent
- Call convention is platform-independent
- IR is platform-independent by construction
- Expression evaluation and optimization pipeline are platform-independent

## What Is NOT Proven (and What Would Be Needed)

### Axioms in Scope
1. **`ieee754_basic_ops_deterministic`** (CrossPlatform.lean): asserts IEEE 754 conformance for basic float operations. This cannot be proven in Lean -- it is a hardware property validated by differential testing.

2. **Intrinsic contract axioms** (Runtime/IntrinsicContracts.lean): 61 axioms about Python builtin behavior (len, abs, bool, str, sorted, reversed, etc.). These model the runtime's behavior and are not in the backend proof files, but are used transitively.

### Gaps Between Formal Proofs and Real Codegen

1. **Cranelift codegen correctness**: The proofs assume that Cranelift correctly translates IR to native/WASM machine code. A full verification would require a verified compiler backend (like CompCert for C), which is far beyond scope.

2. **Rust compiler (rustc) correctness**: The Rust backend proofs establish source-to-source equivalence (MoltTIR -> Rust source). Correctness of the Rust compiler itself is assumed.

3. **Luau VM correctness**: Similarly, Luau emission correctness assumes the Luau VM correctly executes the emitted Luau source.

4. **Float division and power**: `emitBinOp` handles floordiv -> idiv and pow, but the Luau `idiv` (`//`) semantics are not yet modeled in `evalLuauBinOp`. The arithmetic correspondence is proven only for add, sub, mul, mod, eq, lt.

5. **Heap operations**: String content, list operations, and heap-allocated objects are modeled abstractly (via `StringRepr` with length+hash). Full byte-level heap equivalence is not formalized.

6. **I/O**: Observable behavior includes an `outputTrace` field, but the actual I/O semantics (print statement side effects) are not modeled beyond the structure.

7. **Integer overflow**: The proofs work with Lean's arbitrary-precision `Int`. The Rust backend emits `i64`, so overflow behavior for values outside the i64 range is not captured. The NaN-boxed runtime uses a 32-bit payload for integers, and the `INT_MASK` proofs validate the boxing, but full range analysis is not done.

## Connection Between Formal Proofs and Actual Rust Codegen

The formal model in `RustSyntax.lean` / `RustSemantics.lean` / `RustEmit.lean` / `RustCorrect.lean` captures:

- **RustSyntax.lean**: Defines the Rust AST subset used by the transpiler (`RustExpr`, `RustStmt`, `RustType`, `RustBinOp`, `RustUnOp`).
- **RustSemantics.lean**: Defines evaluation functions (`evalRustExpr`, `evalRustBinOp`, `evalRustUnOp`, `execRustStmt`, `execRustStmts`) and the value correspondence (`valueToRust`).
- **RustEmit.lean**: Defines the emission functions (`emitRustExpr`, `emitRustInstr`, `emitRustBinOp`, `emitRustUnOp`, `emitRustType`) and the Rust builtin mapping.
- **RustCorrect.lean**: Proves correctness of the above via structural induction.

The actual Rust codegen lives in the Molt compiler (Rust source). The formal model serves as a specification: if the real transpiler follows the same structural patterns as `emitRustExpr`/`emitRustInstr`, the proven properties hold. The connection is:

1. The formal model covers the core expression/instruction emission path.
2. The real transpiler handles additional complexity (control flow, function calls, imports, error handling) not captured in the model.
3. The proofs guarantee: for the modeled subset (scalar expressions, SSA let bindings, builtins), the transpilation is semantics-preserving.
4. The cross-backend equivalence proofs lift this to: the Rust backend produces the same observable behavior as Native/WASM/Luau for the modeled subset.
