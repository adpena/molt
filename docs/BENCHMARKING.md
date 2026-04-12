# Benchmarking & Performance Gates

Molt is performance-obsessed. Every major change must be validated against our benchmark suite.

## Version Policy
Benchmarks target **Python 3.12+** semantics. Use 3.12 as the minimum baseline,
and record any 3.13/3.14 divergences in specs/tests.

## Runtime CPU Kernel Fast-Path Matrix

The hot tensor kernels in `runtime/molt-runtime/src/builtins/gpu.rs` must not
have silent performance coverage gaps. Current contract:

| Target | Fast path | Scope | Verification |
| --- | --- | --- | --- |
| `aarch64` native | NEON direct-store helpers plus scalar unaligned asm loads/stores | `linear_rows_f32`, `linear_split_last_dim_f32`, `linear_squared_relu_gate_interleaved_f32` | `cargo test -p molt-runtime gpu_ -- --nocapture` plus native workload sample |
| `x86_64` native | SSE unaligned 4-lane helpers | `linear_rows_f32`, `linear_split_last_dim_f32`, `linear_squared_relu_gate_interleaved_f32` | `cargo check -p molt-runtime --target x86_64-apple-darwin` or the relevant host target |
| `wasm32` with `simd128` | `wasm32` SIMD128 unaligned 4-lane helpers | `linear_rows_f32`, `linear_split_last_dim_f32`, `linear_squared_relu_gate_interleaved_f32` | `cargo check -p molt-runtime --target wasm32-unknown-unknown` with the SIMD-enabled target config used by the lane |
| Other targets | Scalar fallback only | Same semantics, lower throughput | Explicitly treat as a perf gap until a target-specific fast path lands |

Rules:
- Do not assume a fast path is active unless the target/feature lane above is
  actually compiled and verified.
- When adding a new target-specific kernel lane, update this matrix in the same
  change.
- If a benchmark or profile result comes from a scalar fallback lane, say so
  explicitly in the artifact note rather than implying parity with optimized
  native targets.

## Split Runtime Contract

For `--target wasm --split-runtime`, the packaging contract is:
- `output.wasm` is the raw rewritten app module emitted before split packaging.
- `app.wasm` must be smaller than `output.wasm`; it is expected to go through
  split-app deforestation (`_post_link_optimize`) plus wasm-opt when enabled.
- `molt_runtime.wasm` must be tree-shaken against the app import surface.
- If any of those assumptions stop holding, treat it as a real regression in
  the split-runtime pipeline rather than “normal wasm variance”.

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
It also records wasm import-surface metrics per benchmark
(`molt_wasm_import_count`, `molt_wasm_function_import_count`,
`molt_wasm_function_imports_per_kb`) to track call-surface density over time.

### Falcon Split-Runtime Host-Fed Benchmarks

For Falcon-OCR split-runtime validation, use the dedicated host-fed mode in
`tools/bench_wasm.py` instead of ad hoc `node wasm/run_wasm.js` invocations.
This mode records structured per-phase results and writes canonical JSON under
`bench/results/`.

```bash
# Fast, reproducible init-only proof on the real split-runtime artifact
uv run --python 3.12 python3 tools/bench_wasm.py \
  --falcon-hostfed \
  --runner node \
  --falcon-phase init_only \
  --falcon-phase-timeout-s 120 \
  --json-out bench/results/falcon_split_runtime_init_only.json

# Bounded first-token attempt with explicit timeout classification
uv run --python 3.12 python3 tools/bench_wasm.py \
  --falcon-hostfed \
  --runner node \
  --falcon-phase init_plus_1_token \
  --falcon-phase-timeout-s 10 \
  --json-out bench/results/falcon_split_runtime_token_timeout.json
```

Rules:
- Use `--falcon-phase init_only` when you need deterministic startup data without
  paying full token-generation cost.
- Use `--falcon-phase-timeout-s` for heavyweight phases so failure is explicit
  (`runner_timeout`) and the run still emits a benchmark JSON artifact.
- Successful phases capture `molt_profile_json` payloads from the wasm runtime in
  the phase record when `MOLT_PROFILE=1` / `MOLT_PROFILE_JSON=1` are enabled.
- Do not assume timed-out synchronous wasm inference can always dump a final
  profile payload on shutdown; the timeout classification is reliable, but a
  JS-side `SIGTERM` handler cannot preempt a long-running synchronous wasm call.

## Compiled GPU Kernel Backend Lanes

Compiled `@gpu.kernel` now has three distinct execution states:

- default compiled lane: runtime-owned sequential launch semantics
- explicit native Metal lane: real backend execution on macOS
- wasm lanes: correct compiled launch semantics, but still not real GPU backend dispatch

Native Metal is opt-in and must be enabled at both build time and run time:

```bash
MOLT_RUNTIME_GPU_METAL=1 \
MOLT_GPU_BACKEND=metal \
MOLT_TRACE_GPU_BACKEND=1 \
molt run --profile dev path/to/kernel_smoke.py
```

