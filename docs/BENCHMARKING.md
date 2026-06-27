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

The citable release performance authority is the canonical scoreboard, not the
developer harness:

```bash
uv run --python 3.12 python3 tools/perf_scoreboard.py \
  --set core --backend native --backend llvm --profile release-fast \
  --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent
```

It is mirrored by `.github/workflows/perf-gate.yml`. Use `tools/bench.py` for
native triage and `tools/bench_wasm.py` for WASM triage; their JSON and Markdown
outputs are non-canonical evidence and must not be cited as PR/release
performance authority.
To exercise single-module linking, add `--linked` (requires `wasm-ld` and
`wasm-tools`).
Use `tools/bench_individual.py` for focused native micro-benchmark slices. It
reuses the backend daemon by default so build timings represent the normal
warm developer path; pass `--isolate-daemon` only when explicitly measuring
cold daemon startup or investigating daemon crash isolation. Isolated cleanup is
scoped to the current `MOLT_SESSION_ID` or explicit backend socket, preserves
foreign-session daemons, and records daemon custody events in
`tmp/bench/daemon_custody.jsonl`.
`tools/bench.py` and `tools/bench_wasm.py` also prune backend daemons only
through `src/molt/backend_daemon_custody.py` after canonicalizing their session
environment, so concurrent benchmark agents keep their own warm daemon/cache
state instead of matching loose sockets or PIDs.
Both runners reuse Molt build caches by default. Pass
`--no-molt-build-cache` only for an intentional cold/no-cache rebuild study; it
is not the default throughput mode.
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

Native WebGPU is also opt-in:

```bash
MOLT_RUNTIME_GPU_WEBGPU=1 \
MOLT_GPU_BACKEND=webgpu \
MOLT_TRACE_GPU_BACKEND=1 \
molt run --profile dev path/to/kernel_smoke.py
```

Current native compiled-kernel backend proofs:
- Metal: real backend dispatch on macOS
- WebGPU: real `wgpu` backend dispatch on native desktop
- WASM split-runtime/browser host: real browser-host WebGPU dispatch is available
  through `wasm/browser_host.js` when `MOLT_GPU_BACKEND=webgpu` is present in the
  WASI env and the host provides either:
  - `gpuKernelDispatcher`, or
  - the default worker-backed WebGPU dispatcher (`wasm/browser_gpu_worker.js`)

Browser WebGPU rules:
- The browser host contract is worker-oriented. A synchronous compiled kernel
  launch cannot run on the page main thread and still wait for WebGPU readback
  correctly.
- `loadMoltWasm({...})` may inject `env` entries into WASI boot, which is the
  canonical way to request `MOLT_GPU_BACKEND=webgpu` on browser-hosted wasm.
- Deterministic host proof:
  - [tests/test_wasm_browser_gpu_host.py](/Users/adpena/Projects/molt/tests/test_wasm_browser_gpu_host.py)
    - compiled browser-host wasm kernel correctness
    - browser-host WebGPU dispatcher usage

