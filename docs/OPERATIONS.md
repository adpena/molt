# Operations Guide

This document consolidates remote access, logging, progress reporting, and
multi-agent workflow rules.

## Version Policy
Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11
compatibility. If 3.12/3.13/3.14 differ, document the chosen target in specs/tests.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see [docs/spec/STATUS.md](docs/spec/STATUS.md)).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
- **Node PATH drift (macOS shells)**: some shells resolve `/usr/local/bin/node` (for example Node 14) before modern Homebrew/fnm nodes. Set `MOLT_NODE_BIN=/opt/homebrew/bin/node` (or another Node >= 18 path) for deterministic wasm lanes.
- **Node wasm instability/noise**: use deterministic Node wasm flags (`--no-warnings --no-wasm-tier-up --no-wasm-dynamic-tiering --wasm-num-compilation-tasks=1`) to avoid warning-noise diffs and post-run Zone OOM incidents on large linked modules.

## Differential Suite (Operational Controls)
- **Memory profiling**: set `MOLT_DIFF_MEASURE_RSS=1` to collect per-test RSS metrics.
- **Build profile default**: diff harness defaults to `--build-profile dev` (override with `--build-profile release` or `MOLT_DIFF_BUILD_PROFILE=release` for release validation).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) records jobs, limits, and RSS aggregates.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `MOLT_DIFF_FAILURES` or `--failures-output`).
- **OOM retry**: OOM failures retry once with `--jobs 1` by default (`MOLT_DIFF_RETRY_OOM=0` disables).
- **Memory caps**: default 10 GB per-process; override with `MOLT_DIFF_RLIMIT_GB`/`MOLT_DIFF_RLIMIT_MB` or disable with `MOLT_DIFF_RLIMIT_GB=0`.
- **Wrapper policy**: diff runs disable `RUSTC_WRAPPER`/`sccache` by default for portability. Opt in with `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` on hosts where wrapper caches are known-good.
- **Pass-log pruning**: when `--log-dir` is enabled, per-test logs for passing tests are pruned by default to reduce clutter. Set `MOLT_DIFF_LOG_PASSES=1` to keep pass logs.
- **Backend daemon policy**: diff runs use `MOLT_DIFF_BACKEND_DAEMON` when set; otherwise defaults to platform-safe auto (`0` on macOS, `1` elsewhere).
- **dyld hardening**: after a dyld import-format incident, diff runs force `MOLT_BACKEND_DAEMON=0` for safety. Set `MOLT_DIFF_QUARANTINE_ON_DYLD=1` only when you need cold target/state quarantine.
- **Local dyld fallback**: on macOS, dyld retries/quarantine can route to local `/tmp` lanes (`MOLT_DIFF_DYLD_LOCAL_FALLBACK=1`, default). Override root with `MOLT_DIFF_DYLD_LOCAL_ROOT=<abs path>`.
- **Cache hardening**: set `MOLT_DIFF_FORCE_NO_CACHE=1|0` to force/disable `--no-cache`. Default is platform-safe auto (`1` on macOS, `0` elsewhere); dyld guard/retry also enables it for the current run.
- **Shared target pinning (macOS)**: explicitly set both `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to the same shared path for diff runs so workers do not drift onto ad-hoc/default targets and duplicate rebuilds.
- **Interrupted-run cleanup**: before a new long sweep, clear stale harness workers from prior crashes (`ps -axo pid,command | rg "tests/molt_diff.py"` then `kill -TERM <pid>`/`kill -KILL <pid>` as needed). Keep one supervising diff run per shared target.

## On-Call Runbook (Stdlib Intrinsics Gates)
Use this section when `python3 tools/check_stdlib_intrinsics.py` fails in CI or
on-call triage.

### Standard First Commands
Run these first to establish current state:
```bash
python3 tools/sync_stdlib_top_level_stubs.py
python3 tools/sync_stdlib_submodule_stubs.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
cat tools/stdlib_intrinsics_ratchet.json
python3 tools/check_stdlib_intrinsics.py --update-doc
```

### WASM Runner Quick Triage
Use these when wasm parity lanes fail before module semantics are reached:
```bash
which -a node
node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -e "const m=require('node:wasi'); console.log(typeof m.WASI)"
which wasmtime
wasmtime --version
```

### Failure String -> Copy-Paste Response
`stdlib intrinsics lint failed: stdlib top-level coverage gate violated`
```bash
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: top-level module/package duplicate mapping`
```bash
uv run --python 3.12 python3 - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py" and len(rel.parts) == 2:
        pkgs[rel.parts[0]] = str(rel)
    elif len(rel.parts) == 1 and p.suffix == ".py":
        mods[p.stem] = str(rel)
