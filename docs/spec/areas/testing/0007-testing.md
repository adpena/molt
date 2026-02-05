# Molt Testing & Verification Strategy

See `README.md` for quick-start testing commands and CI parity job summaries.

## Version Policy
Molt targets **Python 3.12+** semantics only. When 3.12/3.13/3.14 diverge,
document the chosen target in specs/tests and keep the differential suite aligned.

## 1. Differential Testing: The `molt-diff` Harness
`molt-diff` is a specialized tool that ensures Molt semantics match CPython. The current harness lives in `tests/molt_diff.py` and builds + runs binaries via `uv run --python 3.12 python3 -m molt.cli build`.

### 1.0 Performance + Memory Controls
- **Parallelism**: auto-selected based on CPU and available memory (default budget: 2 GB/worker).
  - Override with `--jobs <n>` or `MOLT_DIFF_MAX_JOBS=<n>`.
  - Tune memory budget with `MOLT_DIFF_MEM_PER_JOB_GB=<n>` or `MOLT_DIFF_MEM_AVAILABLE_GB=<n>`.
- **Memory cap**: enforced per process by default (10 GB). Disable with `MOLT_DIFF_RLIMIT_GB=0`.
- **OOM retry**: OOM failures are retried once with `--jobs 1` (disable via `--no-retry-oom` or `MOLT_DIFF_RETRY_OOM=0`).
- **Warm cache**: `--warm-cache` or `MOLT_DIFF_WARM_CACHE=1` prebuilds all tests to seed `MOLT_CACHE`.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `--failures-output` or `MOLT_DIFF_FAILURES`).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) includes run metadata and RSS aggregates when enabled.
- **Memory report**: run `python3 tools/diff_memory_report.py --run-id <id>` to list top RSS offenders (uses `rss_metrics.jsonl`).
- **Top offenders printout**: when `MOLT_DIFF_MEASURE_RSS=1`, the harness prints top 5 RSS offenders at the end (override with `MOLT_DIFF_RSS_TOP=<n>`).
- **Summary top list**: `summary.json` includes `rss.top` with the top offenders (file + build/run RSS).

### 1.1 Methodology
1.  **Input**: A Python source file `test_case.py`.
2.  **Execution**:
    - Run `uv run --python 3.12 python3 test_case.py` -> Capture `stdout`, `stderr`, `exit_code`.
    - Run `uv run --python 3.12 python3 tests/molt_diff.py test_case.py` -> Build with Molt, run the binary, capture outputs.
3.  **Comparison**: Assert that all captured outputs are identical.

### 1.2 State Snapshoting
For complex tests, we use `molt.dump_state()` to export a JSON representation of global variables and compare the JSON output between runs.

### 1.3 Curated Parity Suite
Basic parity cases live in `tests/differential/basic/`. Run the full suite via:
```
uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic
```
The verified subset contract uses `tools/verified_subset.py` to validate and run
these suites in CI.

### 1.4 Differential coverage reporting
Generate metadata coverage summaries from `# MOLT_META` headers:
```
uv run --python 3.12 python3 tools/diff_coverage.py
```
The report is written to `tests/differential/COVERAGE_REPORT.md` by default.

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
- **Python**: `uv run --python 3.12 pytest` (unit and integration tests under `tests/`).
- **Differential**: run `uv run --python 3.12 python3 tests/molt_diff.py <case.py>` for curated parity cases (expand over time).
- **Benchmarks**: `tools/bench.py` for local validation; add CI regression gates as they stabilize.
