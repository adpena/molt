# Benchmarking & Performance Gates

Molt is performance-obsessed. Every major change must be validated against our benchmark suite.

## Version Policy
Benchmarks target **Python 3.12+** semantics. Use 3.12 as the minimum baseline,
and record any 3.13/3.14 divergences in specs/tests.

## Running Benchmarks

We use `tools/bench.py` for native and `tools/bench_wasm.py` for WASM.
To exercise single-module linking, add `--linked` (requires `wasm-ld` and
`wasm-tools`).
For performance parity work, prefer linked WASM artifacts (`tools/bench_wasm.py --linked`)
and use the linked runner path by default.
If you build standalone WASM artifacts for perf validation, use
`uv run --python 3.12 python3 -m molt.cli build --target wasm --require-linked`
to ensure only linked output is produced.
Use `tools/bench_wasm.py --require-linked` to fail fast when linking is unavailable.
For targeted wasm failure triage, use benchmark filtering plus control-runner checks:

```bash
uv run --python 3.12 python3 tools/bench_wasm.py \
  --bench bench_async_await \
  --bench bench_channel_throughput \
  --runner node \
  --control-runner wasmtime \
  --node-max-old-space-mb 8192 \
  --samples 1 \
  --warmup 0 \
  --require-linked
```

This emits `molt_wasm_failure_*` fields and `molt_wasm_control_*` fields per
failed benchmark in JSON outputs for quick node-vs-wasmtime classification.

```bash
# Basic run
uv run --python 3.14 python3 tools/bench.py

# One-off script (CLI wrapper or direct harness)
molt bench --script path/to/script.py
uv run --python 3.14 python3 tools/bench.py --script path/to/script.py

# Record results to JSON (standard for PRs)
uv run --python 3.14 python3 tools/bench.py --json-out bench/results/my_change.json

# Increase warmup runs (default: 1, or 0 for --smoke)
uv run --python 3.14 python3 tools/bench.py --warmup 2

# Comparison vs CPython
uv run --python 3.14 python3 tools/bench.py --compare cpython
```

## Native Baselines (Optional)

`tools/bench.py` can compare Molt against optional native baselines using the
same benchmark scripts:

- **PyPy**: auto-probed via `uv run --python pypy@3.11` (skipped if unavailable).
- **Cython/Numba**: install with `uv sync --group bench --python 3.12` (also included in the `dev` group).
- **Codon**: install `codon` and ensure it is on PATH.

Disable any baseline with `--no-pypy`, `--no-cython`, `--no-numba`, `--no-codon`,
respectively.
Use `--no-cpython` when you want a direct Molt-vs-friend comparison lane without
paying the CPython runtime cost.
Use `--runtime-timeout-sec <seconds>` to cap per-process runtime for long suites
and keep partial runs bounded/reproducible.

## Combined Native + WASM Report

After writing `bench/results/bench.json` and `bench/results/bench_wasm.json` (or
linked output when using `--linked`), generate the
combined report:

```bash
uv run --python 3.14 python3 tools/bench_report.py
```

This writes `docs/benchmarks/bench_summary.md` by default. Commit the report alongside
the JSON results to keep native and WASM performance tracking aligned.
Add `--update-readme` to refresh the Performance & Comparisons block in `README.md`.

## Benchmark Artifact Diffing

Use `tools/bench_diff.py` to compare two benchmark artifacts and highlight
regressions/improvements per metric:

```bash
python3 tools/bench_diff.py \
  bench/results/cluster12_codon_subset_after_stats_coerce_fastpath.json \
  bench/results/cluster13b_codon_subset_samples5_after_setdefault_empty_list_lowering.json \
  --top 10 \
  --json-out bench/results/bench_diff_latest.json
```

Notes:
- By default, it diffs all shared numeric metrics.
- It skips all-zero metrics unless `--include-zero-only-metrics` is passed.
- Use `--metrics` to constrain analysis (for example `--metrics molt_time_s molt_codon_ratio`).

## Friend-Owned Suite Benchmarking

For apples-to-apples validation against friend priorities, use the
friend manifest harness:

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --include-disabled \
  --dry-run
```

Then enable and run pinned suites:

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --suite codon_benchmarks \
  --checkout \
  --fetch \
  --update-doc
```

Artifacts:
- machine-readable: `results.json`
- human summary: `summary.md`
- published summary: `docs/benchmarks/friend_summary.md` (`--update-doc`)

Rules:
- Pin friend repos to immutable `repo_ref` values before enabling suites.
- Record compile and run phases separately when friends compile ahead of run.
- Classify cases as `runs_unmodified`, `requires_adapter`, or `unsupported_by_molt`.

