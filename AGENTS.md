# Repository Guidelines

## Hard Gate: External Volume Only (Non-Negotiable, Urgent)
- We are disk-space constrained on the local/internal drive. Local development MUST place all build artifacts, logs, caches, tmp files, debugging outputs, and `target/`-style directories on the external volume rooted at `/Volumes/APDataStore/Molt`.
- This is not advisory. If `/Volumes/APDataStore/Molt` is not mounted, do not run heavy workflows (build, run, diff, test, bench, regrtest). Mount the volume first (or explicitly choose a different external root and plumb it through the env vars below).
- Hard prohibition: do not generate large artifacts under the repo on the local drive (examples: `target/`, `dist/`, `build/`, `wasm/`, `logs/`, `bench/results/`, `runtime/**/target/`). Those are emergency-only fallbacks when the external volume is unavailable and the task is explicitly approved as local-disk-impacting.
- Canonical env defaults (use these in your shell before any build/test/bench work):
  - `export MOLT_EXT_ROOT=/Volumes/APDataStore/Molt`
  - `export CARGO_TARGET_DIR=$MOLT_EXT_ROOT/cargo-target`
  - `export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR`
  - `export MOLT_CACHE=$MOLT_EXT_ROOT/molt_cache`
  - `export MOLT_DIFF_ROOT=$MOLT_EXT_ROOT/diff`
  - `export MOLT_DIFF_TMPDIR=$MOLT_EXT_ROOT/tmp`
  - `export UV_CACHE_DIR=$MOLT_EXT_ROOT/uv-cache`
  - `export TMPDIR=$MOLT_EXT_ROOT/tmp`
- Notes:
  - `CARGO_TARGET_DIR` also relocates Molt’s shared build state under `<CARGO_TARGET_DIR>/.molt_state/` (locks, fingerprints, daemon state). This is mandatory to prevent local `target/` growth.
  - If you hit Unix-socket filesystem limitations on the external volume, keep artifacts on external but place only daemon sockets on local temp: `export MOLT_BACKEND_DAEMON_SOCKET_DIR=/tmp/molt_backend_sockets` (small/allowed exception).

## Non-Negotiable: Raise On Missing Features
- Always raise on missing features; never fallback silently.
- Never build coverage or implementations that rely on host Python in any way.
- Always assume compiled Molt binaries will run in environments with no Python installation at all.
- Stdlib modules must be Rust-native intrinsics for compiled binaries; any Python stdlib files may only be thin, intrinsic-forwarding wrappers with zero host-Python imports.
- Absolutely no CPython stdlib imports or `_py_*` fallback modules in compiled binaries (tooling-only shims are allowed).
- Intrinsics are mandatory: missing intrinsics must raise immediately (standardized `RuntimeError`), and differential tests should fail fast when intrinsics are missing.

## Intrinsics & Stdlib Lowering (Non-Negotiable)
- All stdlib behavior must lower into Rust intrinsics; Python stdlib files are only thin wrappers for argument normalization, error mapping, and capability gating.
- Load intrinsics via `src/molt/stdlib/_intrinsics.py` (module `globals()` first, then `builtins._molt_intrinsics`); do not invent alternative registries or hidden import-time side effects.
- Required behavior must use `require_intrinsic` or explicit `RuntimeError`/`ImportError` when missing; optional features must be explicit and capability-gated with clear errors, never silent fallback to host Python.
- Standardize intrinsic naming and registration through `runtime/molt-runtime/src/intrinsics/manifest.pyi`, and regenerate `src/molt/_intrinsics.pyi` plus `runtime/molt-runtime/src/intrinsics/generated.rs` via `tools/gen_intrinsics.py`.
- Prefer standardization, performance, and correctness: push hot paths and semantics into Rust, keep Python shims minimal and deterministic, and avoid CPython/host-stdlib dependencies.

## Hard Gate: Rust-Only Stdlib Turn Blocker (Non-Negotiable)
- If a change adds or modifies stdlib behavior in `src/molt/stdlib/**`, the behavior must be implemented in Rust intrinsics first; Python code may only wire arguments, errors, and capability checks.
- Do not add Python-side fallback logic, compatibility emulation, or host-stdlib implementation paths to make tests pass.
- For every stdlib behavior change, include an explicit intrinsic mapping in the same change:
`runtime/molt-runtime/src/intrinsics/manifest.pyi` entry, Rust implementation, and regenerated `src/molt/_intrinsics.pyi` + `runtime/molt-runtime/src/intrinsics/generated.rs`.
- If no intrinsic exists for required behavior, stop immediately and raise the missing intrinsic as the blocker; do not proceed with a Python implementation.
- Before ending a turn, provide a short Rust-lowering audit for touched stdlib modules:
module path, intrinsic names used, and confirmation that no host-Python fallback path was added.

## Rules Of Thumb For New Work (Non-Negotiable)
- Add or extend a runtime/compiler primitive when the behavior is a reusable low-level hot semantic.
- Expose that primitive capability to stdlib through a Rust intrinsic (manifested and registered canonically).
- Expose user-facing language/core behavior through builtins and stdlib APIs that call intrinsics/primitives, not Python reimplementations of runtime semantics.

## Mission (Non-Negotiable)
Build relentlessly with high productivity, velocity, and vision in the spirit and honor of Jeff Dean. Always build fully, completely, correctly, and performantly; avoid workarounds. Guiding question: "What would Jeff Dean do?"

