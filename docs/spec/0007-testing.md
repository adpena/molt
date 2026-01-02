# Molt Testing & Verification Strategy

## 1. Differential Testing: The `molt-diff` Harness
`molt-diff` is a specialized tool that ensures Molt semantics match CPython.

### 1.1 Methodology
1.  **Input**: A Python source file `test_case.py`.
2.  **Execution**:
    - Run `python3 test_case.py` -> Capture `stdout`, `stderr`, `exit_code`.
    - Run `molt run test_case.py` -> Capture `stdout`, `stderr`, `exit_code`.
3.  **Comparison**: Assert that all captured outputs are identical.

### 1.2 State Snapshoting
For complex tests, we use `molt.dump_state()` to export a JSON representation of global variables and compare the JSON output between runs.

## 2. Automated Test Generation (Hypothesis)
We use `Hypothesis` to generate random Python ASTs that fall within the Molt Tier 0 subset.
- **Rules**:
    - Only use supported primitives.
    - No `exec`/`eval`.
    - Valid scope resolution.
- **Goal**: Find edge cases in type inference or codegen that cause divergence from CPython.

## 3. Metamorphic Testing
To verify the optimizer:
1.  Take a program `P`.
2.  Apply a semantics-preserving transformation `T` (e.g., `inline_function`, `rename_variable`) to get `P'`.
3.  Ensure `Molt(P)` and `Molt(P')` produce the same output and similar performance characteristics.

## 4. Guard/Deopt Validation
To test Tier 1:
- Create "Bait" tests that trigger deoptimization (e.g., passing a `float` to a function that was specialized for `int`).
- Verify that the runtime correctly switches to the slow path without crashing or losing state.

## 5. Continuous Integration Gates
- **Rust**: `cargo test` (unit tests for IR and Runtime).
- **Python**: `pytest tests/differential` (the `molt-diff` suite).
- **Benchmarks**: `molt bench --check-regressions`.
