# Operations Guide

This document consolidates remote access, logging, progress reporting, and
multi-agent workflow rules.

## Version Policy
Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11
compatibility. If 3.12/3.13/3.14 differ, document the chosen target in specs/tests.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.

## Differential Suite (Operational Controls)
- **Memory profiling**: set `MOLT_DIFF_MEASURE_RSS=1` to collect per-test RSS metrics.
- **Build profile default**: diff harness defaults to `--build-profile dev` (override with `--build-profile release` or `MOLT_DIFF_BUILD_PROFILE=release` for release validation).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) records jobs, limits, and RSS aggregates.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `MOLT_DIFF_FAILURES` or `--failures-output`).
- **OOM retry**: OOM failures retry once with `--jobs 1` by default (`MOLT_DIFF_RETRY_OOM=0` disables).
- **Memory caps**: default 10 GB per-process; override with `MOLT_DIFF_RLIMIT_GB`/`MOLT_DIFF_RLIMIT_MB` or disable with `MOLT_DIFF_RLIMIT_GB=0`.
- **Wrapper policy**: diff runs disable `RUSTC_WRAPPER`/`sccache` by default for portability. Opt in with `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` on hosts where wrapper caches are known-good.

## Build Throughput (Multi-Agent)
- **Stable cache keys**: the CLI enforces `PYTHONHASHSEED=0` by default to keep IR/cache keys deterministic across invocations.
- **Hash-seed override**: set `MOLT_HASH_SEED=<value>` to override; set `MOLT_HASH_SEED=random` to opt out.
- **Cache invalidation scope**: object-cache keys use IR payload + runtime/backend fingerprint. Unrelated stdlib file edits do not invalidate every cached object unless they affect generated IR for that build.
- **Lock-check cache**: deterministic lock checks are cached in `target/lock_checks/` to avoid repeated `uv lock --check` and `cargo metadata --locked` on every build.
- **Concurrent rebuild suppression**: backend/runtime Cargo rebuilds acquire file locks under `<CARGO_TARGET_DIR>/.molt_state/build_locks/` so parallel agents sharing a target dir wait instead of duplicating rebuilds.
- **Build state override**: set `MOLT_BUILD_STATE_DIR=<path>` to pin lock/fingerprint metadata to a custom shared location; by default it lives under `<CARGO_TARGET_DIR>/.molt_state/`.
- **sccache auto-enable**: the CLI enables `sccache` automatically when available (`MOLT_USE_SCCACHE=auto`); set `MOLT_USE_SCCACHE=0` to disable. Cargo builds now auto-retry once without `RUSTC_WRAPPER` when a wrapper-level `sccache` failure is detected.
- **Dev profile routing**: `molt ... --profile dev` maps to Cargo profile `dev-fast` by default. Override with `MOLT_DEV_CARGO_PROFILE`; use `MOLT_RELEASE_CARGO_PROFILE` for release lane overrides.
- **Backend daemon**: native backend compiles use a persistent daemon by default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift cold-start. Tune startup with `MOLT_BACKEND_DAEMON_START_TIMEOUT` and in-daemon object cache size with `MOLT_BACKEND_DAEMON_CACHE_MB`.
- **Daemon lifecycle**: daemon sockets/logs/fingerprints live under `<CARGO_TARGET_DIR>/.molt_state/backend_daemon/` (or `MOLT_BUILD_STATE_DIR`). If an agent sees daemon protocol/connectivity errors, the CLI restarts daemon once under lock before falling back to one-shot compile.
- **Bootstrap command**: `tools/throughput_env.sh --apply` (or `eval "$(tools/throughput_env.sh --print)"`) configures:
  - `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache` when external volume exists
  - `CARGO_TARGET_DIR=~/.molt/throughput_target` (local APFS/ext4 for Rust incremental hard-links)
  - `SCCACHE_DIR=/Volumes/APDataStore/Molt/sccache` and `SCCACHE_CACHE_SIZE=20G` on external, else local `10G`
  - `MOLT_USE_SCCACHE=1`, `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`, and `CARGO_INCREMENTAL=0` for better cross-agent cacheability
