# Molt

A research-grade project to compile a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM),
with strict reproducibility, rigorous testing, and staged compatibility.

> Molt = Python shedding its skin into native code.

## Capabilities (Current)

- **Tier 0 Structification**: Compiles typed Python classes to native structs with fixed-offset access.
- **Native Async**: Compiles `async/await` syntax (currently flattened for MVP).
- **Async iteration**: Supports `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for` (sync-iter fallback enabled for now).
- **Molt Packages**: First-class support for Rust-backed packages, with production wire formats (MsgPack/CBOR) and Arrow IPC for tabular data; JSON is a compatibility/debug format.
- **AOT Compilation**: Uses Cranelift to generate high-performance machine code.
- **Differential Testing**: Verified against CPython 3.12.

## Limitations (Current)

- **Classes & object model**: no inheritance, metaclasses, descriptors, `@classmethod`/`@staticmethod`/`@property`, or full class objects (class names are compile-time only).
- **Attributes**: instances use fixed struct fields with a dynamic instance-dict fallback; no `__getattr__`/`__setattr__` hooks and no user-defined `__slots__` beyond dataclass lowering.
- **Dataclasses**: compile-time lowering for frozen/eq/repr/slots; no `default_factory`, `kw_only`, or `order`; runtime `dataclasses` module provides metadata only.
- **Exceptions**: `try/except/else/finally` + `raise`/reraise support; still partial vs full BaseException semantics (see `docs/spec/0014_TYPE_COVERAGE_MATRIX.md`).
- **Imports**: static module graph only; no dynamic import hooks or full package resolution; allowlisted stdlib modules (e.g., `math`, `random`, `time`, `json`, `re`, `collections`, `itertools`, `functools`, `operator`, `base64`, `pickle`, `unittest`, `site`, `sysconfig`) may load empty stubs for dependency tracking unless implemented.
- **Stdlib**: partial shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `sys`, and `os`; full API parity pending.
- **Async iteration**: `anext` is await-only, and `__aiter__` must return an async iterator (awaitable `__aiter__` still pending).
- **Asyncio**: shim covers `run`/`sleep` only (no loop/task APIs; sleep delay ignored).

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

Latest run: 2026-01-06 (macOS x86_64, CPython 3.14.0).
Top speedups: `bench_sum.py` 231.69x, `bench_parse_msgpack.py` 26.58x,
`bench_matrix_math.py` 10.47x, `bench_prod_list.py` 6.66x, `bench_str_find_unicode_warm.py` 5.95x.
Regressions: `bench_str_count_unicode_warm.py` 0.25x (cache warm path slowdown; investigate).
Build/run failures: Cython/Numba baselines skipped.

### Performance Gates
- Vector reductions (`bench_sum_list.py`, `bench_min_list.py`, `bench_max_list.py`, `bench_prod_list.py`): regression >5% fails the gate.
- String kernels (`bench_str_find.py`, `bench_str_find_unicode.py`, `bench_str_split.py`, `bench_str_replace.py`, `bench_str_count.py`, `bench_str_count_unicode.py`): regression >7% fails the gate.
- Matrix/buffer kernels (`bench_matrix_math.py`): regression >5% fails the gate.
- Any expected perf deltas from new kernels must be recorded here after the run; complex regressions move to `OPTIMIZATIONS_PLAN.md`.

Baseline microbenchmarks (2026-01-06): `bench_min_list.py` 1.90x, `bench_max_list.py` 1.98x,
`bench_prod_list.py` 6.66x, `bench_str_find_unicode.py` 5.19x, `bench_str_count_unicode.py` 1.79x.

| Benchmark | Molt vs CPython | Notes |
| --- | --- | --- |
| bench_matrix_math.py | 10.47x | buffer2d matmul lowering |
| bench_deeply_nested_loop.py | 3.61x | nested loop lowering |
| bench_str_endswith.py | 5.07x | string endswith fast path |
| bench_str_startswith.py | 5.12x | string startswith fast path |
| bench_str_count.py | 5.49x | string count fast path |
| bench_str_split.py | 1.62x | optimized split builder |
| bench_str_replace.py | 4.31x | SIMD-friendly replace path |
| bench_str_join.py | 2.54x | pre-sized join buffer |
| bench_sum_list.py | 2.47x | vector reduction fast path |
