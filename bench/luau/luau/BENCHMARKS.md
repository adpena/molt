# Depyler-Ported Benchmarks

Benchmarks ported from [Depyler](https://github.com/paiml/depyler), a Python-to-Rust
transpiler by PAI ML. Depyler claims up to **12.36x** speedup over CPython for its
compute-intensive benchmark (Fibonacci + statistics), and markets a **340%** (4.4x)
general performance improvement in its MCP integration documentation.

## Source Analysis

Depyler's benchmark corpus is small. The repository contains:

- **One official benchmark**: `benchmarks/python/compute_intensive.py` (Fibonacci sums + statistics)
- **Test fixtures**: `tests/fixtures/python_samples/` with 6 files covering basic functions,
  control flow (factorial, fibonacci, binary search, bubble sort, prime check, GCD, etc.),
  list operations, string operations, and dictionary operations
- **Algorithm examples**: `examples/algorithms/` with fibonacci, quicksort, binary search

The "340% faster" claim appears in their MCP integration doc as an **illustrative example
of tool output format**, not from validated empirical benchmarks. Their PERFORMANCE.md
reports **12.36x** for the compute-intensive benchmark, but notes the transpiler had bugs
requiring a manually-written Rust implementation.

## Benchmark Inventory

| File | Source | Measures | CPython (ms) |
|------|--------|----------|-------------|
| `bench_depyler_compute.py` | `benchmarks/python/compute_intensive.py` | Iterative fibonacci, list accumulation, min/max statistics | ~510 |
| `bench_depyler_factorial.py` | `tests/fixtures/.../control_flow.py` | Recursive factorial, integer multiplication | ~250 |
| `bench_depyler_binary_search.py` | `tests/fixtures/...` + `examples/algorithms/` | Binary search + linear search on sorted arrays | ~70 |
| `bench_depyler_bubble_sort.py` | `tests/fixtures/.../control_flow.py` | O(n^2) nested-loop sorting, element swaps | ~280 |
| `bench_depyler_prime_sieve.py` | `tests/fixtures/.../control_flow.py` | Trial-division primality, modulo arithmetic | ~330 |
| `bench_depyler_gcd.py` | `tests/fixtures/.../control_flow.py` | Euclidean GCD, while-loop + modulo throughput | ~520 |
| `bench_depyler_quicksort.py` | `examples/algorithms/quicksort.py` | In-place quicksort, recursive partitioning | ~220 |
| `bench_depyler_list_ops.py` | `tests/fixtures/.../list_operations.py` | List append, sum, max, filter, reverse | ~130 |
| `bench_depyler_string_ops.py` | `tests/fixtures/.../string_operations.py` | String concatenation, char counting, reversal | ~120 |
| `bench_depyler_digits.py` | `tests/fixtures/.../control_flow.py` | Integer modulo/division (digit sum, reverse int) | ~540 |
| `bench_depyler_power.py` | `tests/fixtures/.../control_flow.py` | Iterative exponentiation, tight multiply loops | ~120 |

CPython baselines measured on macOS (Apple Silicon), Python 3.x, single run.

## Depyler's Published Numbers

From `PERFORMANCE.md` (their only validated benchmark, `compute_intensive.py`):

| Metric | Python | Rust (manual) | Speedup |
|--------|--------|---------------|---------|
| Execution time | 10.1 ms | 819.8 us | 12.36x |
| Memory usage | 9,328 KB | 1,936 KB | 4.8x |

Environment: Linux 6.8.0, x86_64, measured with hyperfine.
Note: Rust code was **manually written**, not auto-transpiled (their transpiler had bugs).

## Adaptation Notes

All benchmarks were adapted for the Molt-Luau backend's supported feature set:
- `while` loops instead of `for ... in range(...)` patterns
- No list comprehensions, no tuple unpacking, no classes, no exceptions
- Type annotations on all variables and function signatures
- `main()` function called at module level (not behind `if __name__`)
- Returns `int` (0/1) instead of `bool` where needed
- Uses `list[int]` instead of `dict` for statistics results

## Running

```bash
# CPython baselines only
python benchmarks/luau/run_benchmarks.py --cpython-only

# Full comparison (requires molt + lune)
python benchmarks/luau/run_benchmarks.py
```

## Depyler CLI

Depyler provides `depyler transpile <file.py>` and `depyler compile <file.py>` commands.
There is no `depyler bench` subcommand. To compare directly, install via `cargo install depyler`
and transpile individual files, then compile and time the resulting Rust binaries.
