Checkpoint: 2026-01-11 04:58:09 CST
Git: 0a4bb694500c6e3327b91c5f21793c2bde4ddffb

Summary
- Fixed native poll ABI by passing raw task pointers into poll functions and
  aligning block_on/spawn call conventions.
- Scoped WASM label blocks to jump targets to avoid invalid else/if nesting
  (generator protocol parity restored).
- Refreshed native + WASM benchmark outputs and tracked `bench_wasm.json`.

Files touched (uncommitted)
- CHECKPOINT.md

Docs/spec updates needed?
- None.

Tests run
- `cargo build --release --package molt-runtime`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`

Known gaps
- Layout guard overhead remains high; bench_struct/bench_attr_access/bench_fib still below CPython.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
