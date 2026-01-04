# Molt Testing & Verification Strategy

See `README.md` for quick-start testing commands and CI parity job summaries.

## 1. Differential Testing: The `molt-diff` Harness
`molt-diff` is a specialized tool that ensures Molt semantics match CPython. The current harness lives in `tests/molt_diff.py` and builds + runs binaries via `python3 -m molt.cli build`.

### 1.1 Methodology
1.  **Input**: A Python source file `test_case.py`.
2.  **Execution**:
    - Run `python3 test_case.py` -> Capture `stdout`, `stderr`, `exit_code`.
    - Run `python tests/molt_diff.py test_case.py` -> Build with Molt, run the binary, capture outputs.
3.  **Comparison**: Assert that all captured outputs are identical.

### 1.2 State Snapshoting
For complex tests, we use `molt.dump_state()` to export a JSON representation of global variables and compare the JSON output between runs.

### 1.3 Curated Parity Suite
Basic parity cases live in `tests/differential/basic/`. Run the full suite via:
```
python tests/molt_diff.py tests/differential/basic
```

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
- **Rust**: `cargo test` (runtime + core unit tests).
- **Python**: `pytest` (unit and integration tests under `tests/`).
- **Differential**: run `python tests/molt_diff.py <case.py>` for curated parity cases (expand over time).
- **Benchmarks**: `tools/bench.py` for local validation; add CI regression gates as they stabilize.
