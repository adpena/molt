Checkpoint: 2026-01-08 00:04:29 CST
Git: ecfab0f runtime: fix clippy and linux link

Summary
- Fixed wasm type table indexing so user function types don’t alias the 6-arg string_count_slice signature (async parity build now validates).
- Revalidated wasm async protocol parity and ran full lint/test/differential passes across supported Python versions.
- Ran miri and string_ops fuzz (bounded run completed; long fuzz run interrupted manually after coverage settled).
- Committed BigInt heap fallback, format mini-language expansion, memoryview metadata, and corresponding tests/docs updates.
- Applied cargo fmt cleanup after CI reported rustfmt drift.
- Addressed clippy warnings in runtime helpers and added `-lm` on Linux link to fix CI linker failures.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- `uv run --python 3.12 pytest tests/test_wasm_async_protocol.py -q`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `python3 tools/runtime_safety.py miri`
- `python3 tools/runtime_safety.py fuzz --target string_ops` (interrupted)
- `cargo +nightly fuzz run string_ops -- -max_total_time=10`
- `cargo fmt --check` (failed before reformat)
- `cargo fmt`
- `cargo clippy -- -D warnings`

Known gaps
- Format protocol still lacks `__format__` fallback, named field formatting, and locale-aware grouping.
- memoryview remains 1D only (no multidimensional shapes or advanced buffer exports).
- Numeric tower still missing complex/decimal and int helpers (`bit_length`, `to_bytes`, `from_bytes`).
- Matmul remains buffer2d-only; no `__matmul__`/`__rmatmul__` for arbitrary types.
- OPT-0005/6/7 perf follow-through still pending; benches not rerun.

Pending changes
- CHECKPOINT.md

Next 5-step plan
1) Re-run benches and update `bench/results/bench.json`, then summarize in README/ROADMAP if deltas are significant.
2) Implement OPT-0006 prefix-count metadata for Unicode count cache and re-benchmark warm/cold.
3) Implement OPT-0007 struct store fast-path with deopt guards and add a mutation differential test.
4) Extend format/memoryview parity: `__format__` fallback and multidimensional buffer exports.
5) Revisit STATUS/ROADMAP after perf work and update any newly closed gaps.
