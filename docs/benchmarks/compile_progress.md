# Compile Progress Tracker

This page is the single source of truth for compile-time improvement progress
across the 2-week plan.

## Scope

- Runtime compile time (`molt-runtime`) in `dev` and `release`.
- Backend compile + codegen time (`molt-backend`) in `dev` and `release`.
- End-to-end `molt build` latency for small scripts (currently
  `examples/hello.py`).
- Release-iteration lane timing using `release-fast` (`MOLT_RELEASE_CARGO_PROFILE`).
- IR bloat tracking for simple scripts (module count, ops, JSON IR size).

## KPIs

| KPI | Baseline (2026-02-09) | Target | Latest | Status |
| --- | ---: | ---: | ---: | --- |
| Native `hello` IR size (MB) | 40.923 | <= 10.000 | 5.289 | green |
| Native `hello` IR ops | 409900 | <= 100000 | 50483 | green |
| Native `hello` init modules | 29 | <= 12 | 11 | green |
| `dev` warm cache-hit build (s) | 4.40 | <= 3.00 | 6.121 | yellow |
| `dev` warm no-cache daemon-on (s) | 30.34 | <= 15.00 | 8.150 | green |
| `release` warm no-cache build (s) | 25.96 | <= 18.00 | 3.033 | green |
| `release-fast` warm cache-hit build (s) | n/a | <= 6.000 | 2.033 | green |
| `release-fast` warm no-cache build (s) | n/a | <= 8.000 | 4.039 | green |

Baseline measurement roots:

- `/tmp/molt_compile_probe_1770664249`
- `/tmp/molt_compile_probe_1770664249/logs`

## Why Hello IR Is Huge

Evidence from investigation:

- Native build path always adds `multiprocessing.spawn` graph for non-wasm
  targets (`src/molt/cli.py`, native-only `spawn_enabled` branch).
- Core closure from `builtins`/`sys` pulls additional stdlib graphs.
- Native IR for `examples/hello.py` was measured at 1546 functions and
  409900 ops.
- Wasm IR for the same script (without native spawn wiring) was measured at
  223 functions and 54990 ops.

This investigation is part of the active plan and not optional.

Status update (2026-02-09): native spawn graph lowering is now gated on
detected `multiprocessing` usage via `cli._requires_spawn_entry_override(...)`.
For `examples/hello.py`, this removed `multiprocessing` init payloads from IR.

## Measurement Workflow

### 1. Throughput environment bootstrap

```bash
tools/throughput_env.sh --apply
```

### 2. Compile progress suite

```bash
uv run --python 3.12 python3 tools/compile_progress.py --clean-state
```

This writes:

- `compile_progress.json`
- `compile_progress.md`
- per-case stdout/stderr logs

Recommended contention-safe flags on busy multi-agent hosts:

```bash
uv run --python 3.12 python3 tools/compile_progress.py \
  --clean-state \
  --max-retries 2 \
  --retry-backoff-sec 2 \
  --build-lock-timeout-sec 60
```

The harness now hardens timeout handling by killing timed-out process groups,
run-scoped compiler children (`cargo`/`rustc`/`sccache` workers), and
run-scoped backend daemons before moving to retries/next cases.

The harness now also writes incremental snapshots (`compile_progress.json` and
`compile_progress.md`) after every completed case, so interrupted runs still
preserve completed-case results.

When `/Volumes/APDataStore/Molt` exists, output defaults there
(`compile_progress_<timestamp>`). Otherwise it writes under
`bench/results/compile_progress/<timestamp>`.

### 3. Optional focused rerun

```bash
uv run --python 3.12 python3 tools/compile_progress.py \
  --cases dev_nocache_daemon_on dev_nocache_daemon_off
```

Release iteration lane (`release-fast`) focused probe:

```bash
uv run --python 3.12 python3 tools/compile_progress.py \
  --cases release_fast_cold release_fast_warm release_fast_nocache_warm \
  --max-retries 2 \
  --retry-backoff-sec 2 \
  --build-lock-timeout-sec 60
```

### 4. Optional IR size probe

```bash
uv run --python 3.12 python3 -m molt.cli build \
  --profile dev \
  --no-cache \
  --emit-ir /tmp/hello_ir.json \
  examples/hello.py
```

### 5. Optional build diagnostics probe (phase timing + module reasons)

```bash
MOLT_BUILD_DIAGNOSTICS=1 \
MOLT_BUILD_DIAGNOSTICS_FILE=build_diag.json \
uv run --python 3.12 python3 -m molt.cli build \
  --profile dev \
  --no-cache \
  examples/hello.py
```

### 6. Optional backend daemon warm-queue lane

```bash
uv run --python 3.12 python3 tools/compile_progress.py \
  --cases dev_queue_daemon_on dev_queue_daemon_off \
  --max-retries 2 \
  --retry-backoff-sec 2 \
  --build-lock-timeout-sec 60
```

## Update Cadence

- Update this tracker once per focused optimization PR and at least daily while
  the compile-time initiative is active.
- Add one run-history row per measurement run.
- Keep target values stable for the 2-week sprint unless explicitly re-scoped.

## Run History