```bash
# Basic run
uv run --python 3.14 python3 tools/bench.py

# One-off script (CLI wrapper or direct harness)
molt bench --script path/to/script.py
uv run --python 3.14 python3 tools/bench.py --script path/to/script.py

# Record triage results to JSON (not PR/release authority)
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

Native benchmark JSON keeps raw timing arrays (`*_samples_s`) alongside the
existing mean fields. A lane only contributes samples when every measured run
succeeds; partial failures leave the mean null and the sample array empty for
that attempted lane. When CPython is enabled, Molt native timings are also gated
by exact stdout/stderr parity against the stable CPython sample output. The
artifact records hash-only `molt_output_parity` evidence rather than raw output,
sets `molt_ok=false`, nulls Molt speedup/ratio fields on mismatch, writes the
JSON evidence, and exits nonzero before any baseline update.

Native Molt rows carry the same phase-aware failure contract used by friend
suite Molt runners. `molt_status` is `pass` for clean rows; failed rows set
`molt_failure_phase` (`build`, `run`, or `parity`), `molt_failure_status`, and a
nested `molt_failure` object with `detail`, `message`, `returncode`,
`timed_out`, `elapsed_s`, `signal`, `guard_violation`, and cleaned orphan
process groups. Guard-owned failures retain canonical statuses such as
`timeout`, `signal_exit`, and `rss_limit_exceeded`; Molt-specific stderr
signatures refine non-guard failures into details such as
`backend_daemon_empty_response`, `backend_daemon_died_in_flight`, and
`molt_runtime_invalid_object_header_before_dec_ref`. A `molt.cli run` failure
that reports backend-daemon compile failure is classified as build phase even
when the enclosing friend-suite command was a run command.
`tools/bench.py` also emits a sibling Markdown summary plus bounded
`molt_failure_details` in `results.json` and
`molt_failure_details.jsonl`/`*_molt_failure_details.jsonl`. The JSON
`custody_artifacts` block references the summary, failure-detail sidecar,
guard command profile, repo-process sentinel, and backend-daemon cleanup JSONL
so daemon crashes and RSS-guard kills remain evidence-producing runs.

Hot-only `tools/perf_scoreboard.py --sample-hot-only` receipts are attribution
evidence, not speedup gates. If the looped profiling binary fails during the
size phase, the JSON refusal now preserves bounded `size_stdout_tail` and
`size_stderr_tail` fields plus `size_status`/`size_exit_code`; use those fields
to diagnose the failed binary before rerunning or moving any performance claim.

Benchmarks that directly exercise Molt runtime intrinsics without an external
reference implementation are explicitly Molt-only in `tools/bench_metadata.py`.
Those rows must record `reference_runtime="molt"` and skip CPython/PyPy/Codon/
Nuitka/Pyodide baseline lanes instead of relying on host-Python fallbacks.
When an external reference script exists, update that metadata in the same
change that adds the reference lane.

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
- Runtime-gated metrics are comparable only when the matching `*_ok` gate is
  true in both artifacts. Failed native Molt rows do not contribute
  `molt_time_s`, speedup, or ratio diffs; failed WASM rows do not contribute
  `molt_wasm_time_s` diffs.
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

Upstream tinygrad lane:

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --suite tinygrad_off_the_shelf \
  --dry-run
```

To run against an already-pinned local checkout without editing the manifest,
override both the suite root and the expected ref:

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --suite tinygrad_off_the_shelf \
  --suite-root tinygrad_off_the_shelf=/path/to/tinygrad \
  --repo-ref tinygrad_off_the_shelf=<commit-sha> \
  --no-checkout