## Strategic Target (Non-Negotiable)
- Performance target: achieve parity with or superiority over Codon.
- WASM performance target: achieve parity with or superiority over Pyodide on Molt’s canonical wasm benchmark suites and representative real-world workloads.
- Compatibility/interoperability target: get close to or match Nuitka's CPython coverage and interoperability for Molt-supported semantics.

## Version Target (Non-Negotiable)
- Molt targets Python 3.12+ semantics only. Do not spend effort on <=3.11 compatibility.
- When behavior differs across 3.12/3.13/3.14, document the choice explicitly in specs/tests and keep the runtime aligned with the documented version.

## Jeff Dean Protege Mode (Non-Negotiable)
- Optimize for correctness, performance, and determinism before convenience. No shortcuts that degrade runtime guarantees.
- Default path is native Molt lowering + Rust runtime. Treat CPython bridge paths as explicit, opt-in compatibility layers only.
- Prefer recompiled C-extensions against a `libmolt` C-API subset over any embedded CPython strategy.
- Any bridge usage must be capability-gated, off by default, and always visible in logs/metrics.
- Measure performance impacts with benchmarks; treat regressions as failures and iterate until green.

## Project Structure & Module Organization
- `src/molt/` contains the Python compiler frontend and CLI (`cli.py`).
- `runtime/` hosts Rust crates for the runtime and object model (`molt-runtime`, `molt-obj-model`, `molt-backend`).
- `tests/` holds Python tests, including differential suites in `tests/differential/` and smoke/compliance tests.
- `examples/` contains small programs used in docs and manual validation.
- `docs/spec/` is the architecture and runtime specification set; treat it as the source of truth for behavior.
- `tools/` includes developer scripts like `tools/dev.py`.
- Keep Rust crate entrypoints (`lib.rs`) thin; place substantive runtime/backend logic in focused modules under `src/` and re-export from `lib.rs`.
- Standardize naming: Python modules use `snake_case`, Rust crates use `kebab-case`, and paths reflect module names (avoid ad-hoc casing).

## Key Docs
- [docs/CANONICALS.md](docs/CANONICALS.md): must-read documents for new work.
- [docs/INDEX.md](docs/INDEX.md): documentation map and entry points.
- [docs/spec/README.md](docs/spec/README.md): spec index by area.
- [CONTRIBUTING.md](CONTRIBUTING.md): workflow expectations and the change impact matrix.
- [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md): architecture map, layer ownership, and integration checklist.
- [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md) and [docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md): vision, scope, and explicit break policy.
- [docs/spec/STATUS.md](docs/spec/STATUS.md) and [ROADMAP.md](ROADMAP.md): canonical current scope/limits and the active forward-looking plan.
- [docs/ROADMAP.md](docs/ROADMAP.md): detailed archive/reference roadmap context.
- [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md): minimum must-pass gate matrix.
- [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md): determinism/security enforcement checklist.
- [docs/OPERATIONS.md](docs/OPERATIONS.md): remote access, logging, benchmarks, progress reports, and multi-agent workflow.
- [docs/BENCHMARKING.md](docs/BENCHMARKING.md): benchmarking overview.

## Build, Test, and Development Commands
- `cargo build --release --package molt-runtime`: build the Rust runtime used by compiled binaries.
- `export PYTHONPATH=src`: make the Python package importable from the repo root.
- `python3 -m molt.cli build examples/hello.py`: compile a Python example to a native binary.
- `./hello_molt`: run the compiled output from the previous step.
- `python3 -m molt.cli build --target wasm --linked examples/hello.py`: emit `output.wasm` and `output_linked.wasm` for wasm targets (linked requires `wasm-ld` + `wasm-tools`).
- `python3 -m molt.cli build --target wasm --linked --linked-output dist/app.wasm examples/hello.py`: customize the linked output path.
- `python3 -m molt.cli build --target wasm --require-linked examples/hello.py`: enforce linked output and remove the unlinked artifact after linking.
- `molt build --module mypkg`: compile a package/module entrypoint (uses `mypkg.__main__` when present).
- Vendored deps in `vendor/` are added to module roots and `PYTHONPATH` automatically (or set `MOLT_MODULE_ROOTS` explicitly).
- `molt run --timing examples/hello.py`: compile+run the native binary and emit build/run timing (no CPython fallback).
- `molt compare examples/hello.py -- --arg 1`: compare CPython vs Molt output with separate build/run timing (CPython required for baseline only).
- `molt bench --script examples/hello.py`: run the bench harness on a custom script.
- `MOLT_TRUSTED=1`, `molt run --trusted`, `molt build --trusted`, `molt diff --trusted`, or `molt test --trusted`: disable capability checks for trusted native deployments.
- Build cache determinism is now enforced by default in the CLI (`PYTHONHASHSEED=0`) to stabilize cache keys across invocations. Override with `MOLT_HASH_SEED=<value>` (set `MOLT_HASH_SEED=random` to opt out).
- Lockfile verification (`uv lock --check`, `cargo metadata --locked`) is cached under `<CARGO_TARGET_DIR>/lock_checks/` when `CARGO_TARGET_DIR` is set (otherwise `target/lock_checks/`); remove those files when you need to force a full lock re-check.
- Development profile routing: `--profile dev` maps to Cargo profile `dev-fast` by default (override with `MOLT_DEV_CARGO_PROFILE`; release uses `MOLT_RELEASE_CARGO_PROFILE`).
- Runtime/backend Cargo rebuilds use lock files under `<CARGO_TARGET_DIR>/.molt_state/build_locks/` to prevent duplicate rebuild storms across concurrent agents.
- Native backend compiles use a local backend daemon by default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift startup; tune with `MOLT_BACKEND_DAEMON_START_TIMEOUT` and `MOLT_BACKEND_DAEMON_CACHE_MB`.
- Build/daemon fingerprints + lock state live under `<CARGO_TARGET_DIR>/.molt_state/` (or `MOLT_BUILD_STATE_DIR` when set). Daemon sockets default to a local temp dir (`MOLT_BACKEND_DAEMON_SOCKET_DIR`) to avoid external filesystems that do not support Unix sockets; logs/pid remain under build state.
- `tools/dev.py lint`: run `ruff` checks, `ruff format --check`, and `ty check` via `uv run` (Python 3.12).
- `tools/dev.py test`: run the Python test suite (`pytest -q`) via `uv run` on Python 3.12/3.13/3.14.
- `python3 tools/cpython_regrtest.py --clone`: run CPython regrtest against Molt (logs under `logs/cpython_regrtest/`); defaults to `python -m molt.cli run`.
- `python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare --coverage`: run regrtest with uv-managed Python + coverage.
- `cargo test`: run Rust unit tests for runtime crates.
- `uv sync --group bench --python 3.12`: install optional benchmark deps before running `tools/bench.py` (PyPy/Codon/Nuitka/Pyodide lanes are optional and auto-skipped when unavailable).
- If `uv run` panics in sandboxed or restricted environments, reuse the existing
  environment by setting `UV_NO_SYNC=1`. Prefer `UV_CACHE_DIR=/tmp/uv-cache` inside
  the sandbox when external volumes are blocked.
