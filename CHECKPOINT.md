Checkpoint: 2026-01-12T01:59:16-0600
Git: 7a9177263a05b91d59c370c22d859efdd736c7a6 (dirty)

Summary
- Added async free-var closure capture (stored in future payload) and async decorator diff coverage.
- Routed closure-backed CALL_FUNC paths through call_bind via new `molt_function_closure_bits` (native + WASM).
- Updated wasm harness to support `func_new_closure` + `function_closure_bits` and to pass closure args.
- Fixed CI rustfmt failure via `cargo fmt` and updated README/STATUS for async closures + ASGI shim.

Files touched (uncommitted)
- CHECKPOINT.md

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`

Benchmarks
- Not run in this session.

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