## Binary Size & Cold-Start (Optional)

See `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` for required metrics.
Common tools:
- `cargo bloat -p molt-runtime --release`
- `cargo llvm-lines -p molt-runtime`
- `llvm-size <binary>` (native size)
- `twiggy top output.wasm` (WASM size drivers)
- `wasm-opt -Oz -o output.opt.wasm output.wasm`
- `wasm-tools strip output.opt.wasm -o output.stripped.wasm`
- `gzip -k output.wasm` / `brotli -f output.wasm` (compressed size)

## Performance Gates

We enforce strict "Performance Gates" in CI. If a PR causes a regression beyond these limits, it will be blocked.

| Category | Gate (Max Regression) | Examples |
| --- | --- | --- |
| Vector Reductions | 5% | `sum`, `min`, `max` on lists |
| String Kernels | 7% | `find`, `split`, `replace` |
| Matrix/Buffer | 5% | `matmul`, buffer access |
| General Loops | 10% | CSV parsing, deep loops |

## Lock-Sensitive Benchmarks

When changing the GIL, pointer registry, handle resolution, scheduler locks, or
other runtime synchronization, run a targeted subset in addition to the full
suite. Prioritize workloads that stress attribute access/descriptor dispatch,
struct/shape access, container ops, deep loops, and channel throughput.
Validate native + WASM parity for the same cases.

## How to Interpret Results

- **Speedup (x.xx)**: Molt is X times faster than CPython. (e.g., 10.0x = Molt is 10x faster).
- **Regression (< 1.0x)**: Molt is slower than CPython. This is generally unacceptable for Tier 0 constructs.
- **Super Bench (`--super`)**: Runs 10 samples and calculates variance. Use this for final release validation or when results are noisy.
- **Molt build vs run time**: `molt_build_s` captures compile time; `molt_time_s` is run time only for fair runtime comparisons.
- **WASM build vs run time**: `molt_wasm_build_s` captures wasm compile time; `molt_wasm_time_s` is run time only.

## Profiles

Use `molt profile <script.py>` to generate flamegraphs and identify bottlenecks in the compiler or runtime.

### Runtime Hot-Path Counters (`MOLT_PROFILE_JSON`)

For runtime attribution work, emit machine-readable counters from compiled runs:

```bash
PYTHONPATH=src \
MOLT_PROFILE=1 \
MOLT_PROFILE_JSON=1 \
uv run --python 3.12 python3 -m molt.cli run --profile dev --trusted \
  bench/friends/repos/codon_benchmarks/bench/codon/sum.py
```

Notes:
- `molt_profile ...` (text) and `molt_profile_json {...}` (JSON) are emitted on stderr.
- For file-driven Codon cases, pass explicit input paths:
  - `word_count.py <input_file>`
  - `taq.py <input_file>`
- Keep these runs in `--profile dev` for iterative optimization loops; use `--profile release` for publication-grade benchmark reports.

### Native-Arch Perf Profile (Opt-In)

For production-grade native benchmark runs, enable the native-arch profile:

```bash
MOLT_PERF_PROFILE=native-arch \
uv run --python 3.14 python3 tools/bench.py --compare codon
```

Equivalent toggle: `MOLT_NATIVE_ARCH_PERF=1`.
When enabled for `target=native`, Molt appends `-C target-cpu=native` to `RUSTFLAGS`.

## Compile Throughput Tuning

- Bootstrap a consistent throughput environment first:
  - `eval "$(tools/throughput_env.sh --print)"`
  - or `tools/throughput_env.sh --apply` (configures `sccache` size and runs cache prune policy)
- Throughput bootstrap defaults `CARGO_INCREMENTAL=0` to maximize cacheability/shared throughput under multi-agent contention. Set `CARGO_INCREMENTAL=1` only for local incremental-debug sessions.
- Prefer `--profile dev` for iteration loops (`molt build/run/compare/diff/test --suite diff`); reserve `--profile release` for release gates and perf publication.
- `--profile dev` routes to Cargo `dev-fast` by default; override with `MOLT_DEV_CARGO_PROFILE` when profiling alternative dev profiles.
- Keep cache keys deterministic by default (`PYTHONHASHSEED=0` is enforced by CLI). Override via `MOLT_HASH_SEED=<value>` only when explicitly testing hash-seed sensitivity.
- Enable Rust compile caching:
  - `MOLT_USE_SCCACHE=1` (or leave default `auto` when `sccache` is installed)
  - `sccache -s` to inspect hit rates