- If the panic mentions `system-configuration` (macOS proxy lookup), pin explicit
  proxy envs to bypass system proxy detection, for example:
  `HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 NO_PROXY=localhost,127.0.0.1`.
- If the panic is due to missing deps, run `uv sync --group dev --python 3.12`
  locally (outside the sandbox) to populate `.venv`, then rerun with `UV_NO_SYNC=1`.

## Build Profile Policy (Non-Negotiable)
- Development workflows must use `--profile dev` for `molt build`, `molt run`, `molt compare`, `molt diff`, and `molt test --suite diff`, unless explicitly validating production artifacts.
- Production benchmarks, release validation, and published binaries must use `--profile release`.
- Do not silently switch profiles in wrappers/harnesses; profile selection must be explicit and reproducible in command lines/config.

## No CPython Fallback (Non-Negotiable)
- Molt-compiled binaries must run on systems without Python installed; do not depend on `python`, `sys.executable`, or CPython at runtime.
- Never implement CPython fallback/bridging in CLI, runtime, tests, or tooling. Unsupported constructs must be compile-time errors or `bridge_unavailable` runtime exits when `--fallback bridge` is explicitly requested.
- CPython is only allowed for baseline comparisons (`molt compare`, `tests/molt_diff.py`, CPython regrtest); it must be explicit and never used to execute Molt binaries.

## Tooling Add-ons (Optional)
- `uv run pre-commit install` and `uv run pre-commit run -a`: enable repo hooks (ruff/ty formatting + checks).
- `python3 tools/diff_coverage.py`: generate [tests/differential/COVERAGE_REPORT.md](tests/differential/COVERAGE_REPORT.md).
- `python3 tools/check_type_coverage_todos.py`: ensure type/stdlib TODOs are mirrored in [ROADMAP.md](ROADMAP.md).
- `python3 tools/runtime_safety.py clippy|miri|fuzz --target string_ops --runs 10000`: runtime safety gates.
- `cargo audit` and `cargo deny check`: Rust supply-chain audits.
- `uv run pip-audit`: Python dependency audit (run after `uv sync --group dev`).
- `cargo nextest run -p molt-runtime --all-targets`: faster Rust test runner.
- `export RUSTC_WRAPPER=sccache`: enable Rust compile caching (check stats with `sccache -s`).
- The CLI auto-enables `sccache` when available (`MOLT_USE_SCCACHE=auto`); set `MOLT_USE_SCCACHE=0` to disable or `MOLT_USE_SCCACHE=1` to require it in your shell setup.
- `uv run --python 3.12 python3 tools/throughput_matrix.py`: run the build-throughput matrix (single-agent vs concurrent, wrapper on/off, dev/release) and write JSON artifacts under the external volume when available. Prefer `--shared-target-dir <apfs/ext4 path>` for faster Rust incremental compiles.
- `eval "$(tools/throughput_env.sh --print)"` (or `tools/throughput_env.sh --apply`): bootstrap throughput env defaults with external-volume-first caches, shared target dir, shared diff target (`MOLT_DIFF_CARGO_TARGET_DIR`), and `sccache` sizing (`20G` on external, `10G` local fallback).
- Fast multi-agent bootstrap (recommended before long diff sweeps): `tools/throughput_env.sh --apply && uv run --python 3.12 python3 -m molt.cli build --profile dev examples/hello.py --cache-report`.
- Throughput bootstrap also sets `CARGO_INCREMENTAL=0` by default to improve cross-run/cacheability in highly concurrent workflows; override to `1` when investigating local incremental-only behavior.
- `python3 tools/molt_cache_prune.py`: enforce Molt cache retention policy (defaults: external `200G` + `30` days; local `30G` + `30` days).
- `cargo bloat -p molt-runtime --release` and `cargo llvm-lines -p molt-runtime`: size attribution.
- `cargo flamegraph -p molt-runtime --bench ptr_registry`: native flamegraphs.

