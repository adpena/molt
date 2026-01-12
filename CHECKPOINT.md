Checkpoint: 2026-01-12T00:15:44-0600
Git: 821f9242932c9e8e41e1b3e2d8f24f6e94d2e446

Summary
- Fixed WASM table collisions/stack issues and refreshed bench outputs in the previous commit.
- Collapsed nested `if` checks in `molt_is_callable` to satisfy clippy.
- CI green on `main` (Run 20909814135).

Files touched (uncommitted)
- logs/clif_fib.txt
- logs/clif_functools.txt
- logs/clif_lru.txt
- logs/clif_lru_factory.txt
- logs/clif_sum_list.txt
- logs/clif_wrapper.txt
- logs/ir_fib.txt

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`
- `cargo clippy -- -D warnings`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
