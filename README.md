# Molt

A research-grade project to compile a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM),
with strict reproducibility, rigorous testing, and staged compatibility.

> Molt = Python shedding its skin into native code.

Canonical status lives in [docs/spec/STATUS.md](docs/spec/STATUS.md) (README and [ROADMAP.md](ROADMAP.md) are kept in sync).

## Strategic Targets
- Performance: parity with or superiority to Codon on tracked benchmarks.
- Coverage/interoperability: approach Nuitka-level CPython surface coverage and
  interoperability for Molt-supported semantics, while honoring Molt vision
  constraints (determinism, capability gates, and no hidden host fallback).

## Documentation Quick Links
- Docs index (canonical navigation): [docs/INDEX.md](docs/INDEX.md)
- Spec index (full spec map): [docs/spec/README.md](docs/spec/README.md)
- Differential suite organization + run ledger: [tests/differential/INDEX.md](tests/differential/INDEX.md)
- Examples guide: [examples/README.md](examples/README.md)
- Demo guide: [demo/README.md](demo/README.md)
- Bench guides: [bench/README.md](bench/README.md), [bench/friends/README.md](bench/friends/README.md)
- Packaging guides: [packaging/README.md](packaging/README.md), [packaging/templates/linux/README.md](packaging/templates/linux/README.md)

## Optimization Program Kickoff