Current acceptance proof:
- [tests/test_gpu_kernel_compiled.py](/Users/adpena/Projects/molt/tests/test_gpu_kernel_compiled.py)
  - compiled native kernel correctness
  - compiled native kernel with explicit Metal backend
- [tests/test_wasm_split_runtime.py](/Users/adpena/Projects/molt/tests/test_wasm_split_runtime.py)
  - compiled split-runtime wasm kernel correctness

Rule:
- If `MOLT_GPU_BACKEND=metal` is requested and the runtime was not built with
  `MOLT_RUNTIME_GPU_METAL=1`, that is a real configuration error and must
  raise. Do not silently fall back to the sequential launcher.

```bash
# Basic run
uv run --python 3.14 python3 tools/bench.py

# One-off script (CLI wrapper or direct harness)
molt bench --script path/to/script.py
uv run --python 3.14 python3 tools/bench.py --script path/to/script.py

# Record results to JSON (standard for PRs)
uv run --python 3.14 python3 tools/bench.py --json-out bench/results/my_change.json

# Isolated dynamic-builtin micro-slices (not part of core KPI suite)
uv run --python 3.14 python3 tools/bench.py --dynamic-builtin-only \
  --json-out bench/results/dynamic_builtins.json

# Increase warmup runs (default: 1, or 0 for --smoke)
uv run --python 3.14 python3 tools/bench.py --warmup 2

# Comparison vs CPython
uv run --python 3.14 python3 tools/bench.py --compare cpython
```

## Native Baselines (Optional)

`tools/bench.py` compares Molt against optional baseline lanes using the same
benchmark scripts:

- **PyPy**: auto-probed via `uv run --python pypy@3.11` (skipped if unavailable).
- **Codon**: install `codon` and ensure it is on PATH.
- **Nuitka**: install `nuitka` (or pass `--nuitka-cmd "python -m nuitka"`).
- **Pyodide**: provide a runner prefix with `--pyodide-cmd` or `MOLT_BENCH_PYODIDE_CMD`.

Disable any baseline with `--no-pypy`, `--no-codon`, `--no-nuitka`, and
`--no-pyodide`, respectively.
Use `--no-cpython` when you want a direct Molt-vs-friend comparison lane without
paying the CPython runtime cost.
Use `--runtime-timeout-sec <seconds>` to cap per-process runtime for long suites
and keep partial runs bounded/reproducible.

## Combined Native + WASM Report

After writing the benchmark JSON artifacts, generate the canonical combined
report and refresh the `STATUS.md` benchmark block from the checked-in manifest:

```bash
uv run --python 3.14 python3 tools/bench_report.py \
  --manifest bench/results/docs_manifest.json \
  --update-status-doc
```

The manifest is the single source of truth for which benchmark artifacts feed
the published docs:
- detailed generated report: `docs/benchmarks/bench_summary.md`
- concise generated summary block: `docs/spec/STATUS.md`

Check freshness without rewriting:

```bash
uv run --python 3.14 python3 tools/bench_report.py \
  --manifest bench/results/docs_manifest.json \
  --check \
  --update-status-doc
```

README should link to status and benchmark docs, not own generated benchmark data.

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
- Use `--fail-regression-count`, `--fail-regression-pct`, and
  `--fail-regression-abs` to make regressions fail with exit code `2` in CI/swarms.
- Manual perf validation can run this gate against `bench/results/baseline.json` for
  `molt_cpython_ratio`, `molt_time_s`, and `molt_build_s`.

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

PyPerformance lane (uses `molt run --profile dev` for the Molt runner):

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --suite pyperformance_benchmarks \
  --checkout \
  --fetch