```

The manifest pins `tinygrad_off_the_shelf` to immutable upstream commit
`a83710396c991272241e40da94489747c2393851` and enables the suite for the real
CPython adapter plus upstream-owned tinygrad contract. The `tinygrad` runner
executes `CHECK_OOB=0 DEV=CPU TYPED=1 python test/test_tiny.py` through
`uv run --isolated --no-project --with typeguard` because the pinned upstream
package imports its typeguard hook at module import time; runner-local
`PYTHONPATH={suite_root}` and `PYTHONDONTWRITEBYTECODE=1` let
`test/test_tiny.py` import the checked-out package without installing or
modifying it, so source custody stays clean.
Suite-wide `XDG_CACHE_HOME` and `CACHEDB` resolve under the run `output_root`,
so upstream tinygrad cache writes are evidence artifacts rather than pinned
checkout mutations. The
CPython runner executes
`tools/tinygrad_off_shelf_adapter.py` against the checked-out upstream package
through the same isolated no-project `typeguard` dependency and bytecode-write
ban; the adapter is only a public-API workload driver. The Molt runner is executable by
default and uses the project interpreter token
(`{project_python} -m molt.cli run`) while forwarding the required full-stdlib
build profile with `--build-arg=--stdlib-profile --build-arg=full`; do not
replace it with ambient `molt` from `PATH` or a micro-profile build.
`{python}` remains the harness/base interpreter token for isolated friend
commands such as `uv run --python {python}`; Molt CLI runners use
`{project_python}` so repo-installed dependencies are present under
`PYTHONNOUSERSITE=1`. Earlier guarded evidence reached
`molt-backend --daemon` and then tripped the process RSS guard at 12.005 GB after
435.5s (`tmp/memory_guard/friends_tinygrad_molt_sqlite_profile.json`), proving
that blocker was backend-daemon compile memory before adapter workload execution.
Native TIR optimization now processes uncached user functions in bounded
op/count batches and applies/cache-writes each batch immediately; the next Molt
runner proof (`bench/results/friends/20260612T184515Z/`) reached that bounded
path and reduced single-backend RSS before failing on aggregate process-tree RSS
from overlapping daemon plus hidden one-shot fallback. CLI daemon failure now fails closed after
full-request admission instead of restarting the daemon or launching that hidden
second backend compile. The follow-up Molt runner proof
(`bench/results/friends/20260612T203111Z/`,
`tmp/memory_guard/friends_tinygrad_molt_daemon_custody.json`) no longer trips
the outer memory guard (`violation=null`, no orphaned process groups, 4.92 GB
peak process-tree RSS); it fails closed with `Backend daemon compile failed:
backend daemon died while request was in flight`. A later guarded Molt-only rerun
(`bench/results/friends/20260612T205850Z/`) used 10 GB process / 18 GB aggregate
bench caps, recorded no memory violation, preserved host/control-plane process
groups in `memory_guard/bench_friends_sentinel.jsonl`, and failed after 208.19s
with `Backend daemon compile failed: backend daemon returned empty response`.
The 21:12 guard sidecar
(`tmp/memory_guard/friends_tinygrad_molt_daemon_harness_custody.json`) records a
separate daemon compile-memory event: the suite sentinel terminated only the
Molt-owned `molt-backend --daemon` process group when the daemon reached the 12
GB process RSS cap. Native application-object batching now consumes the same
`MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching, and the production
self-spawn worker path is covered by
`cargo test -p molt-backend --test native_batch_worker_spawn`
(`tmp/memory_guard/cargo_test_native_batch_worker_spawn_cleanup_diag_20260615.json`):
the real `molt-backend` binary compiles two live functions as two materialized
batches through `--native-batch-job-file`. Daemon-off proof
now builds the full-stdlib adapter and reaches runtime execution under guard;
a 2026-06-15 list-workloads smoke
(`tmp/memory_guard/tinygrad_importlib_module_from_spec_smoke.json`) timed out
after 900s with `violation=null`, no orphaned process groups, 3.75 GB peak
process-tree RSS, and Cargo incremental quarantine while compiling the
full-stdlib tinygrad adapter. The active backend IR was 49 MB with 5,845
functions and 866,671 ops, so this is cold build/compiler-throughput evidence
before adapter workload enumeration, not a tinygrad semantic failure. Direct
guarded backend replays of that IR
(`tmp/memory_guard/tinygrad_backend_replay_indexed_20260615.json` and
`tmp/memory_guard/tinygrad_backend_replay_indexed_scratch_20260615.json`) both
detected 1,469 leaf functions and failed closed before object emission because
`MOLT_RUNTIME_INTRINSIC_SYMBOLS` was absent; their 0.891 GB and 0.887 GB peak
RSS receipts are backend compile-memory evidence only. A later lazy-index
guarded list-workloads retry
(`tmp/memory_guard/tinygrad_adapter_list_workloads_lazy_index_20260615.json`)
still timed out in the full-stdlib adapter build after 1200s with
`violation=null`, no orphaned process groups, 1.34 GB peak process RSS, and
2.28 GB peak process-tree RSS; the post-run sentinel receipt
(`tmp/memory_guard/process_sentinel_after_lazy_index_20260615.json`) returned 0
with no incident or orphaned process groups. The older 1.985 GB invalid-header
receipt is now historical after the importlib bootstrap export, list-clear
detach, namedtuple return-boundary ownership, defaultdict factory-handle
ownership, and Python-origin carrier cleanup fixes. Direct rebuilt-adapter
evidence covered the then-four default public-API workloads.
The current CPython adapter source now enumerates five default public-API
workloads, including `attention_core`, and the pinned upstream CPython probe
exits cleanly for all five. The official `tinygrad_off_the_shelf` Molt friend
runner with clean pinned source custody reached upstream tinygrad's lazy pattern
compiler at `tinygrad/uop/upat.py:167`, where `upat_compile` calls
`exec(code_str, globs, namespace)`. Unrestricted `exec()` is outside Molt's
verified AOT subset; historical artifact:
`bench/results/friends/2026-06-20-tinygrad-origin-fix-rerun/`. The friend
manifest now runs `tools/tinygrad_upat_static_exec_registry.py` as a prepare
step, writes the generated `_molt_tinygrad_upat_static_exec_registry` module
under the run output root, admits that module beside upstream `tinygrad` in the
Molt static-package lane, and configures
`tools/tinygrad_off_shelf_adapter.py` to install the registry as the
package-scoped `tinygrad.uop.upat.exec` global. Unknown matcher source strings
still fail closed through the generated registry. Fresh 2026-06-23 guarded
evidence (`bench/results/friends/20260623T131504Z-tinygrad-molt-fixed-env/`)
now materializes the static registry, preserves clean pinned source custody,
builds and links the Molt native binary under the sanitized Windows harness
environment, and fails at runtime with `TypeError: 'str' object is not
callable` from `<molt-builtin>` line 12. Do not treat `UPAT_COMPILE=0` as a
completion bypass. A
source-custody CPython probe of the pinned `attention_core` workload with
`UPAT_COMPILE=0` also returned 1 before Molt was involved: it got past
`upat_compile` but failed in upstream tinygrad's interpreted matcher with
`NameError: name 'do_substitute' is not defined` from
`tinygrad/codegen/simplify.py:57` via `tinygrad/uop/ops.py:1346`
`universal_match`. Therefore `UPAT_COMPILE=0` is not a usable Molt diagnostic
lane for this pinned attention workload and must not be used as completion
evidence. Do not patch, vendor, or translate tinygrad sources for this lane.
Its output is intended to drive GPU primitive, typed runtime upload/readback,
MLIR/MIL lowering, and profiler work.