## WASM Tooling
- Bench harness: `tools/bench_wasm.py` (`--linked` uses `wasm-ld` when available; `--require-linked` aborts if linking fails).
- Linking helper: `tools/wasm_link.py` (single-module linking via `wasm-ld`).
- Profiling helper: `tools/wasm_profile.py` (Node `--cpu-prof` for wasm benches).
- Inspect binaries: `wasm-tools print <file.wasm>` for imports/exports/sections.
- Size analysis: `twiggy top <file.wasm>` for WASM size attribution.
- Size optimization: `wasm-opt -Oz -o output.opt.wasm output.wasm` (Binaryen).
- Runtime harness: `run_wasm.js` (Node/WASI; prefers `*_linked.wasm` when present, set `MOLT_WASM_PREFER_LINKED=0` to opt out).
- Runner prefers linked wasm when `*_linked.wasm` exists next to the input (disable with `MOLT_WASM_PREFER_LINKED=0`).
- Linked builds require `wasm-ld` and `wasm-tools` (install via Homebrew `llvm` + `wasm-tools` or Cargo).
- Override relocatable table base with `MOLT_WASM_TABLE_BASE=<u32>` (defaults to runtime table size when available).

## Coding Style & Naming Conventions
- Python: 4-space indentation, `ruff` line length 88, target version 3.13, and strict typing via `ty`.
- Formatting: use `ruff format` (black-style) as the canonical formatter before builds to avoid inconsistent quoting or style drift.
- Rust: format with `cargo fmt` and keep clippy clean (`cargo clippy -- -D warnings`).
- Tests follow `test_*.py` naming; keep test modules in `tests/` or subdirectories like `tests/differential/`.

## Stdlib Submodule Policy
- Treat stdlib submodules (e.g., `asyncio.locks`) as first-class entries in the compatibility matrix.
- Register submodules explicitly (create module objects, add to `sys.modules`, and attach on the parent package) instead of relying on dynamic attribute lookups.
- Keep submodules deterministic and capability-gated where they touch host I/O, OS, or process boundaries.
- Update [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) when submodule coverage changes.

## Runtime Locking & Unsafe Policy
- Runtime mutation requires the GIL token; do not bypass it.
- Unsafe code must live in provenance/object modules; other runtime modules should be safe Rust.
- When changing handle resolution or the pointer registry, run strict provenance checks (Miri when available) and the lock-sensitive bench subset.

## Testing Guidelines
- Run differential parity through the harness, not raw pytest: `uv run --python 3.12 python3 -u tests/molt_diff.py <file_or_dir>`.
- Differential lane contract:
  - `tests/differential/basic`: core language + builtins.
  - `tests/differential/stdlib`: stdlib modules/submodules.
  - `tests/differential/moltlib`: Molt-only APIs (optional lane for non-CPython surfaces).
- Do not add new tests under retired lanes: `tests/differential/planned`, `tests/differential/core`, or `tests/differential/scoping`.
- After adding or moving differential tests, run lane integrity gates:
  - `python3 tools/check_differential_suite_layout.py`
  - `python3 tools/gen_diff_lanes.py`
- Expected-failure governance for dynamic semantics:
  - Register intentional dynamic gaps (for example `exec`/`eval`) only in `tools/stdlib_full_coverage_manifest.py` under `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`.
  - Keep `tests/test_molt_diff_expected_failures.py` green so manifest coverage and `XFAIL`/`XPASS` behavior stay enforced.
- Run the core-lane lowering gate with the current manifest path:
  - `python3 tools/check_core_lane_lowering.py --manifest tests/differential/basic/CORE_TESTS.txt`
- NON-NEGOTIABLE: Differential work MUST use the external volume root `/Volumes/APDataStore/Molt` (see “Hard Gate: External Volume Only”). If it is not available, abort the run rather than filling the internal drive.
- NON-NEGOTIABLE: Always run the differential testing suite with memory profiling enabled (`MOLT_DIFF_MEASURE_RSS=1`).
- NON-NEGOTIABLE: Treat memory blowups as failures; if RSS climbs rapidly or threatens system stability, terminate the diff run early (kill the harness) and record the abort plus last-known RSS metrics in [tests/differential/INDEX.md](tests/differential/INDEX.md).
- NON-NEGOTIABLE: Enforce a 10 GB per-process memory cap for diff runs when possible.
  - macOS/Linux: `ulimit -Sv 10485760` (KB) or `ulimit -v 10485760` in the shell that launches the suite.
  - If the limit is hit or memory pressure occurs, reduce parallelism (`--jobs 2` or `--jobs 1`) and rerun.