- Current phase: Week 1 observability complete with Week 0 baseline lock artifacts captured; next focus is Week 2 specialization + wasm stabilization clusters.
- Canonical optimization scope: [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- Canonical optimization execution log: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Latest observability artifact snapshot: [bench/results/optimization_progress/2026-02-11_week1_observability/summary.md](bench/results/optimization_progress/2026-02-11_week1_observability/summary.md).
- Baseline lock summary: [bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md](bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md).
- Current compile-throughput recovery status: stdlib mid-end functions now default to Tier C unless explicitly promoted; budget degrade checkpoints are stage-level with pre-pass evaluation; frontend layer-parallel diagnostics include stdlib-aware effective min-cost policy details.
- Stdlib integrity gate status: `tools/check_stdlib_intrinsics.py` now enforces fallback-pattern bans across all stdlib modules by default (opt-down flag: `--fallback-intrinsic-backed-only`).
- Stdlib coverage gate status: top-level + submodule CPython union coverage (3.12/3.13/3.14) is enforced by `tools/check_stdlib_intrinsics.py` against `tools/stdlib_module_union.py` (missing names, package-kind mismatches, and duplicate mappings are hard failures).
- Stdlib ratchet gate status: `tools/check_stdlib_intrinsics.py` enforces intrinsic-partial budget via `tools/stdlib_intrinsics_ratchet.json`.
- Stdlib lowering audit snapshot: `intrinsic-backed=177`, `intrinsic-partial=696`, `probe-only=0`, `python-only=0`; bootstrap/critical strict-import gates are still active blockers during ongoing lowering burn-down.
- Stdlib namespace hygiene: non-CPython top-level extras are constrained to `_intrinsics` and `test`; Molt-specific DB helpers now live in `moltlib.molt_db` (with `molt.molt_db` compatibility shim).
- Stdlib union maintenance guide: [docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md](docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md).
- Stdlib execution plan: [docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md](docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md).

## Capabilities (Current)

- **Tier 0 Structification**: Compiles typed Python classes to native structs with fixed-offset access.
- **Native Async**: Lowers `async/await` into state-machine poll loops.
- **ASGI shim**: CPython-only adapter for HTTP + lifespan; capability-gated (`capabilities.require("net")`).
- **Async iteration**: Supports `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for` (sync-iter fallback enabled for now).
- **Async context managers**: `async with` lowering for `__aenter__`/`__aexit__`.
- **Async defaults**: `anext(..., default)` awaitable creation outside `await`.
- **Cancellation tokens**: request-scoped defaults with task overrides; cooperative checks via `molt.cancelled()`.
- **Molt Packages**: First-class support for Rust-backed packages, with production wire formats (MsgPack/CBOR) and Arrow IPC for tabular data; JSON is a compatibility/debug format.
- **AOT Compilation**: Uses Cranelift to generate high-performance machine code.
- **Differential Testing**: Verified against CPython 3.12+.
- **No Host Python Runtime**: Compiled Molt binaries are fully self-contained and do not rely on a local Python installation; stdlib behavior must lower into Rust intrinsics (Python wrappers are only thin intrinsic forwarders).
- **Generic aliases (PEP 585)**: builtin `list`/`dict`/`tuple`/`set`/`frozenset`/`type` support `__origin__`/`__args__`.
- **Dict union (PEP 584)**: `dict | dict` and `dict |= dict` parity.
- **Union types (PEP 604)**: `X | Y` unions with `types.UnionType` (`types.Union` on 3.14).
- **Zip strictness (PEP 618)**: `zip(strict=...)` parity.
- **Sets**: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- **Numeric builtins**: `int()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- **BigInt fallback**: heap-backed ints for values beyond inline range.
- **Format mini-language**: conversion flags + numeric formatting for ints/floats, `__format__` dispatch, named fields in `str.format`, and f-string conversion flags.
- **memoryview**: 1D buffer protocol with `format`/`shape`/`strides`/`nbytes`, `cast`, and tuple scalar indexing.
- **String search slices**: `str.find`/`str.count`/`str.startswith`/`str.endswith` support start/end slices with Unicode-aware offsets.
- **Importable builtins**: `import builtins` binds supported builtins for compiled code.
- **Builtin function objects**: allowlisted builtins (`any`, `all`, `callable`, `repr`, `getattr`, `hasattr`, `round`, `next`, `anext`, `print`, `super`) lower to first-class functions.

## Concurrency Model (Vision)

- **CPython-correct asyncio**: single-threaded event loop semantics with deterministic ordering and structured cancellation.
- **True parallelism is explicit**: executors + isolated runtimes/actors with message passing.
- **Shared-memory parallelism is opt-in**: capability-gated and limited to explicitly safe types.
- **Runtime-first**: the compiled binary embeds the Rust runtime (event loop + I/O poller); stdlib wrappers stay thin.

## Limitations (Current)

- **Classes & object model**: C3 MRO + multiple inheritance + `super()` resolution for attribute lookup; no metaclasses or dynamic `type()` construction.
- **Attributes**: instances use fixed struct fields with a dynamic instance-dict fallback; user-defined `__getattr__`/`__getattribute__`/`__setattr__`/`__delattr__` hooks work, but object-level builtins for these are not exposed; no user-defined `__slots__` beyond dataclass lowering.
- **Dataclasses**: compile-time lowering for frozen/eq/repr/slots; no `default_factory`, `kw_only`, or `order`; runtime `dataclasses` module provides metadata only.
- **Exceptions**: `try/except/else/finally` + `raise`/reraise support; still partial vs full BaseException semantics (see [docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md](docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md)).
- **Imports**: static module graph only; relative imports resolved within known packages; no dynamic import hooks or full package resolution.
- **Stdlib**: partial shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pickle` (protocol 0 only), `pprint`, `string`, `typing`, `sys`, `os`, `gc`, `random`, `statistics` (core function surface lowered through Rust intrinsics), `test` (regrtest helpers only), `asyncio`, `threading`, `bisect`, `heapq`, `functools`, `itertools`, `zipfile`, `zipimport`, `collections`, `socket` (error classes only), `select` (error alias only); import-only stubs for `collections.abc`, `_collections_abc`, `_abc`, `_py_abc`, `_asyncio`, `_bz2`, `_weakref`, `_weakrefset`, `importlib`, `importlib.util` (dynamic import hooks pending).
- **Process-based concurrency**: spawn-based `multiprocessing` (Process/Pool/Queue/Pipe/SharedValue/SharedArray) behind capabilities; `fork`/`forkserver` map to spawn semantics; `subprocess`/`concurrent.futures` pending.
- **Reflection**: `type`, `isinstance`, `issubclass`, and `object` are supported with C3 MRO + multiple inheritance; no metaclasses or dynamic `type()` construction.
- **Async iteration**: `anext` returns an awaitable; `__aiter__` must return an async iterator (awaitable `__aiter__` still pending).
- **Asyncio**: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `Event`, `wait`, `wait_for`, `shield`, and basic `gather`; task groups and I/O adapters pending.
- **Typing metadata**: `types.GenericAlias.__parameters__` derivation from `TypeVar`/`ParamSpec`/`TypeVarTuple` is pending.
- **ASGI**: shim only (no websocket support) and not integrated into compiled runtime yet.
- **Async with**: only a single context manager and simple name binding are supported.
- **Matmul**: `@` is supported only for `molt_buffer`/`buffer2d`; other types raise `TypeError`.
- **Numeric tower**: complex supported; decimal is partial (Rust intrinsic-backed context + quantize/compare/compare_total/normalize/exp/div with `as_tuple`/`str`/`repr`/float conversions); missing int helpers (e.g., `bit_length`, `to_bytes`, `from_bytes`).
- **Format protocol**: partial beyond ints/floats; locale-aware grouping still pending (WASM uses host locale for `n`).
- **List membership perf**: `in`/`count`/`index` snapshot list elements to avoid mutation during comparisons; optimization pending.
- **memoryview**: no multidimensional slicing/sub-views; advanced buffer exports pending.
- **C-extensions**: CPython ABI loading is not supported; the primary plan is recompiled extensions against `libmolt`, with an explicit opt-in bridge as an escape hatch.
- **Runtime lifecycle**: explicit init/shutdown now clears caches, pools, and async registries, but per-thread TLS drain and worker thread joins are still pending (see [docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)).
- **Offload demo**: `molt_accel` scaffolding exists (optional dep `pip install .[accel]`), with hooks/metrics (including payload/response byte sizes), auto cancel-check detection, and shared demo payload builders; a `molt_worker` stdio shell supports sync/async runtimes plus `db_query`/`db_exec` (SQLite sync + Postgres async). The decorator can fall back to `molt-worker` in PATH using a packaged default exports manifest when `MOLT_WORKER_CMD` is unset. A Django demo scaffold and k6 harness live in `demo/` and `bench/k6/`; compiled entrypoint dispatch is wired for `list_items`, `compute`, `offload_table`, and `health` while other exports still return a clear error until compiled handlers exist. `molt_db_adapter` adds a framework-agnostic DB IPC payload builder to share with Django/Flask/FastAPI adapters.
- **DB layer**: `molt-db` includes the bounded pool, async pool primitive, SQLite connector (native-only), and an async Postgres connector with per-connection statement cache; Arrow IPC output supports arrays/ranges/intervals/multiranges via struct/list encodings and preserves array lower bounds. WASM builds now have a DB host interface with `db.read`/`db.write` gating, but host adapters/client shims remain pending.

## Install (packages)

### macOS/Linux (Homebrew)

```bash
brew tap adpena/molt
brew install molt
```

The `molt` package includes `molt-worker`. Optional minimal worker:

```bash
brew install molt-worker
```

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.

### Linux/macOS (script)

```bash
curl -fsSL https://raw.githubusercontent.com/adpena/molt/main/packaging/install.sh | bash
```

### Windows (Winget / Scoop)

```powershell
winget install Adpena.Molt
```

If winget doesn't list `Adpena.Molt` yet, the submission may still be pending. In that case,
use Scoop or the script installer below.

```powershell
scoop bucket add adpena https://github.com/adpena/scoop-molt
scoop install molt
```

Script install (PowerShell):

```powershell
irm https://raw.githubusercontent.com/adpena/molt/main/packaging/install.ps1 | iex
```

### Dev vs package installs (no collisions)

- Packaged installs keep build artifacts in `~/.molt` by default.
- For local development, use the repo directly:

```bash
PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli build examples/hello.py
```

To keep dev artifacts isolated, set a different home:

```bash
MOLT_HOME=~/.molt-dev PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli build examples/hello.py
```

## Install verification

### macOS/Linux (Homebrew or script)

```bash
molt doctor --json
```

Expected: JSON output, exit code 0.

```bash
molt build examples/hello.py
```

Expected: compiled binary under `$MOLT_BIN` (defaults to `~/.molt/bin`).

Example JSON shape (values vary):

```json
{
  "schema_version": "1.0",
  "command": "doctor",
  "status": "ok",
  "data": {
    "checks": [
      {"name": "python", "ok": true, "detail": "3.12.x (requires >=3.12)"},
      {"name": "uv", "ok": true, "detail": "<path-to-uv>"},
      {"name": "cargo", "ok": true, "detail": "<path-to-cargo>"}
    ]
  },
  "warnings": [],
  "errors": []
}
```

Failed checks include a `level` and optional `advice` list in `data.checks`.

### Windows (Winget/Scoop/script)

```powershell
molt doctor --json
```

Expected: JSON output, exit code 0.

```powershell
molt build examples\\hello.py
```

Expected: compiled binary under `%MOLT_BIN%` (defaults to `%USERPROFILE%\\.molt\\bin`).

### Common failures (doctor)

- **python**: install Python 3.12+ and reopen your terminal.
  - macOS: `brew install python@3.12`
  - Windows: `winget install Python.Python.3.12`
  - Linux: install Python 3.12+ via your package manager
- **uv** (recommended): install uv.
  - macOS: `brew install uv`
  - Windows: `winget install Astral.Uv` or `scoop install uv`
  - Linux: `curl -LsSf https://astral.sh/uv/install.sh | sh`
- **cargo/rustup**: install Rust toolchain and ensure PATH is updated.
  - macOS/Linux: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
  - Windows: `winget install Rustlang.Rustup`
  - Then: `source $HOME/.cargo/env` (macOS/Linux) or reopen your terminal (Windows)
- **clang**: install a C toolchain.
  - macOS: `xcode-select --install`
  - Linux: `sudo apt-get update && sudo apt-get install -y clang lld`
  - Windows: `winget install LLVM.LLVM` and set `CC=clang`
- **wasm-target** (optional): `rustup target add wasm32-wasip1`
- **uv.lock** / **uv.lock_fresh**: run `uv sync` or `uv lock`
- **molt-runtime**: run `cargo build --release --package molt-runtime`

## Quick start (source)

```bash
# 1. Install toolchains (Rust + Python 3.12); uv recommended for Python deps.
# 2. Install Python deps (uses uv.lock)
uv sync --python 3.12

# 3. Build the runtime
cargo build --release --package molt-runtime

# 4. Compile and run a Python script (from source)
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli build examples/hello.py
~/.molt/bin/hello_molt

# Development profile (faster local iteration)
uv run --python 3.12 python3 -m molt.cli build --profile dev examples/hello.py

# Optional: keep the binary local instead
uv run --python 3.12 python3 -m molt.cli build --output ./hello_molt examples/hello.py
./hello_molt

# Trusted host access (native only)
MOLT_TRUSTED=1 ./hello_molt
uv run --python 3.12 python3 -m molt.cli build --trusted examples/hello.py
molt run --trusted examples/hello.py

# Use JSON parsing only when explicitly requested
uv run --python 3.12 python3 -m molt.cli build --codec json examples/hello.py

# Optional: install the CLI entrypoint instead of using PYTHONPATH
uv pip install -e .
molt build examples/hello.py  # binary defaults to ~/.molt/bin (override with --output or MOLT_BIN)

# Optional: accel/decorator support
pip install .[accel]  # brings in msgpack and packaged default exports for molt_accel
export MOLT_WORKER_CMD="molt-worker --stdio --exports demo/molt_worker_app/molt_exports.json --compiled-exports demo/molt_worker_app/molt_exports.json"
```

## CLI overview

- `molt run <file.py>`: compile with Molt and run the native binary (`--trusted` disables capability checks). Use `--timing` for build/run timing; script args are forwarded by default (use `--` to separate).
- `molt build --module pkg` / `molt run --module pkg`: compile or run a package entrypoint (`pkg.__main__` when present).
- Build profiles: use `--profile dev` for local development/iteration, and `--profile release` for production validation, benchmarks, and shipping artifacts.
- Dev profile routing: `--profile dev` defaults to Cargo `dev-fast` (override with `MOLT_DEV_CARGO_PROFILE`; release override: `MOLT_RELEASE_CARGO_PROFILE`).
- Release iteration lane: use `MOLT_RELEASE_CARGO_PROFILE=release-fast` for faster release-profile compile iterations, and benchmark it with `tools/compile_progress.py --cases release_fast_cold release_fast_warm release_fast_nocache_warm`.
- Build-cache determinism: CLI runs enforce `PYTHONHASHSEED=0` by default so repeated builds share cache keys; override via `MOLT_HASH_SEED=<value>` (`MOLT_HASH_SEED=random` disables this).
- Rust compile cache: when `sccache` is installed, the CLI auto-enables it (`MOLT_USE_SCCACHE=auto`; set `MOLT_USE_SCCACHE=0` to disable). If a wrapper-level `sccache` error is detected, the CLI retries the Cargo build once without `RUSTC_WRAPPER`.
- Native backend daemon: native backend compiles run through a persistent daemon by default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift startup; tune with `MOLT_BACKEND_DAEMON_START_TIMEOUT` and `MOLT_BACKEND_DAEMON_CACHE_MB`.
- Multi-agent throughput tooling: bootstrap with `tools/throughput_env.sh --apply`, benchmark with `tools/throughput_matrix.py`, run compile KPI snapshots with `tools/compile_progress.py`, and enforce cache retention with `tools/molt_cache_prune.py`.
- Shared diff target: keep `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` (set automatically by throughput bootstrap) so diff workers reuse the same Cargo artifacts instead of triggering duplicate rebuilds.
- Diff run lock: full diff runs coordinate via `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock`; tune queue wait with `MOLT_DIFF_RUN_LOCK_WAIT_SEC` and `MOLT_DIFF_RUN_LOCK_POLL_SEC`.
- `molt build --output <path|dir>`: directory outputs use the default filename; `--out-dir` only affects final outputs (intermediates remain under `$MOLT_HOME/build/<entry>`).
- `molt compare <file.py>`: compare CPython vs Molt compiled output with separate build/run timing.
- `molt test`: run the dev test suite (wraps `uv run --python 3.12 python3 tools/dev.py test`); `--suite diff|pytest` available (`--trusted` disables capability checks).
- `molt diff <path>`: differential testing via `uv run --python 3.12 python3 tests/molt_diff.py` (`--trusted` disables capability checks).
- `molt build --target <triple> --cache --deterministic --capabilities <file|profile> --sysroot <path>`: cross-target builds with lockfile + capability checks (`--trusted` for trusted native deployments). Use `MOLT_SYSROOT` / `MOLT_CROSS_SYSROOT` for defaults.
- `molt bench` / `molt profile`: wrappers over `tools/bench.py` and `tools/profile.py` (`molt bench --script <path>` for one-off scripts).
- `molt doctor`: toolchain readiness checks (uv/cargo/clang/locks).
- `molt vendor --extras <name>`: materialize Tier A sources into `vendor/` with a manifest.
- `molt package --sign --signer cosign --signing-key <key>`: sign artifacts and emit SBOM/signature sidecars with `.moltpkg` bundles (`--signer codesign` on macOS).
- `molt package --sbom-format spdx`: emit SPDX SBOM sidecars (`cyclonedx` is the default).
- `molt verify --require-signature --verify-signature --trusted-signers <policy>`: enforce signature verification and trust policies on packages.
  Example policy: `docs/trust_policy.example.toml`.
- Vendored deps under `vendor/` are added to module roots and `PYTHONPATH` automatically (or set `MOLT_MODULE_ROOTS` explicitly).
- `molt clean`: remove build caches (`MOLT_CACHE`) and transient artifacts (`MOLT_HOME/build`).
- `molt completion --shell bash|zsh|fish`: emit shell completions.
- `molt package` / `molt publish` / `molt verify`: bundle and verify `.moltpkg` archives (local paths or HTTP(S) registry URLs); `molt package` emits CycloneDX SBOM + signature metadata sidecars and supports `--sign`.

### Accel decorator options (DX)
- `entry`: worker export name; must be present in the exports/compiled manifest (e.g., `list_items`, `compute`). Mismatch → compile-time error or runtime `InvalidInput`/`InternalError`.
- `codec`: `msgpack` preferred; must match manifest `codec_in`/`codec_out`.
- `timeout_ms`: client timeout; on timeout we send `__cancel__` and close pipes.
- `payload_builder`/`response_factory`: customize request/response shaping for your endpoint contract.
- `allow_fallback`: when True, falls back to the original view on accel failure.
- Hooks: `before_send`, `after_recv`, `metrics_hook`, `cancel_check`.
- Sample entries in demo manifests: `list_items` (msgpack), `compute` (msgpack), `offload_table` (json), `db_query` (msgpack), `db_exec` (msgpack).

## ASGI shim (CPython)

Wrap a `molt.net` handler into an ASGI app for local integration testing:

```python
from molt.asgi import asgi_adapter
from molt.net import Request, Response


def handler(request: Request) -> Response:
    return Response.text("ok")


app = asgi_adapter(handler)
```

The adapter is capability-gated and calls `capabilities.require("net")` per request.

## Documentation & Architecture

- **Developer Guide**: `docs/DEVELOPER_GUIDE.md` - Start here for codebase navigation and concepts.
- **Contributing**: `CONTRIBUTING.md` - Process and standards for contributors.

See `docs/spec/areas/` for detailed architectural decisions.
- `docs/spec/areas/core/0002-architecture.md`: IR Stack & Pipeline
- `docs/spec/areas/runtime/0003-runtime.md`: NaN-boxed Object Model & Memory Management
- `docs/spec/areas/wasm/0005-wasm-interop.md`: WASM & FFI Strategy
- `wit/molt-runtime.wit`: WASM runtime intrinsics contract (WIT)
- `docs/spec/areas/runtime/0009_GC_DESIGN.md`: Hybrid RC + Generational GC
- `docs/spec/areas/tooling/0012_MOLT_COMMANDS.md`: CLI command specification
- `docs/spec/areas/tooling/0013_PYTHON_DEPENDENCIES.md`: Dependency compatibility strategy

## Testing

### CI Parity Jobs
- **WASM parity**: CI runs a dedicated `test-wasm` job that executes `tests/test_wasm_control_flow.py` via Node.
- **Differential suite**: CI runs both lanes on CPython 3.12+ (minimum 3.12): `tests/differential/basic` (core/builtins) and `tests/differential/stdlib` (stdlib modules/submodules).

### Local Commands
- Python: `uv run --python 3.12 python3 tools/dev.py test` (runs `pytest -q` via `uv run` on Python 3.12/3.13/3.14)
- Rust: `cargo test`
- Differential (single case): `uv run --python 3.12 python3 tests/molt_diff.py <case.py>`
- Differential (lane): `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic` or `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/stdlib`
- Differential layout gate: `python3 tools/check_differential_suite_layout.py`
- Bench setup (optional): `uv sync --group bench --python 3.12`
- Codon baseline (optional): install `codon` and run benches with an arm64 interpreter on Apple Silicon (e.g., `uv run --python /opt/homebrew/bin/python3.14 python3 tools/bench.py --json-out bench/results/bench.json`); see `bench/README.md` for current skips.
- WASM build (linked): `uv run --python 3.12 python3 -m molt.cli build --target wasm --linked examples/hello.py` (emits `output.wasm` + `output_linked.wasm`; linked requires `wasm-ld` + `wasm-tools`).
- WASM build (custom linked output): `uv run --python 3.12 python3 -m molt.cli build --target wasm --linked --linked-output dist/app_linked.wasm examples/hello.py`.
- WASM build (require linked): `uv run --python 3.12 python3 -m molt.cli build --target wasm --require-linked examples/hello.py` (linked output is primary; unlinked artifact removed).
- WASM run (Node/WASI): `node run_wasm.js /path/to/output.wasm` (requires linked output; build with `--linked` or `--require-linked`).
- WASM bench: `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json` (requires `wasm-ld` + `wasm-tools`; linked output is required by default), then compare against the native CPython baselines in `bench/results/bench.json`.

## Performance & Comparisons

After major features or optimizations, run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` and
`uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`, then run
`uv run --python 3.14 python3 tools/bench_report.py --update-readme` to refresh this section with a short summary
(date/host, top speedups, regressions, and any build failures) for both native and WASM, including WASM vs CPython ratios.
Optimization backlog (see `ROADMAP.md` for tracked TODOs): wasm trampoline payload init/bulk helpers, cached task-trampoline eligibility on function headers, coroutine cancel-token reuse when safe, and cached mio websocket poll registration to avoid per-wait clones.
Install optional baselines with `uv sync --group bench --python 3.12`.
PyPy baselines use `uv run --no-project --python pypy@3.11` to bypass
`requires-python` and remain comparable.
Codon baselines require the `codon` CLI; on Apple Silicon, run the bench harness
under an arm64 interpreter so Codon can link against its runtime.
Nuitka baselines require `nuitka` (or `--nuitka-cmd "python -m nuitka"`).
Pyodide baselines require `--pyodide-cmd` (or `MOLT_BENCH_PYODIDE_CMD`) pointing
to a runner command that accepts `<script> [args...]`.
Codon skip reasons are tracked in `bench/README.md`.
For cross-version baselines, run the bench harness under each CPython version
(`uv run --python 3.12 python3 tools/bench.py --json-out bench/results/bench_py312.json`,
`uv run --python 3.13 python3 tools/bench.py --json-out bench/results/bench_py313.json`,
`uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench_py314.json`)
and summarize deltas across files.
Type-hint specialization is available via `--type-hints=trust` (no guards, fastest)
or `--type-hints=check` (guards inserted). `trust` requires clean `ty` results and
assumes hints are correct; incorrect hints are user error and may miscompile.
Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`) execute
10 samples and record mean/median/variance/range stats in the JSON output; reserve
them for release tagging or explicit requests.
`bench_deeply_nested_loop.py` is a compiler-folded toy benchmark; use
`bench_csv_parse.py` as the more realistic loop/parse workload when gauging
real-world nested-loop behavior.