NumPy off-the-shelf lane:

```bash
uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --suite numpy_off_the_shelf \
  --dry-run
```

`numpy_off_the_shelf` is enabled and pinned to upstream NumPy commit
`c81c49f77451340651a751e76bca607d85e4fd55` (the peeled `v2.4.2` commit). The
`source_audit` runner verifies pinned source-tree custody, the `cpython` runner
executes an isolated `numpy==2.4.2` baseline through
`tools/numpy_off_shelf_adapter.py`, the `c_api_scan` runner executes
`molt extension scan --source {suite_root}/numpy --fail-on-missing`, and the
`molt` runner attempts the same public workloads through
`MOLT_EXTERNAL_STATIC_PACKAGES=numpy`, explicit `module.extension.exec`
capability, and all-loaded-`numpy.*` module-origin custody. Its purpose is to
prove off-the-shelf NumPy source compile/import through Molt-owned headers,
source-recompiled native extension package build/staging/runtime custody, and
NumPy C-API symbol closure. Build admission already fails closed for admitted
package-local `.so`/`.pyd` artifacts without valid sidecar manifests and
fingerprints artifact/manifest hashes in graph, wrapper, and backend
object-cache inputs; native builds also publish validated artifacts, sidecars,
package `__init__.py` files, and runtime shim candidates under a deterministic
`external_static_packages/<plan-digest>/` runtime root and inject that staged
root into generated native binaries before runtime startup. The lane remains red
until the source-recompiled package build, NumPy C-API closure, and no-host NumPy
runtime load proof pass.
Do not satisfy it through CPython wheel fallback, ambient host Python imports,
or patched NumPy sources.
`molt extension scan` reports each required C-API token as `runtime_backed`,
`source_compile_only`, `fail_fast`, or `missing`; `--fail-on-missing` fails for
both missing and fail-fast symbols so header stubs cannot overclaim NumPy C-API
support.
For `source = "git"` suites, `tools/bench_friends.py` now records source
custody in `results.json`: requested ref, resolved commit, checked-out `HEAD`,
ref verification, clean-tree status, and whether `--suite-root` overrode the
manifest checkout path. Git custody is checked both before admission and after
suite execution; a mismatch, dirty checkout, ignored checkout artifact, or
runner-created source-tree artifact is a hard failure. Runners that declare
`json_stdout = true` must emit valid JSON; the harness preserves the raw
payloads and folds per-workload `elapsed_s` values into runner
`structured_median_s` fields plus flattened suite metrics only for runners with
`role = "workload"`. Custody and scan roles remain suite-failing when non-green,
but never feed speedup math.