- Keep backend daemon enabled for native compile loops (`MOLT_BACKEND_DAEMON=1`; default) so Cranelift initialization is amortized across builds.
- In multi-agent runs, share cache/target roots on the external volume to improve reuse:
  - `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache`
  - `CARGO_TARGET_DIR=/Volumes/APDataStore/Molt/target`
- Keep diff runs on the same shared target:
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` (set automatically by `tools/throughput_env.sh --apply`)
- For differential throughput, wrappers are disabled by default for portability; opt in only on stable hosts:
  - `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`

### Suggested Throughput Baseline Command

```bash
MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache \
CARGO_TARGET_DIR=/Volumes/APDataStore/Molt/target \
MOLT_USE_SCCACHE=1 \
uv run --python 3.12 python3 -m molt.cli build examples/hello.py --profile dev --cache-report
```

### Throughput Matrix Harness

Use the dedicated matrix harness to compare single-agent vs concurrent throughput
across profile and wrapper modes:

```bash
uv run --python 3.12 python3 tools/throughput_matrix.py \
  --concurrency 2 \
  --timeout-sec 75 \
  --shared-target-dir /Users/$USER/.molt/throughput_target \
  --run-diff \
  --diff-jobs 2 \
  --diff-timeout-sec 180
```

- Results are written to `matrix_results.json` under the chosen output root.
- When `/Volumes/APDataStore/Molt` exists, outputs default there automatically.
- Diff matrix runs always set `MOLT_DIFF_MEASURE_RSS=1` and enforce `MOLT_DIFF_RLIMIT_GB=10`.
- Prefer `--shared-target-dir` on a hard-link-friendly filesystem (APFS/ext4). If Cargo reports incremental hard-link fallback, move the target dir off filesystems like exFAT.

### Compile Progress Tracker

Use the compile progress suite to track the optimization initiative with stable
case definitions (cold/warm, cache-hit/no-cache, daemon on/off, and
`release-fast` iteration lanes):

```bash
uv run --python 3.12 python3 tools/compile_progress.py --clean-state
```

Add `--diagnostics` to collect per-case compiler phase timings/module reason
payloads automatically.

- Outputs:
  - `compile_progress.json` (machine-readable snapshot)
  - `compile_progress.md` (human summary table)
  - per-case logs under `logs/`
  - per-case diagnostics under `diagnostics/` when `--diagnostics` is set
  - snapshots are refreshed after every completed case to preserve partial
    progress if a long run is interrupted
- Optional compiler diagnostics (phase timings + module inclusion reasons):
  - `MOLT_BUILD_DIAGNOSTICS=1`
  - `MOLT_BUILD_DIAGNOSTICS_FILE=build_diagnostics.json`
  - Example:
    `MOLT_BUILD_DIAGNOSTICS=1 MOLT_BUILD_DIAGNOSTICS_FILE=build_diag.json uv run --python 3.12 python3 -m molt.cli build --profile dev --no-cache examples/hello.py`
- Queue lanes (daemon warm queue, opt-in):
  - `--cases dev_queue_daemon_on dev_queue_daemon_off`
  - each queue case performs warmup runs before the measured attempt
- Release-iteration lanes (`MOLT_RELEASE_CARGO_PROFILE=release-fast`):
  - `--cases release_fast_cold release_fast_warm release_fast_nocache_warm`
- Contention controls (recommended on busy hosts):
  - `--max-retries 2 --retry-backoff-sec 2 --build-lock-timeout-sec 60`
  - timed-out attempts now perform run-scoped compiler cleanup before retrying
    (kills stale `cargo`/`rustc`/`sccache` children and run-scoped backend
    daemons)
  - `SIGTERM` exits (`rc=143`/`rc=-15`) are classified as retryable
  - add `--resume` for persistent-shell reruns so interrupted sweeps continue
    from already completed cases
- Default output root:
  - `/Volumes/APDataStore/Molt/compile_progress_<timestamp>` when available
  - `bench/results/compile_progress/<timestamp>` otherwise
- Progress board and KPI targets live in
  `docs/benchmarks/compile_progress.md`.

### Cache Retention Policy

- `tools/throughput_env.sh --apply` runs `tools/molt_cache_prune.py` by default.
- Defaults:
  - External `MOLT_CACHE`: `200G` max + `30` day age pruning.
  - Local fallback: `30G` max + `30` day age pruning.
- Override with env vars before running the script:
  - `MOLT_CACHE_MAX_GB=<n>`
  - `MOLT_CACHE_MAX_AGE_DAYS=<n>`
  - `MOLT_CACHE_PRUNE=0` to skip prune.

## Optimization Plan

Long-term or complex optimizations that require research are tracked in `OPTIMIZATIONS_PLAN.md`. If your change is a major architectural shift, please update that plan first.
