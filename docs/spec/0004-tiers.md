# Molt Tiers & Soundness

## 1. Tier 0: "Frozen Python"
This is the "Gold Standard" for performance.
- **Constraints**:
    - No `eval()` or `exec()`.
    - No monkeypatching of classes/modules after initialization.
    - No dynamic `__bases__` changes.
    - All imports must be statically resolvable.
- **Benefits**:
    - Full structification of objects.
    - Static dispatch for most method calls.
    - Maximum inlining.

## 2. Tier 1: "Guarded Python"
Allows most standard Python dynamism with a performance penalty.
- **Mechanism**:
    - **Guards**: Checks inserted at every dynamic site (e.g., `getattr`).
    - **Specialization**: If a site is observed to be monomorphic (always the same type), Molt JIT-compiles (or AOT-specializes) a fast path.
    - **Deoptimization**: If a guard fails, the execution falls back to a slower, generic interpreter loop or a "slow island".

## 3. Soundness Model
Molt ensures correctness through:
- **Static Verification**: Type and shape inference must prove the safety of Tier 0 optimizations.
- **Runtime Assertions**: In debug builds, every assumption made by the compiler is checked.
- **Differential Testing**: The Molt test runner executes the same code in CPython and Molt, comparing the results (including side effects where possible).