Artifacts:
- machine-readable: `results.json`
- human summary: `summary.md`
- bounded Molt failure index: `molt_failure_details.jsonl` and the
  `molt_failure_details` block in `results.json`
- published summary: `docs/benchmarks/friend_summary.md` (`--update-doc`)
- memory custody: `memory_guard/bench_friends_sentinel.jsonl`,
  `memory_guard/backend_daemon_cleanup.jsonl`, and serialized
  `memory_guard_incidents` in `results.json`
- daemon/log custody references: the `custody_artifacts` block in
  `results.json` points at the guard command profile, sentinel JSONL, backend
  daemon cleanup JSONL, summary, and failure-detail sidecar.

Rules:
- Pin friend repos to immutable `repo_ref` values before enabling suites.
- Clean stale friend checkout caches with `molt clean --apply` or
  `tools/dev.py clean-artifacts --apply`; `bench/friends/repos/` is canonical
  ignored artifact state, not durable evidence.
- Record compile and run phases separately when friends compile ahead of run.
- Classify cases as `runs_unmodified`, `requires_adapter`, or `unsupported_by_molt`.
- Use explicit runner lanes (`pypy`, `codon`, `nuitka`, `pyodide`, `tinygrad`,
  `numpy`, or another manifest-declared runner name); invalid runner names are
  rejected rather than silently ignored. `friend` is kept only as a legacy
  generic lane.
- Treat interrupted runs and RSS sentinel trips as evidence-producing runs when
  the suite can finalize an artifact; emergency-writer tests cover partial
  JSON/markdown snapshots, while real sentinel trips may leave only bounded
  sidecar receipts plus identity-verified backend-daemon cleanup logs. Guarded
  commands with `--summary-json` write a `status: "running"` summary before child
  launch, so a hard-killed guard parent still leaves repro command, resolved
  limits, guard process identity, and host/control-plane samples at the requested
  summary path.

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

### Cold start vs steady state

The throughput table (`tools/bench.py`) reports **warm steady-state**: each
runtime is warmed (`--warmup`, default 1) on the exact artifact path the
measured runs reuse, so on macOS the warmup primes the `amfid`/provenance cache
for the binary's cdhash and the page cache for the same bytes. The `MoltRun`
number is therefore post-warmup, same-cdhash, and `bench.py` now records
**mean (headline) + min (best-achievable) + variance** for every runtime
unconditionally (hyperfine norm), not only under `--super`. Best-case
regressions are visible in `runtime_stats` without a special flag.

**Cold start and binary size** come from the dedicated matrix tool
`tools/output_startup_size_audit.py`, never from the throughput table (mixing a
cold molt launch against a warm CPython launch would be unfair). Per matrix case
(`native`/`wasm`/`luau` × profile × backend) it records:

- `same_path`: warm steady-state startup (provenance + page cache warm).
- `page_cache_cold`: a fresh-copy load. NOTE: `shutil.copy2` preserves the bytes
  and thus the **cdhash**, so on macOS this is page-cache-cold but
  provenance-WARM — it does not pay the amfid tax.
- `cold_first_sighting`: the genuine TRUE-cold sample — the single first run of a
  freshly built artifact, recorded before any other run touches its cdhash, so
  it pays the one-time `amfid`/`syspolicyd` validation tax (~100-280ms on macOS,
  OS-side/IPC-bound, 0 CPU, cached per cdhash thereafter). This is only genuinely
  cold when the build did not prime the binary; the audit invokes `molt build`
  without `--prime` (off by default for ordinary builds), so the artifact is
  unprimed.
