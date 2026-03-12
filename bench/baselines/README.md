# Benchmark Baselines

This directory stores baseline benchmark snapshots used by `tools/perf_regression.py`
for performance regression detection.

## Baseline Format

Each baseline file is a standard Molt benchmark JSON file (same schema as
`bench/results/bench.json` produced by `tools/bench.py`).  The top-level
structure is:

```json
{
  "schema_version": 1,
  "created_at": "2026-01-04T02:35:41.118510+00:00",
  "git_rev": "abc123...",
  "super_run": false,
  "samples": 3,
  "warmup": 1,
  "system": {
    "platform": "macOS-26.2-arm64-arm-64bit",
    "python": "3.12.12",
    "machine": "arm64",
    "cpu_count": 10,
    "load_avg": [2.1, 1.8, 1.5]
  },
  "benchmarks": {
    "bench_fib.py": {
      "cpython_time_s": 0.305,
      "molt_time_s": 0.099,
      "molt_build_s": 1.23,
      "molt_size_kb": 1797.27,
      "molt_speedup": 3.07,
      "molt_cpython_ratio": 0.326,
      "molt_ok": true
    }
  }
}
```

### Per-benchmark fields used by the regression detector

| Field | Type | Description |
|-------|------|-------------|
| `molt_time_s` | float | Runtime in seconds (regression metric: **runtime**) |
| `molt_build_s` | float | Compile time in seconds (regression metric: **compile_time**) |
| `molt_size_kb` | float | Binary size in KB (regression metric: **binary_size**) |
| `molt_ok` | bool | Whether Molt build+run succeeded |

When `--super` mode is used, each benchmark entry also contains a
`super_stats` block with per-runner `mean_s`, `median_s`, `variance_s`,
`range_s`, `min_s`, `max_s` — used for distribution-based statistical tests.

## Creating a baseline

```bash
# Run benchmarks and save as baseline
uv run --python 3.12 python3 tools/bench.py --super --json-out bench/baselines/baseline_$(date +%Y%m%d).json

# Or promote current bench/baseline.json
cp bench/baseline.json bench/baselines/baseline_$(date +%Y%m%d).json
```

## Using baselines for regression detection

```bash
# Compare current results against a baseline
uv run --python 3.12 python3 tools/perf_regression.py \
  --current bench/results/bench.json \
  --baseline bench/baselines/baseline_20260301.json

# Compare against all historical baselines for trend analysis
uv run --python 3.12 python3 tools/perf_regression.py \
  --current bench/results/bench.json \
  --baseline-dir bench/baselines/

# CI gate mode (exit code 1 on regression)
uv run --python 3.12 python3 tools/perf_regression.py \
  --current bench/results/bench.json \
  --baseline bench/baseline.json \
  --json-out /tmp/perf_report.json
```

## Naming convention

- `baseline_YYYYMMDD.json` — dated snapshot
- `baseline_vX.Y.Z.json` — release-tagged snapshot
- The canonical latest baseline is `bench/baseline.json` (repo root level).

## Historical trend analysis

When `--baseline-dir` is provided, the regression detector loads all `.json`
files sorted by `created_at` timestamp and performs linear regression on each
metric to detect gradual performance drift that might not trigger single-run
thresholds.
