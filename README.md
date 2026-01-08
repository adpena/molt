# Molt

A research-grade project to compile a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM),
with strict reproducibility, rigorous testing, and staged compatibility.

> Molt = Python shedding its skin into native code.

Canonical status lives in `docs/spec/STATUS.md` (README and ROADMAP are kept in sync).

## Capabilities (Current)

- **Tier 0 Structification**: Compiles typed Python classes to native structs with fixed-offset access.
- **Native Async**: Compiles `async/await` syntax (currently flattened for MVP).
- **Async iteration**: Supports `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for` (sync-iter fallback enabled for now).
- **Molt Packages**: First-class support for Rust-backed packages, with production wire formats (MsgPack/CBOR) and Arrow IPC for tabular data; JSON is a compatibility/debug format.
- **AOT Compilation**: Uses Cranelift to generate high-performance machine code.
- **Differential Testing**: Verified against CPython 3.12.
- **Sets**: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- **Numeric builtins**: `int()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- **String count slices**: `str.count` supports start/end slices with Unicode-aware offsets.
- **Importable builtins**: `import builtins` binds supported builtins for compiled code.

## Limitations (Current)

- **Classes & object model**: C3 MRO + multiple inheritance + `super()` resolution for attribute lookup; no metaclasses or dynamic `type()` construction; descriptor protocol still partial.
- **Attributes**: instances use fixed struct fields with a dynamic instance-dict fallback; no `__getattr__`/`__setattr__` hooks and no user-defined `__slots__` beyond dataclass lowering.
- **Dataclasses**: compile-time lowering for frozen/eq/repr/slots; no `default_factory`, `kw_only`, or `order`; runtime `dataclasses` module provides metadata only.
- **Exceptions**: `try/except/else/finally` + `raise`/reraise support; still partial vs full BaseException semantics (see `docs/spec/0014_TYPE_COVERAGE_MATRIX.md`).
- **Imports**: static module graph only; no dynamic import hooks or full package resolution; allowlisted stdlib modules (e.g., `math`, `random`, `time`, `json`, `re`, `collections`, `itertools`, `functools`, `operator`, `base64`, `pickle`, `unittest`, `site`, `sysconfig`) may load empty stubs for dependency tracking unless implemented.
- **Stdlib**: partial shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `typing`, `sys`, and `os`; import-only stubs for `collections.abc` and `importlib` (dynamic import hooks pending).
- **Reflection**: `type`, `isinstance`, `issubclass`, and `object` are supported with single-inheritance base chains; no metaclasses or dynamic `type()` construction.
- **Async iteration**: `anext` is await-only, and `__aiter__` must return an async iterator (awaitable `__aiter__` still pending).
- **Asyncio**: shim covers `run`/`sleep` only (no loop/task APIs; sleep delay ignored).
- **Matmul**: `@` is supported only for `molt_buffer`/`buffer2d`; other types raise `TypeError`.
- **Numeric tower**: BigInt heap fallback for large ints; complex/decimal not implemented; missing int helpers (e.g., `bit_length`, `to_bytes`).
- **Format protocol**: conversion flags + numeric mini-language for ints/floats supported; `__format__` fallback, named fields, and locale-aware grouping pending.
- **memoryview**: partial buffer protocol with 1D `format`/`shape`/`strides`; no multidimensional shapes or advanced buffer exports.
- **Offload demo**: `molt_accel` scaffolding exists and a `molt_worker` stdio shell returns a deterministic `list_items` response; compiled entrypoint dispatch, cancellation, and Django demo wiring are still pending.
- **DB layer**: `molt-db` pool skeleton exists; async drivers and Postgres protocol integration are not implemented yet.

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
- `wit/molt-runtime.wit`: WASM runtime intrinsics contract (WIT)
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

Latest run: 2026-01-08 (macOS arm64, CPython 3.14.0).
Top speedups: `bench_sum.py` 221.45x, `bench_channel_throughput.py` 17.95x,
`bench_async_await.py` 12.82x, `bench_matrix_math.py` 7.92x, `bench_prod_list.py` 5.95x.
Regressions: `bench_descriptor_property.py` 0.25x, `bench_fib.py` 0.26x,
`bench_struct.py` 0.31x, `bench_str_split.py` 0.42x, `bench_max_list.py` 0.53x.
Build/run failures: Cython/Numba baselines skipped.

### Performance Gates
- Vector reductions (`bench_sum_list.py`, `bench_min_list.py`, `bench_max_list.py`, `bench_prod_list.py`): regression >5% fails the gate.
- String kernels (`bench_str_find.py`, `bench_str_find_unicode.py`, `bench_str_split.py`, `bench_str_replace.py`, `bench_str_count.py`, `bench_str_count_unicode.py`): regression >7% fails the gate.
- Matrix/buffer kernels (`bench_matrix_math.py`): regression >5% fails the gate.
- Any expected perf deltas from new kernels must be recorded here after the run; complex regressions move to `OPTIMIZATIONS_PLAN.md`.

Baseline microbenchmarks (2026-01-08): `bench_min_list.py` 0.54x, `bench_max_list.py` 0.53x,
`bench_prod_list.py` 5.95x, `bench_str_find_unicode.py` 4.79x, `bench_str_count_unicode.py` 2.02x.

| Benchmark | Molt vs CPython | Notes |
| --- | --- | --- |
| bench_matrix_math.py | 7.92x | buffer2d matmul lowering |
| bench_deeply_nested_loop.py | 1.74x | nested loop lowering |
| bench_str_endswith.py | 5.02x | string endswith fast path |
| bench_str_startswith.py | 4.97x | string startswith fast path |
| bench_str_count.py | 5.19x | string count fast path |
| bench_str_split.py | 0.42x | optimized split builder |
| bench_str_replace.py | 4.43x | SIMD-friendly replace path |
| bench_str_join.py | 0.72x | pre-sized join buffer |
| bench_sum_list.py | 0.67x | vector reduction fast path |
