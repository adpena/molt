# TIR/Native Performance Snapshot - 2026-04-22

Command pattern:
`uv run --python 3.12 python3 tools/bench.py --script <bench> --samples 3 --warmup 1 --molt-profile release --no-pypy --no-nuitka --no-pyodide --json-out ...`

| Benchmark | CPython s | Molt build s | Molt run s | Molt / CPython | Binary KB | Notes |
|---|---:|---:|---:|---:|---:|---|
| bench_fib.py | 0.0704 | 1.9986 | 0.0669 | 1.05x | 14876.7 | Codon unavailable locally |
| bench_str_find.py | 0.0245 | 4.7052 | 0.0203 | 1.20x | 14892.9 | Codon unavailable locally |
| bench_sum_list.py | 0.0401 | 5.2192 | 0.0090 | 4.48x | 14892.9 | Codon unavailable locally |

Outcome: sampled native Molt release runs faster than CPython on all three selected benchmarks. `bench_fib.py` is the weakest margin and should be the first hot-path target after TIR correctness gates.
Build time remains a separate throughput target; these speedups use run time only.
