Checkpoint: 2026-01-11T17:34:48-0600
Git: cfd0a1a5d0d46a20d8d31db97cf3041f38ec8bfd (dirty)

Summary
- Fixed capability lookup crash by memoizing parsed capabilities and avoiding temporary set membership in `has()`.
- Fixed stdlib root `__init__.py` module naming to avoid empty module names.
- Updated Django demo path and checkpoint freshness requirements in docs.
- Applied `cargo fmt` to satisfy CI rustfmt on the wasm backend.

Files touched (uncommitted)
- AGENTS.md
- CHECKPOINT.md
- Cargo.lock
- GEMINI.md
- OPTIMIZATIONS_PLAN.md
- ROADMAP.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- docs/spec/STATUS.md
- logs/clif_fib.txt
- logs/clif_sum_list.txt
- logs/ir_fib.txt
- run_wasm.js
- runtime/molt-backend/Cargo.toml
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/capabilities.py
- src/molt/cli.py
- src/molt/frontend/__init__.py
- tests/wasm_harness.py
- tools/bench_wasm.py
- tools/wasm_link.py
- wit/molt-runtime.wit

Docs/spec updates needed?
- None (roadmap updated).

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `cargo fmt`

Known gaps
- wasm perf is still ~2-4x slower than CPython on list/min/max and struct/descriptor benches (bench_struct ~4.3x).
- wasm table init uses a start-function; element segments still pending if reloc.ELEM is restored.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
