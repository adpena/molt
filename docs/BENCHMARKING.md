# Benchmarking & Performance Gates

Molt is performance-obsessed. Every major change must be validated against our benchmark suite.

## Running Benchmarks

We use `tools/bench.py` for native and `tools/bench_wasm.py` for WASM.
To exercise single-module linking, add `--linked` (requires `wasm-ld` and
`wasm-tools`).
For performance parity work, prefer linked WASM artifacts (`tools/bench_wasm.py --linked`)
and use the linked runner path by default.
If you build standalone WASM artifacts for perf validation, use
`uv run --python 3.12 python3 -m molt.cli build --target wasm --require-linked`
to ensure only linked output is produced.
Use `tools/bench_wasm.py --require-linked` to fail fast when linking is unavailable.

```bash
# Basic run
uv run --python 3.14 python3 tools/bench.py

# One-off script (CLI wrapper or direct harness)
molt bench --script path/to/script.py
uv run --python 3.14 python3 tools/bench.py --script path/to/script.py

# Record results to JSON (standard for PRs)
uv run --python 3.14 python3 tools/bench.py --json-out bench/results/my_change.json

# Increase warmup runs (default: 1, or 0 for --smoke)
uv run --python 3.14 python3 tools/bench.py --warmup 2

# Comparison vs CPython
uv run --python 3.14 python3 tools/bench.py --compare cpython
```

## Native Baselines (Optional)

`tools/bench.py` can compare Molt against optional native baselines using the
same benchmark scripts (no Depyler-specific tests):

- **PyPy**: auto-probed via `uv run --python pypy@3.11` (skipped if unavailable).
- **Cython/Numba**: install with `uv sync --group bench --python 3.12` (also included in the `dev` group).
- **Codon**: install `codon` and ensure it is on PATH.
- **Depyler**: install via `cargo install depyler`.

Disable any baseline with `--no-pypy`, `--no-cython`, `--no-numba`, `--no-codon`,
or `--no-depyler`.

## Combined Native + WASM Report

After writing `bench/results/bench.json` and `bench/results/bench_wasm.json` (or
linked output when using `--linked`), generate the
combined report:

```bash
uv run --python 3.14 python3 tools/bench_report.py
```

This writes `docs/benchmarks/bench_summary.md` by default. Commit the report alongside
the JSON results to keep native and WASM performance tracking aligned.
Add `--update-readme` to refresh the Performance & Comparisons block in `README.md`.

## Binary Size & Cold-Start (Optional)

See `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` for required metrics.
Common tools:
- `cargo bloat -p molt-runtime --release`
- `cargo llvm-lines -p molt-runtime`
- `llvm-size <binary>` (native size)
- `twiggy top output.wasm` (WASM size drivers)
- `wasm-opt -Oz -o output.opt.wasm output.wasm`
- `wasm-tools strip output.opt.wasm -o output.stripped.wasm`
- `gzip -k output.wasm` / `brotli -f output.wasm` (compressed size)

## Performance Gates

We enforce strict "Performance Gates" in CI. If a PR causes a regression beyond these limits, it will be blocked.

| Category | Gate (Max Regression) | Examples |
| --- | --- | --- |
| Vector Reductions | 5% | `sum`, `min`, `max` on lists |
| String Kernels | 7% | `find`, `split`, `replace` |
| Matrix/Buffer | 5% | `matmul`, buffer access |
| General Loops | 10% | CSV parsing, deep loops |

## Lock-Sensitive Benchmarks

When changing the GIL, pointer registry, handle resolution, scheduler locks, or
other runtime synchronization, run a targeted subset in addition to the full
suite. Prioritize workloads that stress attribute access/descriptor dispatch,
struct/shape access, container ops, deep loops, and channel throughput.
Validate native + WASM parity for the same cases.

## How to Interpret Results

- **Speedup (x.xx)**: Molt is X times faster than CPython. (e.g., 10.0x = Molt is 10x faster).
- **Regression (< 1.0x)**: Molt is slower than CPython. This is generally unacceptable for Tier 0 constructs.
- **Super Bench (`--super`)**: Runs 10 samples and calculates variance. Use this for final release validation or when results are noisy.
- **Molt build vs run time**: `molt_build_s` captures compile time; `molt_time_s` is run time only for fair runtime comparisons.
- **WASM build vs run time**: `molt_wasm_build_s` captures wasm compile time; `molt_wasm_time_s` is run time only.

## Profiles

Use `molt profile <script.py>` to generate flamegraphs and identify bottlenecks in the compiler or runtime.

## Optimization Plan

Long-term or complex optimizations that require research are tracked in `OPTIMIZATIONS_PLAN.md`. If your change is a major architectural shift, please update that plan first.