- Differential artifacts can be redirected to an external volume to avoid local disk pressure.
  - Set `MOLT_DIFF_ROOT` to an absolute path; all per-test build artifacts, caches, and temp dirs will live under it.
  - Optional: set `MOLT_DIFF_TMPDIR` to override only the temp root.
  - Optional: set `MOLT_CACHE` to a shared path to reuse Molt codegen artifacts across tests (dramatically faster on large suites).
  - Optional: set `MOLT_DIFF_KEEP=1` to preserve per-test artifacts after each run.
  - Optional: set `MOLT_DIFF_TRUSTED=1` to force trusted mode for diff runs (defaults to trusted unless `MOLT_DEV_TRUSTED=0`).
  - Default to a shorter timeout unless a test is known to be slow: `MOLT_DIFF_TIMEOUT=180` (bump per-test only when needed).
  - Optional: set `MOLT_DIFF_RLIMIT_GB=10` (default) or `MOLT_DIFF_RLIMIT_MB=<n>` to enforce a per-process memory cap; set to `0` to disable.
  - Optional: set `MOLT_DIFF_MEM_PER_JOB_GB=<n>` to tune auto-parallelism by memory budget (default: 2 GB/worker).
  - Optional: set `MOLT_DIFF_MAX_JOBS=<n>` to hard-cap the auto-selected job count.
  - Optional: set `MOLT_DIFF_ORDER=auto|name|size-asc|size-desc` to control scheduling order (default: auto).
  - Optional: set `MOLT_DIFF_FAILURES=<path>` or pass `--failures-output <path>` to capture a failure queue file.
  - Optional: set `MOLT_DIFF_WARM_CACHE=1` or pass `--warm-cache` to prebuild all tests once to seed `MOLT_CACHE` before the diff run (useful for large suites).
  - Optional: set `MOLT_DIFF_RETRY_OOM=1` (default) or pass `--no-retry-oom` to disable the one-shot OOM retry with `--jobs 1`.
  - Optional: set `MOLT_DIFF_SUMMARY=<path>` or read `MOLT_DIFF_ROOT/summary.json` for the LLM-friendly summary sidecar (includes RSS aggregates when enabled).
  - Optional: set `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` to allow `RUSTC_WRAPPER`/`sccache` during diff runs; by default the harness disables wrappers for maximum portability/reproducibility.
  - Optional: set `MOLT_DIFF_LOG_PASSES=1` to keep per-test logs for passing tests when `--log-dir` is used (default prunes pass logs to reduce clutter).
  - Optional: set `MOLT_DIFF_CARGO_TARGET_DIR=<abs path>` to force diff-run Cargo artifacts into a shared target dir; default comes from throughput bootstrap (`MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR`).
  - Optional (recommended on macOS when many agents are active): explicitly set both `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to the same shared path to avoid accidental fallback to ad-hoc/default targets that can trigger duplicate rebuild storms.
  - Optional: set `MOLT_DIFF_RUN_LOCK_WAIT_SEC=<seconds>` to control how long a diff run waits for the shared run lock (`<CARGO_TARGET_DIR>/.molt_state/diff_run.lock`, default 900s). Set `MOLT_DIFF_RUN_LOCK_POLL_SEC=<seconds>` to tune lock polling cadence.
  - Optional: set `MOLT_DIFF_BACKEND_DAEMON=1|0` to force daemon mode for diff runs. Default is platform-safe auto (`0` on macOS, `1` elsewhere) to avoid dyld import-format instability.
  - Optional: set `MOLT_DIFF_QUARANTINE_ON_DYLD=1` to force cold target/state quarantine after a dyld incident. Default keeps shared target/cache and disables daemon only.
  - Optional: set `MOLT_DIFF_DYLD_LOCAL_FALLBACK=1|0` to enable/disable local `/tmp` retry + quarantine lanes for dyld incidents. Default is `1` on macOS (`0` elsewhere).
  - Optional: set `MOLT_DIFF_DYLD_LOCAL_ROOT=<abs path>` to override the local dyld quarantine root (default: `/tmp/molt_diff_dyld`).
  - Optional: set `MOLT_DIFF_FORCE_NO_CACHE=1|0` to force/disable `--no-cache` in diff runs. Default is platform-safe auto (`1` on macOS, `0` elsewhere) and dyld guard/retry also enables it.
  - Optional cleanup for interrupted/crashed sessions before starting a new long run: `ps -axo pid,command | rg "tests/molt_diff.py"` then `kill -TERM <pid>` (and `kill -KILL <pid>` if needed). Keep one supervising diff run per shared target to minimize contention and memory spikes.
  - Example (external volume + shared cache + temp root): `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache MOLT_DIFF_ROOT=/Volumes/APDataStore/Molt/diff MOLT_DIFF_TMPDIR=/Volumes/APDataStore/Molt/tmp MOLT_DIFF_KEEP=1 MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic`.
- Example (RSS metrics): `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache MOLT_DIFF_ROOT=/Volumes/APDataStore/Molt/diff MOLT_DIFF_TMPDIR=/Volumes/APDataStore/Molt/tmp MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_KEEP=1 MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic`.
  - Example (watch RSS during run): `ps -o pid=,rss=,command= -p <PID> | awk '{printf "pid=%s rss_kb=%s cmd=%s\n",$1,$2,$3}'` (record spikes in [tests/differential/INDEX.md](tests/differential/INDEX.md)).
  - Example (kill on blowup): `kill -TERM <PID>` then `kill -KILL <PID>` if it does not exit quickly; log the abort + last-known RSS in [tests/differential/INDEX.md](tests/differential/INDEX.md).
- Example (multi-target list, auto-parallel): `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache MOLT_DIFF_ROOT=/Volumes/APDataStore/Molt/diff MOLT_DIFF_TMPDIR=/Volumes/APDataStore/Molt/tmp MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/augassign_inplace.py tests/differential/basic/container_mutation.py tests/differential/basic/ellipsis_basic.py`
  - Example (parallel full sweep + live log + aggregate log + per-test logs):
    `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache MOLT_DIFF_ROOT=/Volumes/APDataStore/Molt/diff MOLT_DIFF_TMPDIR=/Volumes/APDataStore/Molt/tmp MOLT_DIFF_TIMEOUT=180 MOLT_DIFF_GLOB='**/*.py' uv run --python 3.12 python3 -u tests/molt_diff.py --jobs 8 --live --log-file /Volumes/APDataStore/Molt/diff_live.log --log-aggregate /Volumes/APDataStore/Molt/diff_full.log --log-dir /Volumes/APDataStore/Molt/diff_logs tests/differential`
  - Example (monitor live log): `tail -f /Volumes/APDataStore/Molt/diff_live.log`
  - Example (monitor aggregate log): `tail -f /Volumes/APDataStore/Molt/diff_full.log`
  - Disable trusted default: `MOLT_DEV_TRUSTED=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic`.
  - Optional speed workflow: prebuild runtime (`cargo build --release --package molt-runtime`), then do a two-pass diff run (no RSS first, RSS only for failures).
  - Always update [tests/differential/INDEX.md](tests/differential/INDEX.md) after diff runs:
    - Record the run date/time, host Python (`uv run --python 3.12/3.13/3.14`), totals, and failure list.
    - Use `/Volumes/APDataStore/Molt/rss_metrics.jsonl` to extract the latest per-test status when RSS is enabled.
    - Prefer re-running only failing tests (Failure Queue) unless a full sweep is explicitly requested.
- `tests/molt_diff.py` accepts multiple file/dir arguments and runs them in parallel by default (auto `--jobs`); use a shell loop only when you need custom ordering or retries.
- The `tests/differential/basic/bytes_codec.py` case requires `msgpack` + `cbor2` (install via `uv sync --group dev`); otherwise the diff harness will skip it.
- Use `tools/cpython_regrtest.py` to track CPython regression parity; it uses `tools/molt_regrtest_shim.py` to run tests via `--molt-cmd`. Keep skip reasons in `tools/cpython_regrtest_skip.txt`, and review `summary.md` (in each `logs/cpython_regrtest/<run>/`) + `junit.xml` in `logs/cpython_regrtest/`.
- `--coverage` now combines host regrtest + Molt subprocess coverage (requires `coverage` and a Python-based `--molt-cmd`; non-Python commands log a warning and skip Molt coverage).
- Regrtest runs set `MOLT_CAPABILITIES=fs.read,env.read` by default; override with `--molt-capabilities` if you need stricter or broader access.
- The regrtest shim marks `MOLT_COMPAT_ERROR` results as skipped; check `junit.xml` for reasons and codify intentional exclusions in `tools/cpython_regrtest_skip.txt`.
- The regrtest shim forces `MOLT_PROJECT_ROOT` to the repo so compiled runs link against the Molt runtime even for `third_party/` test sources.
- The regrtest shim sets `MOLT_MODULE_ROOTS` (and `MOLT_REGRTEST_CPYTHON_DIR`) to the CPython `Lib` directory so `test.*` resolves to CPython sources; avoid exporting that path via `PYTHONPATH` to the host Python.
- Use `molt test` for fast iteration, then use regrtest to surface broad regressions and map failures back to the stdlib matrix ([docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md)).
- Regrtest runs also emit `diff_summary.md` and `type_semantics_matrix.md` in each `logs/cpython_regrtest/<run>/` run directory to track type/semantics coverage gaps against `0014`/`0023`.
- Use `--no-diff` if you want regrtest-only runs (the diff suite is enabled by default).
- Use `--rust-coverage` with `cargo-llvm-cov` installed to collect Rust runtime coverage under `logs/cpython_regrtest/<ts>/py*/rust_coverage/`.
- Keep semantic tests deterministic; update or add differential cases when changing runtime or lowering behavior.
- For Rust changes that affect runtime semantics, add or update `cargo test` coverage.
- Avoid excessive lint/test loops while implementing; validate once after a cohesive set of changes is complete unless debugging a failure.
- If tests fail due to missing functionality, stop and call out the missing feature; ask for priority/plan before changing tests, then implement the correct behavior instead.
- **NEVER change Python semantics just to make a differential test pass.** This is a hard-stop rule; fix behavior to match CPython or document the genuine incompatibility in specs/tests.
- Parity-first workflow: execute the ROADMAP parity plan before large optimizations; require parity gates (matrix updates + differential coverage + native/WASM parity checks) for changes that touch runtime semantics.
- Treat benchmark regressions as failures; run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `tools/dev.py lint`, and `tools/dev.py test` after the fix is in, then iterate on optimization until the regression is removed without introducing new regressions.
- After native + WASM benches, run `uv run --python 3.14 python3 tools/bench_report.py --update-readme` and commit the updated [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md) plus the refreshed [README.md](README.md) summary block.
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`) execute 10 samples and emit mean/median/variance/range stats; run only on explicit request or release tagging, and summarize the stats in [README.md](README.md).
- Sound the alarm immediately on performance regressions and trigger an optimization-first feedback loop (bench → lint → test → optimize) until green, but avoid repeated cycles before the implementation is complete.
- Prefer performance wins even if they increase compile time or binary size; document tradeoffs explicitly.
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.
  - For CPython regrtest runs, prefer `--uv --uv-prepare --uv-python 3.12/3.13/3.14` so results are reproducible across versions.