- process baselines: `cpython` and a tiny `c_baseline` for reference.

The one-time macOS amfid tax is unavoidable on the first launch of a given
cdhash; it can be moved to build time with `molt build --prime`. Notarization
fixes Gatekeeper *trust*, not launch *speed*.

Run the audit and feed it into the report:

```bash
uv run --python 3.12 python3 tools/uv_project_env.py \
  --python 3.14 --purpose output-startup-size -- \
  uv run --python 3.14 python3 tools/output_startup_size_audit.py \
  --out-dir bench/results --json
uv run --python 3.12 python3 tools/uv_project_env.py \
  --python 3.14 --purpose bench-report -- \
  uv run --python 3.14 python3 tools/bench_report.py \
  --manifest bench/results/docs_manifest.json \
  --startup-audit bench/results/output_startup_size_audit.json \
  --update-status-doc
```

`bench_report.py` renders a **Cold Start & Binary Size** section (warm min/median,
page-cache-cold, cold first-sighting, CPython/C baselines, budget verdict) and
emits a one-line `Startup/size:` summary into the `STATUS.md` generated block.

Sources for the macOS amfid/provenance facts: The Eclectic Light Company,
"Checking code can take longer now" (first-launch ~0.192s Ventura / ~0.303s
Sequoia, warm amfid ~0.003s once hashes are cached); hyperfine reports
Mean/Min/Max/σ by default and uses `--warmup` to prime caches.

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
  - default adaptive process/tree/global RSS guard plus adaptive child rlimit
  - memory-guard teardown must stay scoped to the guarded root process group
    plus exact tracked escaped PIDs; never widen a violation into killpg of
    child-reported process groups
  - guard-trip diagnostics must include incident-only repro context without
    writing success artifacts by default
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
- **MoltRun = warm steady-state**: the run-time number is measured after warmup on
  the same artifact path (same cdhash), so it excludes the one-time macOS amfid
  cold-start tax. Cold-start is reported separately by the startup/size audit
  (see "Cold start vs steady state"), never folded into this throughput number.
- **min + σ are always recorded**: every runtime's `runtime_stats` carries
  `mean_s` (headline), `min_s` (best-achievable, hyperfine norm), and
  `variance_s`, regardless of `--super`, so best-case regressions surface without
  a special flag.
- **Super Bench (`--super`)**: Runs 10 samples and additionally emits
  `super_stats` (raw successful sample arrays + variance) for the verbose table.
  Use this for final release validation or when results are noisy.
- **Molt build vs run time**: `molt_build_s` captures compile time; `molt_time_s` is run time only for fair runtime comparisons.
- **WASM build vs run time**: `molt_wasm_build_s` captures wasm compile time; `molt_wasm_time_s` is run time only and is `null` unless every measured WASM sample succeeds with a positive finite duration.
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
- `molt_profile ...` (text) and `molt_profile_json {...}` (JSON) are emitted on
  the runtime diagnostics channel — stderr by default, so the profiler can scrape
  them from a captured stderr log as shown above.
- The diagnostics channel is out-of-band: it carries the `molt_profile*` lines
  and the `MOLT_ASSERT_NO_LEAK` leak report, neither of which is program output.
  Set `MOLT_DIAGNOSTICS_FILE=<path>` to redirect the whole channel to a file
  instead of stderr. The differential harness (`tests/molt_diff.py`) sets this
  per run so runtime diagnostics never pollute the stderr it compares for
  exception-signature parity — this is what lets the `MOLT_ASSERT_NO_LEAK`
  memory-safety profile and the differential parity gate run together. The
  standalone `safe_run.py` leak-assert workflow leaves it unset, keeping the leak
  report on stderr for the developer.
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
  - `uv run --python 3.12 python -m molt.cli dx env`
  - or run the command directly under the same facts:
    `uv run --python 3.12 python -m molt.cli dx run -- <command>`
  - shell activation remains available with
    `uv run --python 3.12 python -m molt.cli dx env --format posix`
    or `--format powershell`; `tools/throughput_env.sh --apply` is a POSIX
    compatibility wrapper over that resolver and still runs cache prune policy.