- **Cache retention**: `tools/throughput_env.sh --apply` runs `tools/molt_cache_prune.py` using defaults (external `200G` + `30` days, local `30G` + `30` days). Override with `MOLT_CACHE_MAX_GB`, `MOLT_CACHE_MAX_AGE_DAYS`, or disable via `MOLT_CACHE_PRUNE=0`.
- **Matrix harness**: use `tools/throughput_matrix.py` for reproducible single-vs-concurrent build throughput checks (profiles + wrapper modes), with optional differential mini-matrix.
  - Example: `uv run --python 3.12 python3 tools/throughput_matrix.py --concurrency 2 --timeout-sec 75 --shared-target-dir /Users/$USER/.molt/throughput_target --run-diff --diff-jobs 2 --diff-timeout-sec 180`
  - Output: `matrix_results.json` under the output root (`/Volumes/APDataStore/Molt/...` by default when present).
  - If rustc prints incremental hard-link fallback warnings, move `--shared-target-dir` to a local APFS/ext4 path.

### Fast Dev Playbook (Recommended)
1. `tools/throughput_env.sh --apply`
2. `uv run --python 3.12 python3 -m molt.cli build --profile dev examples/hello.py --cache-report`
3. `MOLT_DIFF_MEASURE_RSS=1 UV_NO_SYNC=1 uv run --python 3.12 python3 -u tests/molt_diff.py --build-profile dev --jobs 2 tests/differential/basic`

Expected behavior in this lane:
- Runtime/backend Cargo crates rebuild once per fingerprint and profile, with parallel agents waiting on shared build locks.
- Backend codegen uses the persistent daemon for native builds when available (fewer cold starts across repeated builds).
- Per-script object artifacts are reused via `MOLT_CACHE` keying when IR+fingerprint inputs are unchanged.

When throughput regresses unexpectedly:
1. Check lock contention under `<CARGO_TARGET_DIR>/.molt_state/build_locks/`.
2. Check `sccache -s` hit rates and wrapper disable/retry logs.
3. Run `tools/throughput_matrix.py` to isolate profile/wrapper/concurrency regressions.

## Remote Access and Persistent Sessions
This project assumes persistent terminal sessions and remote access.
Contributors are expected to use tmux + mosh (or equivalent) for long-running
work.

### Why this exists
Molt involves:
- compilation pipelines
- benchmarking loops
- WASM builds
- profiling and fuzzing

These workflows do not fit fragile, single-terminal sessions.

### Required baseline
All contributors should have:
- tmux (or screen)
- SSH access
- a way to reconnect safely (mosh recommended)

### Expected behaviors
- Long jobs should be run inside tmux
- Logs should be written to disk, not just stdout
- Partial progress should be preserved
- It must be safe to detach and resume

### What this enables
- Asynchronous contribution
- Background compilation and testing
- Phone-based monitoring of progress

## Logging and Benchmark Conventions
Persistent sessions are only useful if output is durable and inspectable.

### Logging rules
- Long-running tasks MUST log to disk
- Logs should be line-oriented text
- Include timestamps and phase markers
- Avoid excessive stdout-only output

Recommended structure:
```
logs/
  compiler/
  benchmarks/
  wasm/
  agents/
  cpython_regrtest/
```

### Benchmarking rules
- Benchmarks must be reproducible
- Record:
  - git commit
  - machine info
  - date/time
- For WASM benches, capture the native CPython baseline and compute WASM vs
  CPython ratios.
- When validating linked WASM runs, pass `--linked` to `tools/bench_wasm.py` and
  note in logs whether the run used linked or fallback mode.