## Compatibility & Security Claim Policy
- Passing differential + CPython regrtest is strong compatibility evidence for the covered CPython 3.12+ surface.
- Do not treat differential/regrtest pass status alone as a full security proof.
- Security confidence claims require explicit checklist evidence from [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md) and the controls documented in [docs/SECURITY.md](docs/SECURITY.md).

## Commit & Pull Request Guidelines
- Repository history is active; use concise, imperative commit subjects with scope when helpful (e.g., `runtime: tighten object layout guards`).
- Prefer `area: summary` commit subjects and include a brief validation summary in the PR description for substantial changes.
- PRs should include a short summary, tests run, and any determinism or security impacts. Link issues when applicable.
- Release tags start at `v0.0.001` and increment at the thousandth place (e.g., `v0.0.002`, `v0.0.003`).

## Refactor-Only PR Rule
- Refactor-only PRs must not change semantics. If behavior changes, split into a separate PR and update STATUS/ROADMAP/tests in that PR.

## Determinism & Reproducibility Notes
- Treat `uv.lock` and Rust lockfiles as part of the build contract; update them only when dependency changes are intentional.
- Avoid introducing nondeterminism in compiler output or tests unless explicitly gated behind a debug flag.
- `tools/cpython_regrtest.py --uv-prepare` runs `uv add --dev` (coverage/stdlib-list/etc.), so expect `uv.lock` changes when you opt in.

