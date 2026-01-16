# Benchmarking & Performance Gates

Molt is performance-obsessed. Every major change must be validated against our benchmark suite.

## Running Benchmarks

We use `tools/bench.py` for native and `tools/bench_wasm.py` for WASM.

```bash
# Basic run
uv run python3 tools/bench.py

# Record results to JSON (standard for PRs)
uv run python3 tools/bench.py --json-out bench/results/my_change.json

# Comparison vs CPython
uv run python3 tools/bench.py --compare cpython
```

## Combined Native + WASM Report

After writing `bench/results/bench.json` and `bench/results/bench_wasm.json`, generate the
combined report:

```bash
uv run --python 3.14 python3 tools/bench_report.py
```

This writes `docs/benchmarks/bench_summary.md` by default. Commit the report alongside
the JSON results to keep native and WASM performance tracking aligned.
Add `--update-readme` to refresh the Performance & Comparisons block in `README.md`.

## Performance Gates

We enforce strict "Performance Gates" in CI. If a PR causes a regression beyond these limits, it will be blocked.

| Category | Gate (Max Regression) | Examples |
| --- | --- | --- |
| Vector Reductions | 5% | `sum`, `min`, `max` on lists |
| String Kernels | 7% | `find`, `split`, `replace` |
| Matrix/Buffer | 5% | `matmul`, buffer access |
| General Loops | 10% | CSV parsing, deep loops |

## How to Interpret Results

- **Speedup (x.xx)**: Molt is X times faster than CPython. (e.g., 10.0x = Molt is 10x faster).
- **Regression (< 1.0x)**: Molt is slower than CPython. This is generally unacceptable for Tier 0 constructs.
- **Super Bench (`--super`)**: Runs 10 samples and calculates variance. Use this for final release validation or when results are noisy.

## Profiles

Use `molt profile <script.py>` to generate flamegraphs and identify bottlenecks in the compiler or runtime.

## Optimization Plan

Long-term or complex optimizations that require research are tracked in `OPTIMIZATIONS_PLAN.md`. If your change is a major architectural shift, please update that plan first.
