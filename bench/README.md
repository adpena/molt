# Benchmark Guide

Last updated: 2026-03-19

This directory contains benchmark artifacts, harness helpers, and friend-suite configuration.

## Primary Commands

- Native: `UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench.py`
- WASM: `UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_wasm.py --linked`
- Combined report: `UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_report.py`

Use canonical roots:

```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_CACHE=$PWD/.molt_cache
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
```

## Current State

- The newest committed combined summary remains stale at `docs/benchmarks/bench_summary.md` (generated 2026-01-19).
- The newest local wasm evidence in-tree is from 2026-03-17:
  - `bench/results/bench_wasm_20260317_130635.json`
  - `bench/results/bench_wasm_20260317_130715.json`
- Those 2026-03-17 artifacts are not green:
  - linked wasm targeted cases failed with `undeclared reference to function #90`
  - unlinked/direct mode failed with `Direct-link mode is unavailable for this wasm artifact`

## Freshness Policy

- Do not treat `docs/benchmarks/bench_summary.md` as current performance truth unless it has been regenerated in the same change.
- When native or wasm benchmark refreshes fail or stall, record the blocker in `docs/benchmarks/optimization_progress.md` and keep raw JSON artifacts under `bench/results/`.