<!-- BENCH_SUMMARY_START -->
Latest run: 2026-01-19 (macOS x86_64, CPython 3.14.0).
Top speedups: `bench_sum.py` 222.42x, `bench_channel_throughput.py` 26.41x, `bench_ptr_registry.py` 11.70x, `bench_sum_list_hints.py` 6.85x, `bench_sum_list.py` 6.35x.
Regressions: `bench_struct.py` 0.20x, `bench_csv_parse_wide.py` 0.26x, `bench_deeply_nested_loop.py` 0.31x, `bench_attr_access.py` 0.40x, `bench_tuple_pack.py` 0.42x, `bench_tuple_index.py` 0.42x, `bench_descriptor_property.py` 0.44x, `bench_fib.py` 0.49x, `bench_csv_parse.py` 0.50x, `bench_try_except.py` 0.88x, `bench_str_join.py` 0.93x.
Slowest: `bench_struct.py` 0.20x, `bench_csv_parse_wide.py` 0.26x, `bench_deeply_nested_loop.py` 0.31x.
Build/run failures: PyPy/Codon/Nuitka/Pyodide baseline availability and per-benchmark skips are reported in `docs/benchmarks/bench_summary.md`.
WASM run: 2026-01-19 (macOS x86_64, CPython 3.14.0). Slowest: `bench_deeply_nested_loop.py` 4.25s, `bench_struct.py` 4.14s, `bench_descriptor_property.py` 1.09s; largest sizes: `bench_channel_throughput.py` 3012.3 KB, `bench_async_await.py` 2930.8 KB, `bench_ptr_registry.py` 2080.8 KB; WASM vs CPython slowest ratios: `bench_struct.py` 11.65x, `bench_descriptor_property.py` 8.74x, `bench_async_await.py` 7.90x.
<!-- BENCH_SUMMARY_END -->

