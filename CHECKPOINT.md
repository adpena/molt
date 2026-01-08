Checkpoint: 2026-01-08 07:00:45 CST
Git: b37a241 runtime: lazy utf8 count prefix cache

Summary
- Made UTF-8 count cache prefix metadata lazy to avoid full-count regressions while keeping slice paths fast.
- Added struct layout mutation differential coverage for class layout deopts.
- Refreshed OPT-0006 notes and updated README performance summary with new bench results.
- Re-ran benches; str.count unicode warm is back to 4.18x with no major regressions.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `cargo test -p molt-runtime`
- `python3 tools/runtime_safety.py miri`
- `cargo +nightly fuzz run string_ops -- -max_total_time=30`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`

Known gaps
- OPT-0007 struct store fast path + layout-stability guard still pending (bench_struct ~0.31x).
- OPT-0008 descriptor/property fast path still pending (bench_descriptor_property ~0.25x).
- OPT-0009 string split/join builder still below parity (bench_str_split ~0.42x).
- Class layout stability guard for structified classes not implemented yet.
- See `docs/spec/STATUS.md` and `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for remaining stdlib gaps.

Pending changes
- CHECKPOINT.md

Next 5-step plan
1) Audit bench regressions >5% (sum_list/attr_access/str_split) and log deltas in `OPTIMIZATIONS_PLAN.md` if real.
2) Implement OPT-0007 layout-stability guard + monomorphic slot stores to cut bench_struct overhead.
3) Implement OPT-0008 descriptor/property lookup IC and add targeted diffs.
4) Implement OPT-0009 split/join builder fast paths and re-bench.
5) Re-run benches, then sync README/ROADMAP/STATUS with updated performance + coverage.
