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
- **WASM parity**: CI runs a dedicated `test-wasm` job that executes `tests/test_wasm_if_else.py` via Node.
- **Differential suite**: CI runs `python tests/molt_diff.py tests/differential/basic` on CPython 3.12.

### Local Commands
- Python: `uv run pytest`
- Rust: `cargo test`
- Differential: `python tests/molt_diff.py <case.py>`
- Bench setup (optional): `uv sync --group bench --python 3.12` (Numba requires <3.13)

## Performance & Comparisons

After major features or optimizations, run `python3 tools/bench.py --json` and update this
section with a short summary (date/host, top speedups, regressions, and any build failures).
Install optional baselines with `uv sync --group bench --python 3.12` to enable Cython/Numba
columns. PyPy baselines are skipped until a PyPy release satisfies `requires-python`.

Latest run: 2026-01-03 (macOS arm64, CPython 3.12.12).
Top speedups: `bench_str_split.py` 10.80x, `bench_str_count.py` 5.73x,
`bench_bytes_find.py` 5.61x, `bench_str_find.py` 5.57x, `bench_str_endswith.py` 5.34x.
Regressions: none vs CPython among successful runs.
Build/run failures: `bench_fib.py`, `bench_matrix_math.py`, `bench_str_join.py`.

| Benchmark | Molt vs CPython | Notes |
| --- | --- | --- |
| bench_str_split.py | 10.80x | string split fast path |
| bench_str_count.py | 5.73x | string count fast path |
| bench_bytes_find.py | 5.61x | bytes find fast path |
| bench_str_find.py | 5.57x | string find fast path |
| bench_str_endswith.py | 5.34x | string endswith fast path |
| bench_fib.py | n/a | Molt build/run failed |
| bench_matrix_math.py | n/a | Molt build/run failed |
| bench_str_join.py | n/a | Molt build/run failed |
