Checkpoint: 2026-01-11 07:06:05 CST
Git: 91e67e67afb2897b3d84428a70910b2c4a0fdeab

Summary
- Refreshed native + wasm benchmark results for the next checkpoint commit.

Files touched (uncommitted)
- CHECKPOINT.md

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`

Known gaps
- Layout guard overhead remains high; bench_struct/bench_attr_access/bench_fib still below CPython.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
