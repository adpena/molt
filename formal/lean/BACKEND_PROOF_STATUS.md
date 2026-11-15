# Backend Proof Status

Generated: 2026-03-16
Updated: 2026-11-12 (the Luau and Rust-transpile targets and their formal models were deleted; git history holds them)

## Sorry Count by File

| File | Sorrys | Axioms | Status |
|------|--------|--------|--------|
| `Backend/CrossBackend.lean` | 0 | 0 | Not built: stale against the current TIR syntax (HF-63) |
| `Backend/BackendDeterminism.lean` | 0 | 0 | Not built: imports a stale module (HF-63) |
| `Backend/TargetIndependence.lean` | 0 | 0 | Not built: imports a stale module (HF-63) |
| `Runtime/WasmNativeCorrect.lean` | 0 | 0 | Complete |
| `Runtime/WasmABI.lean` | 0 | 0 | Complete |
| `Determinism/CrossPlatform.lean` | 0 | 1 | Complete (1 axiom) |

**Total sorrys across all 6 files: 0**

Status checked 2026-10-22 against Lean 4.35.0 and the Lake roots in `lakefile.lean`: only files the default target builds are proven. The cross-backend theorems sit outside the roots and no longer compile against the current TIR syntax; HF-63 tracks them.
**Total axioms across backend files: 1** (`ieee754_basic_ops_deterministic` in CrossPlatform.lean)

## Codebase-Wide Axiom Count

The full Lean codebase contains **68 trust axioms** across 6 files. None are in the
backend proof files themselves (the 1 axiom above is in the Determinism layer).
See `AXIOM_INVENTORY.md` for the complete enumeration.

## What Is Proven

### Cross-Backend Equivalence (CrossBackend.lean)
- `all_backends_equiv` -- the backends (Native, WASM) produce identical observable behavior
- The WASM-Native pairwise equivalence
- Integration with optimization pipeline: `pipeline_backend_equiv`
- `optimized_equiv_unoptimized_any_backend` -- optimization preserves observable behavior on any backend
- Observable behavior respects behavioral equivalence

### Backend Determinism (BackendDeterminism.lean)
- Per-backend emission determinism (Native, WASM)
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

2. **Backend emission link**: the end-to-end chain no longer reaches emitted code. Its only backend link was the Luau `emitExpr_correct`; `WasmCorrect.lean` proves only the `val` and `var` cases, so a full WASM `emitExpr_correct` over `WasmEmit` would restore it (HF-63).

3. **Heap operations**: String content, list operations, and heap-allocated objects are modeled abstractly (via `StringRepr` with length+hash). Full byte-level heap equivalence is not formalized.

4. **I/O**: Observable behavior includes an `outputTrace` field, but the actual I/O semantics (print statement side effects) are not modeled beyond the structure.

5. **Integer overflow**: The proofs work with Lean's arbitrary-precision `Int`, so overflow behavior outside the inline-int range is not captured. The NaN-boxed runtime uses a 32-bit payload for integers, and the `INT_MASK` proofs validate the boxing, but full range analysis is not done.