- Defaults preserve explicit root env vars. When external artifacts are
  preferred, the tooling selects the first healthy configured external root
  (default order `/Volumes/VertigoDataTier/Molt`, then
  `/Volumes/APDataStore/Molt`); otherwise it falls back to canonical repo-local
  artifact roots.
- `tools/bench.py` treats explicit canonical artifact env vars as authoritative
  after conformance setup. `MOLT_EXT_ROOT`, `CARGO_TARGET_DIR`,
  `MOLT_DIFF_CARGO_TARGET_DIR`, `MOLT_CACHE`, `MOLT_DIFF_ROOT`,
  `MOLT_DIFF_TMPDIR`, `UV_CACHE_DIR`, `TMPDIR`, `CARGO_INCREMENTAL`, and
  `MOLT_SESSION_ID` are preserved independently when set; only unset keys are
  derived from the selected artifact root.
- Throughput bootstrap defaults `CARGO_INCREMENTAL=0` to maximize cacheability/shared throughput under multi-agent contention. Set `CARGO_INCREMENTAL=1` only for local incremental-debug sessions.
- Prefer `molt build --build-profile dev` for build-only iteration loops, and `--profile dev` for `molt run/compare/diff/test`; reserve release profiles for release gates and perf publication.
- `--build-profile dev` routes build mode to Cargo `dev` by default; override with `MOLT_DEV_CARGO_PROFILE` when profiling alternative dev profiles.
- Keep cache keys deterministic by default (`PYTHONHASHSEED=0` is enforced by CLI). Override via `MOLT_HASH_SEED=<value>` only when explicitly testing hash-seed sensitivity.
- Enable Rust compile caching:
  - `MOLT_USE_SCCACHE=1` (or leave default `auto` when `sccache` is installed)
  - `sccache -s` to inspect hit rates
- Keep backend daemon enabled for native compile loops (`MOLT_BACKEND_DAEMON=1`; default) so Cranelift initialization is amortized across builds.
- `tools/bench.py` and `tools/bench_wasm.py` pass cache-enabled Molt builds by
  default so benchmark sweeps reuse validated frontend/backend/runtime cache
  entries across concurrent agents. Use `--no-molt-build-cache` only when the
  measurement explicitly requires a no-cache compile.
- Shared cache entries are key-addressed and immutable on benchmark hot paths:
  backend rebuilds select new keys instead of deleting old shared stdlib/module
  objects, same-key compile publication is serialized by the resolved cache
  root's `locks/` directory, Cargo/runtime rebuild locks are shared by resolved
  build-state root, and persisted JSON/text/byte/file/archive cache, state,
  diagnostics, deployment, and package sidecars publish through unique atomic
  temp siblings. WASM runtime rebuilds use Cargo-reported artifact provenance
  plus exact `artifact_sha256` sidecars before hydrating candidate `.wasm`/`.a`
  bytes, so concurrent warm target roots keep old candidates unless Cargo
  reports them as the artifact for the current invocation or a byte-digest
  sidecar proves reuse.
- In multi-agent runs, share cache/target roots under one artifact root to improve reuse:
  - `MOLT_EXT_ROOT=/path/to/artifacts`
  - `MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache`
  - `CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target`