### Performance Gates
- Vector reductions (`bench_sum_list.py`, `bench_min_list.py`, `bench_max_list.py`, `bench_prod_list.py`): regression >5% fails the gate.
- String kernels (`bench_str_find.py`, `bench_str_find_unicode.py`, `bench_str_split.py`, `bench_str_replace.py`, `bench_str_count.py`, `bench_str_count_unicode.py`): regression >7% fails the gate.
- Matrix/buffer kernels (`bench_matrix_math.py`): regression >5% fails the gate.
- Any expected perf deltas from new kernels must be recorded here after the run; complex regressions move to `OPTIMIZATIONS_PLAN.md`.

Baseline microbenchmarks (2026-01-16): `bench_min_list.py` 8.84x, `bench_max_list.py` 8.73x,
`bench_prod_list.py` 6.03x, `bench_str_find_unicode.py` 4.91x, `bench_str_count_unicode.py` 1.95x.

| Benchmark | Molt vs CPython | Notes |
| --- | --- | --- |
| bench_matrix_math.py | 6.06x | buffer2d matmul lowering |
| bench_deeply_nested_loop.py | 0.37x | constant nested loop folding (regressed) |
| bench_str_endswith.py | 4.86x | string endswith fast path |
| bench_str_startswith.py | 4.88x | string startswith fast path |
| bench_str_count.py | 5.06x | string count fast path |
| bench_str_split.py | 4.24x | optimized split builder |
| bench_str_replace.py | 4.28x | SIMD-friendly replace path |
| bench_str_join.py | 2.58x | pre-sized join buffer |
| bench_sum_list.py | 10.97x | vector reduction fast path |
