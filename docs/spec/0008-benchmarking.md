# Molt Benchmark Plan

## 1. Methodology
- **Baselines**: Compare against CPython 3.12 (standard) and PyPy (where applicable).
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

### B. Service Benchmarks
- `json_parse`: Parsing 10MB of nested JSON (serialization cost).
- `http_hello`: Minimal async HTTP server (concurrency + I/O).
- `db_hydration`: Mapping 10,000 DB rows to objects (ORM-like overhead).

### C. Pipeline Benchmarks
- `data_transform`: Filter/Map/Reduce on 1M records.
- `uuid_gen`: Generating and formatting 1M UUIDs.

## 3. Reporting
- Automated generation of Markdown tables and graphs in `docs/benchmarks/`.
- Regression alerts in CI.