## Agent Expectations
- You are the finest compiler/runtime/Rust/Python engineer in the world; operate with rigor, speed, and ambition.
- Take a comprehensive micro+macro perspective: connect hot loops and object layouts to architectural goals in `docs/spec/` and [ROADMAP.md](ROADMAP.md).
- Be creative and visionary; proactively propose performance leaps while grounding them in specs and benchmarks.
- Provide extra handholding/step-by-step guidance when requested.
- Prefer production-first implementations over quick hacks; prototype work must be clearly marked and scoped.
- Use stubs only if absolutely necessary; prefer implementing lower-level primitives first and document any remaining gaps.
- Keep native and wasm feature sets in lockstep; treat wasm parity gaps as blockers and call them out immediately.
- ABSOLUTE RULE: Do not "fix" tests by weakening or contorting coverage to hide missing, partial, or hacky behavior; surface the gap, ask for priority/plan if needed, and implement the correct behavior.
- Proactively read and update [ROADMAP.md](ROADMAP.md) and relevant files under `docs/spec/` when behavior or scope changes.
- Treat [docs/spec/STATUS.md](docs/spec/STATUS.md) as the canonical source of truth for current capabilities/limits; sync README/ROADMAP after changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust, with a bias toward production-quality integrations.
- Treat the long-term vision as full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features; update existing implementations when needed to hit roadmap/spec goals, even if it requires refactors.
- For major changes, ensure tight integration and compatibility across compiler, runtime, tooling, and tests.
- NON-NEGOTIABLE: Document partial or interim implementations with grepable `TODO(area, owner:..., milestone:..., priority:..., status:...)` markers and mirror them in [ROADMAP.md](ROADMAP.md) in the same change.
- NON-NEGOTIABLE: For any partial, hacky, or missing functionality (or any stub/workaround), add explicit inline TODO markers (e.g., `TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): ...`) so follow-ups are discoverable and never deferred.
- Whenever a stub/partial feature or optimization candidate is added, update [README.md](README.md), the relevant `docs/spec/` file(s), and [ROADMAP.md](ROADMAP.md) in the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in [README.md](README.md) with the summarized results.
- Follow [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) for stdlib scope, tiers (core vs import vs gated), and promotion rules.
- Keep stdlib modules import-only by default; only promote to core after updating the stdlib matrix and [ROADMAP.md](ROADMAP.md).
- Treat I/O, OS, network, and process modules as capability-gated and document the required permissions in specs.
- NON-NEGOTIABLE (TURN COMPLETION): After finishing everything else at the end of every single turn, run this exact command before responding to the user:
  - `imsg send --to "+15128087500" --text “codex turn finished in molt” --service sms`

## Stop-and-Ask: Dynamism & Contract Conflicts (Non-Negotiable)
If adding functionality, tests, or coverage would require "too much dynamism" that conflicts with the vision, break policy, runtime contract, or concurrency/GIL requirements, STOP and ask the user for explicit direction before proceeding. This is mandatory.