- Keep diff runs on the same shared target:
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` (set automatically by
    `molt dx env` / `molt dx run`)
- One-file import/stdlib regression loops should use the first-class diff
  stdlib-profile flag and the persistent diff cache root:
  - `uv run --python 3.12 python3 -u tests/molt_diff.py --jobs 1 --stdlib-profile full --json tests/differential/stdlib/importlib_import_module_basic.py`
- For differential throughput, wrappers are disabled by default for portability; opt in only on stable hosts:
  - `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`

### Suggested Throughput Baseline Command

```bash
export MOLT_SESSION_ID="bench-baseline"
eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"
MOLT_USE_SCCACHE=1 \
uv run --python 3.12 python3 -m molt.cli build examples/hello.py --build-profile dev --cache-report
```

### Throughput Matrix Harness

Use the dedicated matrix harness to compare single-agent vs concurrent throughput
across profile and wrapper modes:

```bash
export MOLT_SESSION_ID="throughput-matrix"
eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"
uv run --python 3.12 python3 tools/throughput_matrix.py \
  --concurrency 2 \
  --timeout-sec 75 \
  --shared-target-dir "$CARGO_TARGET_DIR" \
  --run-diff \
  --diff-jobs 2 \
  --diff-timeout-sec 180
```

- Results are written to `matrix_results.json` under the chosen output root.
- Results include a machine-readable `gate_status` block (thresholds, observed
  counts, violations, pass/fail).
- Use `--fail-on-gate` to return exit code `2` when `gate_status.passed=false`.
- Default output root uses the active DX artifact root. Repo-local `$PWD`
  examples are local fallback/user-default examples, not heavy proof-lane
  guidance on constrained internal disks.
- Diff matrix runs always set `MOLT_DIFF_MEASURE_RSS=1` and inherit the adaptive
  child rlimit from `tests/molt_diff.py`; pass `--diff-child-rlimit-gb <n>`
  only for a deliberate narrower-cap investigation.
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
  - timed-out attempts perform marker-scoped compiler cleanup before retrying
    (`cargo`/`rustc`/`sccache` children only); backend daemons remain under
    identity custody so retry policy does not destroy warm concurrent state
  - `SIGTERM` exits (`rc=143`/`rc=-15`) are classified as retryable
  - add `--resume` for persistent-shell reruns so interrupted sweeps continue
    from already completed cases
- Default output root:
  - `$MOLT_EXT_ROOT/compile_progress_<timestamp>` when `MOLT_EXT_ROOT` is set
  - otherwise use a canonical repo-local output root via `--output-root`
- Progress board and KPI targets live in
  `docs/benchmarks/compile_progress.md`.

### Cross-Layer Analysis Capsule

Use `tools/analysis_capsule.py` when a performance or correctness question must
bridge frontend parsing/module closure, IR/TIR pass telemetry, compiler
allocation diagnostics, and final binary/startup evidence. Existing tools remain
the producers; the capsule is the coupled schema and cross-check gate.

```bash
uv run --python 3.12 python tools/analysis_capsule.py \
  --build-diagnostics bench/results/build_diag.json \
  --binary-size-json bench/results/binary_size.json \
  --startup-audit bench/results/output_startup_size_audit.json \
  --tir-fact-graph bench/results/fact_graph_app_main.json \
  --out bench/results/analysis_capsule.json
```

The capsule fails closed when the layers contradict each other, for example when
`compile_modules` contains a module outside the admitted binary-image
`known_modules` closure. Its TIR fact-graph summary surfaces schema-v3
`source_site_value_count`, file-qualified `source_site` records when frontend
source identity is available, and `allocation_ownership_fact_count`; its binary
image allocation/refcount summary consumes the generated `op_kinds.toml`
categories after frontend `borrow`/`release` aliases canonicalize to
`inc_ref`/`dec_ref`. Treat this as the preferred handoff artifact before turning
local AST, IR/TIR, allocation, or binary observations into roadmap or
performance claims.

### Cache Retention Policy

- `tools/throughput_env.sh --apply` runs `tools/molt_cache_prune.py` by default.
- Defaults:
  - `MOLT_CACHE`: `30G` max + `30` day age pruning.
- Override with env vars before running the script:
  - `MOLT_CACHE_MAX_GB=<n>`
  - `MOLT_CACHE_MAX_AGE_DAYS=<n>`
  - `MOLT_CACHE_PRUNE=0` to skip prune.

## Optimization Plan

Long-term or complex optimizations that require research are tracked in `OPTIMIZATIONS_PLAN.md`. If your change is a major architectural shift, please update that plan first.