- Prefer CSV or JSON outputs
- Do not overwrite previous results
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`)
  execute 10 samples and store mean/median/variance/range stats in JSON output;
  reserve these for release tagging or explicit requests.

### Why this matters
- Enables async review
- Enables regression detection
- Enables phone-based inspection

### CPython regrtest harness
The `tools/cpython_regrtest.py` harness writes one run per directory under
`logs/cpython_regrtest/` and emits per-run `summary.md`, `summary.json`, and
`junit.xml`, plus a root `summary.md`. Each run also writes `diff_summary.md`
and `type_semantics_matrix.md` so parity work stays aligned with the stdlib and
type/semantics matrices; `--rust-coverage` adds `rust_coverage/` output
(requires `cargo-llvm-cov`). Use `--clone` and `--uv-prepare` explicitly when
you want networked downloads or dependency installs.
The shim treats `MOLT_COMPAT_ERROR` results as skipped and records the reason
in `junit.xml`; use `tools/cpython_regrtest_skip.txt` for intentional exclusions.
When `--coverage` is enabled, the harness combines host regrtest coverage with
Molt subprocess coverage (use a Python-based `--molt-cmd` to capture it). The
shim forwards interpreter flags from regrtest to the Molt command to preserve
isolation/warnings behavior.
Regrtest runs set `MOLT_MODULE_ROOTS` and `MOLT_REGRTEST_CPYTHON_DIR` to pull
CPython `Lib/test` sources into the static module graph without mutating host
`PYTHONPATH`.
Regrtest runs set `MOLT_CAPABILITIES=fs.read,env.read` by default; override with
`--molt-capabilities` when needed.

Benchmarks that cannot survive a disconnect are not acceptable.

### Profiling harness
- Use `tools/profile.py` for repeatable CPU/alloc profiling runs.
- Default outputs land in `logs/benchmarks/profile_<timestamp>/` with a
  `profile_manifest.json` that records git rev, platform, tool choices, env
  overrides, and per-benchmark artifacts/metrics.
- Prefer `--suite top` for Phase 0 profiling, and add `--profile-compiler` when
  you need compiler-side hotspots.
- Use `--molt-profile` to enable runtime counters (`MOLT_PROFILE=1`) and capture
  allocation/dispatch metrics in the manifest.
- Use `--molt-profile-alloc-sites` to record hot string allocation call sites
  (`MOLT_PROFILE_ALLOC_SITES=1`) and parse `molt_profile_string_site` entries
  into the manifest; tune with `--molt-profile-alloc-sites-limit` when you need
  more or fewer entries.
- Add `--summary` to emit a `profile_summary.json` with top alloc counters,
  allocation-per-call ratios, and hot string allocation sites across benches.
- `molt_profile_cpu_features` lines report detected SIMD features so profile
  manifests can capture which vector paths should be reachable on that host.
- Use `--cpu-tool perf-stat --perf-stat-events ...` on Linux for counter stats
  alongside the time/heap profiling.
- For WASM, use `tools/wasm_profile.py --bench bench_sum` to emit Node
  `.cpuprofile` artifacts in `logs/wasm_profile/<timestamp>/` (pair with
  `--linked` when validating single-module link runs).

## Progress Reporting

### Task scaffolding
Use the helper to create a per-task log + report skeleton:
```bash
tools/new-agent-task.sh <task-name>
```

### Minimal “micro-report” (for frequent updates)
Use this when updating every 10–20 minutes.

```markdown
- [<HH:MM>] <status> — <one-line update>. Output: <path>. Next: <one-line next step>. Resume: <command>.
```

### Full report format
```markdown
## <Task Title>

### Status
<short status line>

### Summary
- ...

### Files Touched
- <path> — <one-line change>

### Tests
- <command> → PASS/FAIL

### Notes
- ...

### Resume
<command>
```

### Coverage reporting helpers
Generate differential coverage summaries and keep spec TODOs in sync:
```bash
uv run --python 3.12 python3 tools/diff_coverage.py
uv run --python 3.12 python3 tools/check_type_coverage_todos.py
```

## Multi-Agent Workflow
This section standardizes parallel agent work on Molt.

### Access and tooling
- Agents may use `gh` (GitHub CLI) and git over SSH.
- Agents are expected to create branches, open PRs, and merge when approved.
- Proactively commit work in logical chunks with clear messages.

### Work partitioning
- Assign each agent a scoped area (runtime/frontend/docs/tests) and avoid
  overlap.
- If cross-cutting changes are required, coordinate early.

### Communication rules
- Always announce: scope, files touched, and expected tests.
- Keep status updates short and explicit.
- Flag any risky changes early.

### Quality gates
- Run extensive linting and tests before PRs or merges.
- Prefer `uv run --python 3.12 python3 tools/dev.py lint` +
  `uv run --python 3.12 python3 tools/dev.py test`, plus relevant `cargo`
  check/test.
- Do not merge if tests are failing unless explicitly approved.

### Merge discipline
- Merge only after tests pass and conflicts are resolved.
- If two agents touch the same area, rebase and re-validate.