for name in sorted(set(mods) & set(pkgs)):
    print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib package kind gate violated`
```bash
uv run --python 3.12 python3 - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required_packages = set(base["STDLIB_PACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required_packages):
    mod = root / f"{name}.py"
    pkg = root / name / "__init__.py"
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: stdlib submodule coverage gate violated`
```bash
python3 tools/sync_stdlib_submodule_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: submodule/package duplicate mapping`
```bash
uv run --python 3.12 python3 - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py":
        if len(rel.parts) > 1:
            pkgs[".".join(rel.parts[:-1])] = str(rel)
    else:
        mods[".".join((*rel.parts[:-1], p.stem))] = str(rel)
for name in sorted((set(mods) & set(pkgs))):
    if "." in name:
        print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib subpackage kind gate violated`
```bash
uv run --python 3.12 python3 - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required = set(base["STDLIB_PY_SUBPACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required):
    parts = name.split(".")
    mod = root.joinpath(*parts[:-1], f"{parts[-1]}.py")
    pkg = root.joinpath(*parts, "__init__.py")
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: unknown intrinsic names`
```bash
python3 tools/gen_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: unknown strict-import modules requested`
```bash
python3 tools/check_stdlib_intrinsics.py --allowlist-modules builtins,sys,types,importlib,importlib.machinery,importlib.util
```

`stdlib intrinsics lint failed: strict-import roots must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
rg -n "TODO\\(stdlib[^,]*,.*status:(missing|partial|planned|divergent)" src/molt/stdlib
```

`stdlib intrinsics lint failed: bootstrap strict roots are incomplete`
```bash
ls src/molt/stdlib/builtins.py src/molt/stdlib/sys.py src/molt/stdlib/types.py
ls src/molt/stdlib/importlib/__init__.py src/molt/stdlib/importlib/machinery.py src/molt/stdlib/importlib/util.py
```

`stdlib intrinsics lint failed: bootstrap strict closure must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: bootstrap modules must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: non-python-only modules cannot depend on python-only stdlib modules`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: strict-import allowlist violated (intrinsic-backed roots imported non-intrinsic-backed stdlib modules)`
```bash
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
```

`stdlib intrinsics lint failed: strict-import roots used forbidden fallback patterns`
```bash
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
```

`stdlib intrinsics lint failed: intrinsic-backed modules used forbidden fallback patterns`
```bash
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: all-stdlib fallback gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
```

`stdlib intrinsics lint failed: intrinsic runtime fallback gate violated`
```bash
rg -n "except .*:\\s*$|pass$" src/molt/stdlib/json/__init__.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: zero non-intrinsic gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/sync_stdlib_submodule_stubs.py --write
```

`stdlib intrinsics lint failed: intrinsic-partial ratchet gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
cat tools/stdlib_intrinsics_ratchet.json
# Lower intrinsic-partial modules first, then tighten max_intrinsic_partial.
```

`stdlib intrinsics lint failed: full-coverage attestation references unknown modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
print("covered_count", len(covered))
for name in sorted(covered):
    print(name)
PY
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract has non-attested modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("contract_without_attestation", sorted(contract - covered))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract missing modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("attested_without_contract", sorted(covered - contract))
PY
```

`stdlib intrinsics lint failed: full-coverage modules must remain intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
statuses = {entry["module"]: entry["status"] for entry in payload["modules"]}
for name in payload.get("fully_covered_modules", []):
    if statuses.get(name) != "intrinsic-backed":
        print(name, statuses.get(name))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract references unknown intrinsics`
```bash
python3 tools/gen_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
print(payload.get("full_coverage_required_intrinsics", {}))
PY
```

`Host fallback imports (\`_py_*\`) are forbidden in stdlib modules.` (file-level scan error)
```bash
rg -n "import _py_|from _py_|__import__\\(|import_module\\(" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: ...` (dynamic message, e.g. baseline invalid/missing)
```bash
python3 tools/gen_stdlib_module_union.py
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/sync_stdlib_submodule_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed:` (file-level scan errors block)
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
uv run --python 3.12 ruff format src/molt/stdlib tools/check_stdlib_intrinsics.py
```

### Differential Expected-Failure Policy (Too Dynamic)
Use this for intentionally unsupported dynamism (for example `exec`/`eval`)
that is documented by vision/break-policy constraints.

```bash
# 1) Declare the planned differential tests in:
#    tools/stdlib_full_coverage_manifest.py
#    TOO_DYNAMIC_EXPECTED_FAILURE_TESTS
#
# 2) Run differential lane as normal; harness auto-converts fail->pass as XFAIL
#    only when CPython passes and the test path is in that manifest tuple.
MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_RLIMIT_GB=10 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/exec_locals_scope.py

# 3) XPASS is treated as failure; remove stale expected-failure entries when
#    Molt gains support.
```

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
- **Daemon socket placement**: sockets default to a local temp dir (`MOLT_BACKEND_DAEMON_SOCKET_DIR`, or explicit `MOLT_BACKEND_DAEMON_SOCKET`) so shared/external volumes that do not support Unix sockets do not break daemon startup.
- **Daemon lifecycle**: daemon logs/pid/fingerprints live under `<CARGO_TARGET_DIR>/.molt_state/backend_daemon/` (or `MOLT_BUILD_STATE_DIR`). If an agent sees daemon protocol/connectivity errors, the CLI restarts daemon once under lock before falling back to one-shot compile.
- **Bootstrap command**: `tools/throughput_env.sh --apply` (or `eval "$(tools/throughput_env.sh --print)"`) configures:
  - `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache` when external volume exists
  - `CARGO_TARGET_DIR=~/.molt/throughput_target` (local APFS/ext4 for Rust incremental hard-links)
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` so differential runs reuse the same shared Cargo artifacts by default
  - `SCCACHE_DIR=/Volumes/APDataStore/Molt/sccache` and `SCCACHE_CACHE_SIZE=20G` on external, else local `10G`
  - `MOLT_USE_SCCACHE=1`, `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`, and `CARGO_INCREMENTAL=0` for better cross-agent cacheability
- **Cache retention**: `tools/throughput_env.sh --apply` runs `tools/molt_cache_prune.py` using defaults (external `200G` + `30` days, local `30G` + `30` days). Override with `MOLT_CACHE_MAX_GB`, `MOLT_CACHE_MAX_AGE_DAYS`, or disable via `MOLT_CACHE_PRUNE=0`.
- **Diff run coordination lock**: `tests/molt_diff.py` acquires `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock` so concurrent agents queue instead of running overlapping diff sweeps. Tune with `MOLT_DIFF_RUN_LOCK_WAIT_SEC` (default 900) and `MOLT_DIFF_RUN_LOCK_POLL_SEC`.
- **Matrix harness**: use `tools/throughput_matrix.py` for reproducible single-vs-concurrent build throughput checks (profiles + wrapper modes), with optional differential mini-matrix.
  - Example: `uv run --python 3.12 python3 tools/throughput_matrix.py --concurrency 2 --timeout-sec 75 --shared-target-dir /Users/$USER/.molt/throughput_target --run-diff --diff-jobs 2 --diff-timeout-sec 180`
  - Output: `matrix_results.json` under the output root (`/Volumes/APDataStore/Molt/...` by default when present).
  - If rustc prints incremental hard-link fallback warnings, move `--shared-target-dir` to a local APFS/ext4 path.
- **Compile-progress suite**: use `tools/compile_progress.py` for standardized
  cold/warm + cache-hit/no-cache + daemon-on/off compile tracking.
  - Example: `uv run --python 3.12 python3 tools/compile_progress.py --clean-state`
  - Include `--diagnostics` to capture per-case phase timing + module-reason
    payloads from compiler builds.
  - Queue lane (daemon warm queue): add
    `--cases dev_queue_daemon_on dev_queue_daemon_off`.
  - Release-iteration lane (`release-fast` Cargo profile override): add
    `--cases release_fast_cold release_fast_warm release_fast_nocache_warm`.
  - Under host contention, prefer:
    `--max-retries 2 --retry-backoff-sec 2 --build-lock-timeout-sec 60`.
  - Timeouts are fail-safe: the harness now kills run-scoped timed-out
    compiler children (`cargo`/`rustc`/`sccache`) and run-scoped backend
    daemons before retrying/continuing.
  - `SIGTERM` exits (`rc=143`/`rc=-15`) are treated as retryable.
  - Snapshots (`compile_progress.json` / `.md`) are updated after each
    completed case so interrupted long runs still preserve progress.
  - Use `--resume` in persistent `tmux`/`mosh` sessions to continue interrupted
    long sweeps (especially release lanes) without rerunning completed cases.
  - Outputs: `compile_progress.json` + `compile_progress.md` + per-case logs.
  - Initiative KPI board lives at `docs/benchmarks/compile_progress.md`.
- **Build diagnostics**: enable compiler phase timing + module-inclusion reasons
  on demand with `MOLT_BUILD_DIAGNOSTICS=1`.
  - Optional output file: `MOLT_BUILD_DIAGNOSTICS_FILE=<path>` (relative paths are
    resolved under the build artifacts directory).
  - Example:
    `MOLT_BUILD_DIAGNOSTICS=1 MOLT_BUILD_DIAGNOSTICS_FILE=build_diag.json uv run --python 3.12 python3 -m molt.cli build --profile dev --no-cache examples/hello.py`

## Weekly Stdlib Scoreboard
Update this table at least weekly during lowering burn-down.

| Date | intrinsic-backed | intrinsic-partial | probe-only | python-only | missing top-level | missing submodules | native parity pass % | wasm parity pass % | memory regressions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-02-12 | 177 | 696 | 0 | 0 | 0 | 0 | TBD | TBD | TBD |

Rules:
- Any PR that worsens this table requires explicit exception sign-off.
- Ratchet updates (`tools/stdlib_intrinsics_ratchet.json`) must only move downward and must ship with real lowering progress in the same change.

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
