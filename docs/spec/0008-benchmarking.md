# Molt Benchmark Plan

## 1. Methodology
- **Baselines**: Compare against CPython 3.12/3.13/3.14 (multi-version baselines),
  PyPy (via `uv run --no-project --python pypy@3.11` to bypass `requires-python`),
  and optional Cython/Numba baselines when the `bench` dependency group is
  installed with Python 3.12.
- **Environment**:
    - macOS arm64 (M1/M2/M3)
    - Linux x86_64 (Ubuntu 22.04)
- **Metrics**:
    - **Wall clock time**: Total execution time.
    - **CPU Cycles / Instructions**: Using `perf` (Linux) or `instruments` (macOS).
    - **Peak RSS**: Max memory usage.
    - **Startup Time**: Time to first instruction.
    - **Binary Size**: Stripped native executable size.

## 2. Benchmark Suites

### A. Micro-benchmarks
- `fib`: Recursive Fibonacci (integer math + function calls).
- `list_ops`: Repeated appending and sorting (collection overhead).
- `string_concat`: Building large strings (memory allocation + UTF-8).
- `dict_lookup`: Hot-path dictionary access.
- `sum_ints`: Reduction over 10M ints (vectorization target).
- `dot_ints`: Dot product over 10M ints (vectorization + memory bandwidth).
- `bytes_find`: Scan 100MB bytes for sentinel (SIMD scan target).

### B. Service Benchmarks
- `structured_parse`: Parsing 10MB of nested MsgPack/CBOR (serialization cost; JSON tracked for compatibility).
- `http_hello`: Minimal async HTTP server (concurrency + I/O).
- `db_hydration`: Mapping 10,000 DB rows to objects (ORM-like overhead).

### C. Pipeline Benchmarks
- `data_transform`: Filter/Map/Reduce on 1M records.
- `uuid_gen`: Generating and formatting 1M UUIDs.
- `log_scan`: Line-by-line parse + reduce (loop hot path, branch-heavy).

## 3. Reporting
- Automated generation of Markdown tables and graphs in `docs/benchmarks/`.
- Regression alerts in CI.
- `tools/bench.py` writes JSON results under `bench/results/` and supports baseline comparisons via `bench/baseline.json`.
- Install optional benchmark deps with `uv sync --group bench --python 3.12` to enable Cython/Numba.
- Capture CPython version baselines by running the harness under each interpreter:
  `uv run --python 3.12 python3 tools/bench.py --json-out bench/results/bench_py312.json`,
  `uv run --python 3.13 python3 tools/bench.py --json-out bench/results/bench_py313.json`,
  `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench_py314.json`.