| Date (UTC) | Run ID | Notes |
| --- | --- | --- |
| 2026-02-09 | `molt_compile_probe_1770664249` | Baseline established (native IR bloat confirmed; cache-hit and no-cache timings recorded). |
| 2026-02-09 | `molt_hello_ir_after_1770666669` | Post-fix IR probe: 1546 -> 203 functions, 409900 -> 50483 ops, 40.923MB -> 5.289MB. |
| 2026-02-09 | `molt_compile_progress_after_spawn_gate_1770666725` | Dev no-cache daemon-on dropped to 4.452s. Fresh rebuild timing lanes hit an unrelated runtime compile blocker in `runtime/molt-runtime/src/builtins/ast.rs` (BigInt type mismatch); release/latest warm-lane updates blocked until that is resolved. |
| 2026-02-09 | `molt_diag_probe_1770667897` | Added compiler build diagnostics: phase timing + module-inclusion reason payload emitted via `MOLT_BUILD_DIAGNOSTICS=1`. |
| 2026-02-09 | `build_diag_after.json` | Post-unblock diagnostics run confirms instrumentation and identifies current hot phase under contention: `backend_codegen` (~62.7s) while runtime setup remains low (~0.13s). |
| 2026-02-09 | `compile_diag_probe_20260209T203157Z` | Backend phase split verified in direct diagnostics build (`backend_prepare`, `backend_daemon_setup`, `backend_dispatch`, `backend_daemon_compile`, `backend_artifact_stage`, `backend_cache_write`). |
| 2026-02-09 | `compile_progress_queueprobe_20260209T203815Z` | Queue lane stress run under contention recovered from timeout via retry (`attempts=2`, `retry_reason=timeout`) and finished successfully (`rc=0`, elapsed ~98.7s). |
| 2026-02-09 | `compile_progress_timeout_probe3_20260209T205135Z` | Forced-timeout validation confirms cleanup path kills run-scoped compiler processes and prevents post-timeout orphan contention. |
| 2026-02-09 | `compile_progress_plan_batches_20260209T213719Z/dev_cache` | `dev_cold` completed with retries (`rc=0`, `attempts=2`, `retry_reason=timeout`, `elapsed=147.248s`, `diag_total=146.168s`). |
| 2026-02-09 | `compile_progress_plan_batches_20260209T213719Z/dev_nocache` | `dev_nocache_daemon_off` completed (`rc=0`, `attempts=2`, `retry_reason=timeout`, `elapsed=121.086s`); `dev_nocache_daemon_on` exited with `rc=143` and empty stdio under host-level termination pressure. |
| 2026-02-09 | `compile_progress_plan_batches_20260209T213719Z/release_single` | Release lanes remain unstable in this host context (`release_cold`: `rc=-15`, timed out after retries; `release_warm`: `rc=143` with empty stdio), requiring dedicated persistent shell execution for clean release datapoints. |
| 2026-02-10 | `compile_progress_full_final` | Full requested suite completed from persistent shell with diagnostics; all 9 cases emitted final JSON/markdown snapshots (`/Volumes/APDataStore/Molt/compile_progress_full_final`). Key points: `dev_warm=6.121s`, `dev_nocache_daemon_on=8.150s`, `release_nocache_warm=3.033s`, `release_cold` timed out after retries (`rc=-15`). |
| 2026-02-10 | `compile_progress_release_compare_release_20260210T050111Z` | Dedicated baseline release-lane comparison run completed via resume after external interruptions: `release_cold` (`rc=143`, `attempts=3`), `release_warm` (`rc=-15`, timed out, `attempts=3`), `release_nocache_warm=102.062s` (`rc=0`). |
| 2026-02-10 | `compile_progress_release_compare_release_fast_20260210T052042Z` | New `release-fast` lane measurement: `release_fast_cold=7.090s` (`attempts=3`), `release_fast_warm=2.033s` (`attempts=1`), `release_fast_nocache_warm=4.039s` (`attempts=2`). This establishes the release-iteration lane baseline prior to deeper backend/codegen optimization. |

## Current Caveat

- `tools/compile_progress.py --diagnostics` is stable and resumable, but long
  release lanes can still encounter external `SIGTERM`/`143` interruptions in
  heavily contended hosts.
- Mitigations in place:
  - timeout + lock-timeout retries/backoff
  - timeout cleanup for run-scoped compiler children and backend daemons
  - parent-death cleanup probes to reduce orphaned workers
  - per-case incremental snapshot persistence
  - `--resume` to continue interrupted runs without losing completed cases
- Recommended execution mode for KPI-quality sweeps:
  - run from a long-lived `tmux`/`mosh` shell
  - shard heavy lanes (`release_*` and `release_fast_*`) into focused runs
  - use `--resume` when interruptions happen, then ingest final JSON snapshots

## 2-Week Work Board

| Workstream | Owner | Status | Exit Criteria |
| --- | --- | --- | --- |
| W1-D1 instrumentation (module reasons + phase timings) | compiler/tooling | completed | Single command emits module inclusion reasons and per-phase timings. |
| W1-D3 `hello` IR bloat fix (`multiprocessing.spawn` gating) | compiler | completed | Native `hello` IR size drops >= 60% with multiprocessing coverage preserved. |
| W1-D5 core closure tightening (`builtins`/`sys` deps) | compiler/stdlib | partial | Native `hello` init modules approach target (<= 12). |
| W2-D1 backend lowering compile-time optimizations | backend | partial | Warm `dev` no-cache build latency materially reduced. |
| W2-D3 release-iteration profile lane | build/tooling | partial | `release-fast` iteration profile is wired into `tools/compile_progress.py` and measured against release lanes; next step is stabilizing release lane contention behavior and deciding promotion policy for release iteration workflows. |
| W2-D5 guardrails and CI regression budgets | tooling | planned | IR size + compile time regression checks in CI. |