```

Artifacts:
- machine-readable: `results.json`
- human summary: `summary.md`
- published summary: `docs/benchmarks/friend_summary.md` (`--update-doc`)

Rules:
- Pin friend repos to immutable `repo_ref` values before enabling suites.
- Record compile and run phases separately when friends compile ahead of run.
- Classify cases as `runs_unmodified`, `requires_adapter`, or `unsupported_by_molt`.
- Use explicit runner lanes: `pypy`, `codon`, `nuitka`, and `pyodide`
  (`friend` is kept only as a legacy generic lane).

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

## Optimization Swarm Gate Bundle

For optimization swarm execution, each landing must attach one reproducible
gate bundle (no exceptions):

- perf delta evidence:
  - benchmark JSON + `tools/bench_diff.py` output for touched lanes
  - compile throughput evidence from `tools/compile_progress.py` and/or
    `tools/throughput_matrix.py`
- correctness evidence:
  - differential parity run with `MOLT_DIFF_MEASURE_RSS=1`
  - memory cap enforcement (`MOLT_DIFF_RLIMIT_GB=10` unless explicitly tuned)
- lowering evidence:
  - `python3 tools/check_stdlib_intrinsics.py`
  - `python3 tools/check_core_lane_lowering.py`
- documentation sync evidence:
  - same-change updates for optimization status docs
    (`docs/spec/STATUS.md`, `ROADMAP.md`,
    `OPTIMIZATIONS_PLAN.md`, `docs/benchmarks/optimization_progress.md`)

## How to Interpret Results

- **Speedup (x.xx)**: Molt is X times faster than CPython. (e.g., 10.0x = Molt is 10x faster).
- **Regression (< 1.0x)**: Molt is slower than CPython. This is generally unacceptable for Tier 0 constructs.
- **Super Bench (`--super`)**: Runs 10 samples and calculates variance. Use this for final release validation or when results are noisy.
- **Molt build vs run time**: `molt_build_s` captures compile time; `molt_time_s` is run time only for fair runtime comparisons.
- **WASM build vs run time**: `molt_wasm_build_s` captures wasm compile time; `molt_wasm_time_s` is run time only.
- **WASM import density**: use `molt_wasm_function_imports_per_kb` and related
  import-count fields to monitor runtime call-surface pressure.

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
- Defaults use `MOLT_EXT_ROOT` when set; otherwise the tooling falls back to
  canonical repo-local artifact roots.
- Throughput bootstrap defaults `CARGO_INCREMENTAL=0` to maximize cacheability/shared throughput under multi-agent contention. Set `CARGO_INCREMENTAL=1` only for local incremental-debug sessions.
- Prefer `molt build --build-profile dev` for build-only iteration loops, and `--profile dev` for `molt run/compare/diff/test`; reserve release profiles for release gates and perf publication.
- `--build-profile dev` routes build mode to Cargo `dev` by default; override with `MOLT_DEV_CARGO_PROFILE` when profiling alternative dev profiles.
- Keep cache keys deterministic by default (`PYTHONHASHSEED=0` is enforced by CLI). Override via `MOLT_HASH_SEED=<value>` only when explicitly testing hash-seed sensitivity.
- Enable Rust compile caching:
  - `MOLT_USE_SCCACHE=1` (or leave default `auto` when `sccache` is installed)
  - `sccache -s` to inspect hit rates
- Keep backend daemon enabled for native compile loops (`MOLT_BACKEND_DAEMON=1`; default) so Cranelift initialization is amortized across builds.
- In multi-agent runs, share cache/target roots under one artifact root to improve reuse:
  - `MOLT_EXT_ROOT=/path/to/artifacts`
  - `MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache`
  - `CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target`
- Keep diff runs on the same shared target:
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` (set automatically by `tools/throughput_env.sh --apply`)
- For differential throughput, wrappers are disabled by default for portability; opt in only on stable hosts:
  - `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`

### Suggested Throughput Baseline Command

```bash
MOLT_CACHE=$PWD/.molt_cache \
CARGO_TARGET_DIR=$PWD/target \
MOLT_USE_SCCACHE=1 \
uv run --python 3.12 python3 -m molt.cli build examples/hello.py --build-profile dev --cache-report
```

### Throughput Matrix Harness

Use the dedicated matrix harness to compare single-agent vs concurrent throughput
across profile and wrapper modes:

```bash
uv run --python 3.12 python3 tools/throughput_matrix.py \
  --concurrency 2 \
  --timeout-sec 75 \
  --shared-target-dir "$PWD/target" \
  --run-diff \
  --diff-jobs 2 \
  --diff-timeout-sec 180
```

- Results are written to `matrix_results.json` under the chosen output root.
- Results include a machine-readable `gate_status` block (thresholds, observed
  counts, violations, pass/fail).
- Use `--fail-on-gate` to return exit code `2` when `gate_status.passed=false`.
- Default output root uses `MOLT_EXT_ROOT` when set, otherwise canonical repo-local roots.
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
  - `--diagnostics`
  - `--diagnostics-file <path>`
  - Example:
    `uv run --python 3.12 python3 -m molt.cli build --build-profile dev --no-cache --diagnostics --diagnostics-file build_diag.json examples/hello.py`
  - Midend payloads include tiering telemetry (`tier_base_summary`,
    `promoted_functions`, `promotion_source_summary`,
    `promotion_hotspots_top`) for PGO-guided tier promotion audits.
  - Disable hot-function tier promotion explicitly with
    `MOLT_MIDEND_HOT_TIER_PROMOTION=0` when doing controlled A/B pass studies.
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
  - `$MOLT_EXT_ROOT/compile_progress_<timestamp>` when `MOLT_EXT_ROOT` is set
  - otherwise use a canonical repo-local output root via `--output-root`
- Progress board and KPI targets live in
  `docs/benchmarks/compile_progress.md`.

### Cache Retention Policy

- `tools/throughput_env.sh --apply` runs `tools/molt_cache_prune.py` by default.
- Defaults:
  - `MOLT_CACHE`: `200G` max + `30` day age pruning.
- Override with env vars before running the script:
  - `MOLT_CACHE_MAX_GB=<n>`
  - `MOLT_CACHE_MAX_AGE_DAYS=<n>`
  - `MOLT_CACHE_PRUNE=0` to skip prune.

## Optimization Plan

Long-term or complex optimizations that require research are tracked in `OPTIMIZATIONS_PLAN.md`. If your change is a major architectural shift, please update that plan first.