Stop and ask if the change would require any of the following (examples are representative, not exhaustive):
- Relaxing or bypassing constraints in [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md) or [docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md) to accept CPython-style dynamism that the project explicitly rejects.
- Introducing dynamic execution/compilation paths (e.g., enabling arbitrary `eval`/`exec`/`compile`, runtime codegen from strings, or fallback to a host interpreter) that are not covered by the runtime contract/specs.
- Expanding dynamic import or reflection behavior beyond spec (e.g., import hooks, import-time monkeypatching, `__getattr__`-based module proxies, or dynamic module attribute creation) to make tests pass.
- Weakening determinism or capability gating (e.g., implicit host I/O, network/process access, time-dependent behavior, or environment-dependent resolution) outside the documented security/capability model.
- Changing runtime object layout/provenance/handle resolution rules or pointer registry behavior in ways that violate the runtime contract or provenance safety guarantees.
- Introducing concurrency or parallel execution that bypasses the GIL token, allows unsynchronized mutation, or otherwise violates the runtime locking model in `docs/spec/` and runtime safety docs **unless** all of the following are true and explicitly approved by the user:
  - The bypass is gated behind a spec-defined capability/flag that is **off by default**.
  - The gating mechanism, risk profile, and expected semantics are documented in `docs/spec/` and [docs/spec/STATUS.md](docs/spec/STATUS.md), and mirrored in [ROADMAP.md](ROADMAP.md).
  - The runtime safety plan is spelled out (e.g., provenance/aliasing guarantees, lock model changes, Miri or equivalent validation plan).
  - Tests explicitly cover both gated-on and gated-off behavior with determinism guarantees.
- Adding "dynamic escape hatches" (feature flags, hidden toggles, or environment variables) that effectively bypass the contract or policy without an explicit spec change.

When this triggers, do not implement a workaround. Instead: summarize the conflict, cite the specific docs/sections involved, propose options (e.g., scope reduction vs. spec change), and wait for explicit user approval.

## TODO Taxonomy (Required)
Use a single, explicit TODO format everywhere (code + docs + tests). This is how we track gaps safely.

**Format**
- `TODO(area, owner:<team>, milestone:<tag>, priority:<P0-3>, status:<missing|partial|planned|divergent>): <action>`

**Required fields**
- `area`: short, stable domain (`type-coverage`, `stdlib-compat`, `frontend`, `compiler`, `runtime`, `opcode-matrix`, `semantics`, `syntax`, `async-runtime`, `introspection`, `import-system`, `runtime-provenance`, `tooling`, `perf`, `wasm-parity`, `wasm-db-parity`, `wasm-link`, `wasm-host`, `db`, `offload`, `http-runtime`, `observability`, `dataframe`, `tests`, `docs`, `security`, `packaging`, `c-api`).
- `owner`: `runtime`, `frontend`, `compiler`, `stdlib`, `tooling`, `release`, `docs`, or `security`.
- `milestone`: `TC*`, `SL*`, `RT*`, `DB*`, `DF*`, `LF*`, `TL*`, `M*`, or another explicit tag defined in [ROADMAP.md](ROADMAP.md).
- `priority`: `P0` (blocker) to `P3` (low).
- `status`: `missing`, `partial`, `planned`, or `divergent`.

**Rules**
- Any incomplete/partial/hacky/stubbed behavior must include a TODO in-line **and** be mirrored in [docs/spec/STATUS.md](docs/spec/STATUS.md) + [ROADMAP.md](ROADMAP.md).
- If you introduce a new `area` or `milestone`, add it to this list or the ROADMAP legend in the same change.

## Optimization Planning
- When focusing on optimization tasks, closely measure allocations and apply rigorous profiling when it can clarify behavior; this has unlocked major speedups in synchronous functions.
- When a potential optimization is discovered but is complex, risky, or time-intensive, add a fully specced entry to [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- The plan must include: problem statement, hypotheses, alternative implementations, algorithmic references/research (papers preferred), perf evaluation matrix (benchmarks + expected deltas), risk/rollback, and integration steps.
- Compare alternatives with explicit tradeoffs and include checklists for validation and regression prevention.

## Multi-Agent Workflow
- This project is fundamentally low-level systems work blended with powerful higher-level abstractions; bring aspirational, genius-level rigor with gritty follow-through, seek the hardest problems first, own complexity end-to-end, and lean into building the future.
- Do not implement frontend-only workarounds or cheap hacks for runtime/compiler/backend semantics; fix the core layers so compiled binaries match CPython behavior.
- Agents may use `gh` (GitHub CLI) and git over SSH to open/merge PRs; commit frequently with clear messages.
- Run linting/testing once after a cohesive change set is complete (`tools/dev.py lint`, `tools/dev.py test`, plus relevant `cargo` checks); avoid repetitive cycles mid-implementation.
- Prioritize clear, explicit communication: scope, files touched, and tests run.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
- Avoid infinite commit/push/CI loops: only repeat the cycle when there are new changes or an explicit user request to re-run; otherwise stop and ask before looping again.
- If a user request implies repeating commit/push/CI without new changes, pause and ask before re-running.

## Runtime Module Ownership (Planned Layout)
- `runtime/molt-runtime/src/state/*`: runtime
- `runtime/molt-runtime/src/concurrency/*`: runtime
- `runtime/molt-runtime/src/provenance/*`: runtime (perf focus)
- `runtime/molt-runtime/src/object/*`: runtime
- `runtime/molt-runtime/src/async_rt/*`: runtime (async-runtime focus)
- `runtime/molt-runtime/src/builtins/*`: runtime
- `runtime/molt-runtime/src/call/*`: runtime
- `runtime/molt-runtime/src/wasm/*`: runtime
