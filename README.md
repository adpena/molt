# Molt

A research-grade project to compile a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM),
with strict reproducibility, rigorous testing, and staged compatibility.

> Molt = Python shedding its skin into native code.

## Capabilities (Current)

- **Tier 0 Structification**: Compiles typed Python classes to native structs with fixed-offset access.
- **Native Async**: Compiles `async/await` syntax (currently flattened for MVP).
- **Molt Packages**: First-class support for Rust-backed packages, with production wire formats (MsgPack/CBOR) and Arrow IPC for tabular data; JSON is a compatibility/debug format.
- **AOT Compilation**: Uses Cranelift to generate high-performance machine code.
- **Differential Testing**: Verified against CPython 3.12.

## Quick start

```bash
# 1. Install dependencies (Rust + Python 3.12)
# 2. Build the runtime
cargo build --release --package molt-runtime

# 3. Compile and run a Python script
export PYTHONPATH=src
python3 -m molt.cli build examples/hello.py
./hello_molt

# Use JSON parsing only when explicitly requested
python3 -m molt.cli build --codec json examples/hello.py
```

## Architecture

See `docs/spec/` for detailed architectural decisions.
- `0002-architecture.md`: IR Stack & Pipeline
- `0003-runtime.md`: NaN-boxed Object Model & Memory Management
- `0005-wasm-interop.md`: WASM & FFI Strategy
- `0009_GC_DESIGN.md`: Hybrid RC + Generational GC
- `0012_MOLT_COMMANDS.md`: CLI command specification
- `0013_PYTHON_DEPENDENCIES.md`: Dependency compatibility strategy

## Testing

### CI Parity Jobs
- **WASM parity**: CI runs a dedicated `test-wasm` job that executes `tests/test_wasm_control_flow.py` via Node.
- **Differential suite**: CI runs `python tests/molt_diff.py tests/differential/basic` on CPython 3.12.

### Local Commands
- Python: `tools/dev.py test` (runs `pytest -q` via `uv run` on Python 3.12/3.13/3.14)
- Rust: `cargo test`
- Differential: `python tests/molt_diff.py <case.py>`
- Bench setup (optional): `uv sync --group bench --python 3.12` (Numba requires <3.13)

## Performance & Comparisons

After major features or optimizations, run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` and update this
section with a short summary (date/host, top speedups, regressions, and any build failures).
Install optional baselines with `uv sync --group bench --python 3.12` to enable Cython/Numba
columns. PyPy baselines use `uv run --no-project --python pypy@3.11` to bypass
`requires-python` and remain comparable.
For cross-version baselines, run the bench harness under each CPython version
(`uv run --python 3.12 python3 tools/bench.py --json-out bench/results/bench_py312.json`,
`uv run --python 3.13 python3 tools/bench.py --json-out bench/results/bench_py313.json`,
`uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench_py314.json`)
and summarize deltas across files.

Latest run: 2026-01-04 (macOS x86_64, CPython 3.14.0).
Top speedups: `bench_parse_msgpack.py` 66.78x, `bench_matrix_math.py` 10.83x,
`bench_prod_list.py` 6.84x, `bench_sum.py` 6.64x, `bench_str_count.py` 5.60x.
Regressions: `bench_str_split.py` 1.82x (down from 2.32x prior run; investigate).
Build/run failures: PyPy/Cython/Numba baselines skipped.

### Performance Gates
- Vector reductions (`bench_sum_list.py`, `bench_min_list.py`, `bench_max_list.py`, `bench_prod_list.py`): regression >5% fails the gate.
- String kernels (`bench_str_find.py`, `bench_str_find_unicode.py`, `bench_str_split.py`, `bench_str_replace.py`, `bench_str_count.py`, `bench_str_count_unicode.py`): regression >7% fails the gate.
- Matrix/buffer kernels (`bench_matrix_math.py`): regression >5% fails the gate.
- Any expected perf deltas from new kernels must be recorded here after the run; complex regressions move to `OPTIMIZATIONS_PLAN.md`.

Baseline microbenchmarks (2026-01-04): `bench_min_list.py` 1.94x, `bench_max_list.py` 2.04x,
`bench_prod_list.py` 6.84x, `bench_str_find_unicode.py` 4.68x, `bench_str_count_unicode.py` 1.86x.

| Benchmark | Molt vs CPython | Notes |
| --- | --- | --- |
| bench_matrix_math.py | 10.83x | buffer2d matmul lowering |
| bench_deeply_nested_loop.py | 4.69x | nested loop lowering |
| bench_str_endswith.py | 4.90x | string endswith fast path |
| bench_str_startswith.py | 5.12x | string startswith fast path |
| bench_str_count.py | 5.60x | string count fast path |
| bench_str_split.py | 1.82x | optimized split builder |
| bench_str_replace.py | 4.63x | SIMD-friendly replace path |
| bench_str_join.py | 2.19x | pre-sized join buffer |
| bench_sum_list.py | 2.65x | vector reduction fast path |
