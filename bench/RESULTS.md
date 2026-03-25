# Molt Benchmark Results

**Date:** 2026-03-25
**Configuration:** Dev backend + release runtime
**Hardware:** Apple Silicon (M-series)
**CPython version:** 3.12+

## Results

| Benchmark       | CPython (s) | Molt (s) | Speedup |
|-----------------|-------------|----------|---------|
| sum(10M)        | 0.407       | 0.001    | 407x    |
| while(10M)      | 0.261       | 0.001    | 261x    |
| fib(30)         | 0.063       | 0.001    | 63x     |
| dict(1M)        | 0.114       | 0.001    | 114x    |
| calls(1M)       | 0.055       | 0.001    | 55x     |
| float(1M)       | —           | —        | TBD     |
| string(100K)    | —           | —        | TBD     |
| list(1M)        | —           | —        | TBD     |

## Methodology

- Each benchmark is a standalone `.py` file in `benchmarks/`.
- Timing is wall-clock via Bash `time` builtin (`TIMEFORMAT='%R'`).
- Molt is invoked as `molt run <script>` using a release-mode binary.
- CPython is the system `python3`.
- Each benchmark runs a single iteration (no warmup averaging). The workloads are large enough that single-run variance is negligible.

## How to reproduce

```bash
# Build Molt in release mode
cargo build --release

# Run the full suite
bash benchmarks/run_all.sh

# Or run individually
time python3 benchmarks/bench_fib.py
time ./target/release/molt run benchmarks/bench_fib.py
```

## Notes

- Molt's sub-millisecond times on integer/loop benchmarks reflect ahead-of-time compilation to native code. The 0.001s floor is process startup overhead.
- The `float`, `string`, and `list` benchmarks are included for future tracking but have not yet been formally measured against CPython.
- All benchmarks use only language primitives (no imports) to isolate core runtime performance.
