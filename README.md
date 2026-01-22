# Molt

A research-grade project to compile a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM),
with strict reproducibility, rigorous testing, and staged compatibility.

> Molt = Python shedding its skin into native code.

Canonical status lives in `docs/spec/STATUS.md` (README and ROADMAP are kept in sync).

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
- **Differential Testing**: Verified against CPython 3.12.
- **Generic aliases (PEP 585)**: builtin `list`/`dict`/`tuple`/`set`/`frozenset`/`type` support `__origin__`/`__args__`.
- **Sets**: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- **Numeric builtins**: `int()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- **BigInt fallback**: heap-backed ints for values beyond inline range.
- **Format mini-language**: conversion flags + numeric formatting for ints/floats, `__format__` dispatch, named fields in `str.format`, and f-string conversion flags.
- **memoryview**: 1D buffer protocol with `format`/`shape`/`strides`/`nbytes`, `cast`, and tuple scalar indexing.
- **String search slices**: `str.find`/`str.count`/`str.startswith`/`str.endswith` support start/end slices with Unicode-aware offsets.
- **Importable builtins**: `import builtins` binds supported builtins for compiled code.
- **Builtin function objects**: allowlisted builtins (`any`, `all`, `callable`, `repr`, `getattr`, `hasattr`, `round`, `next`, `anext`, `print`, `super`) lower to first-class functions.

## Limitations (Current)

- **Classes & object model**: C3 MRO + multiple inheritance + `super()` resolution for attribute lookup; no metaclasses or dynamic `type()` construction.
- **Attributes**: instances use fixed struct fields with a dynamic instance-dict fallback; user-defined `__getattr__`/`__getattribute__`/`__setattr__`/`__delattr__` hooks work, but object-level builtins for these are not exposed; no user-defined `__slots__` beyond dataclass lowering.
- **Dataclasses**: compile-time lowering for frozen/eq/repr/slots; no `default_factory`, `kw_only`, or `order`; runtime `dataclasses` module provides metadata only.
- **Exceptions**: `try/except/else/finally` + `raise`/reraise support; still partial vs full BaseException semantics (see `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`).
- **Imports**: static module graph only; no dynamic import hooks or full package resolution.
- **Stdlib**: partial shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pickle` (protocol 0 only), `pprint`, `string`, `typing`, `sys`, `os`, `gc`, `random`, `test` (regrtest helpers only), `asyncio`, `threading`, `bisect`, `heapq`, `functools`, `itertools`, `collections`, `socket` (error classes only), `select` (error alias only); import-only stubs for `collections.abc`, `importlib`, `importlib.util` (dynamic import hooks pending).
- **Reflection**: `type`, `isinstance`, `issubclass`, and `object` are supported with C3 MRO + multiple inheritance; no metaclasses or dynamic `type()` construction.
- **Async iteration**: `anext` returns an awaitable; `__aiter__` must return an async iterator (awaitable `__aiter__` still pending).
- **Asyncio**: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `Event`, `wait`, `wait_for`, `shield`, and basic `gather`; task groups and I/O adapters pending.
- **Typing metadata**: `types.GenericAlias.__parameters__` derivation from `TypeVar`/`ParamSpec`/`TypeVarTuple` is pending.
- **ASGI**: shim only (no websocket support) and not integrated into compiled runtime yet.
- **Async with**: only a single context manager and simple name binding are supported.
- **Matmul**: `@` is supported only for `molt_buffer`/`buffer2d`; other types raise `TypeError`.
- **Numeric tower**: complex/decimal not implemented; missing int helpers (e.g., `bit_length`, `to_bytes`, `from_bytes`).
- **Format protocol**: partial beyond ints/floats; locale-aware grouping still pending (WASM uses host locale for `n`).
- **List membership perf**: `in`/`count`/`index` snapshot list elements to avoid mutation during comparisons; optimization pending.
- **memoryview**: no multidimensional slicing/sub-views; advanced buffer exports pending.
- **Runtime lifecycle**: explicit init/shutdown now clears caches, pools, and async registries, but per-thread TLS drain and worker thread joins are still pending (see `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`).
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

- `molt run <file.py>`: run CPython with Molt shims for parity checks (`--trusted` disables capability checks).
- `molt test`: run the dev test suite (`tools/dev.py test`); `--suite diff|pytest` available (`--trusted` disables capability checks).
- `molt diff <path>`: differential testing via `tests/molt_diff.py` (`--trusted` disables capability checks).
- `molt build --target <triple> --cache --deterministic --capabilities <file|profile>`: cross-target builds with lockfile + capability checks (`--trusted` for trusted native deployments).
- `molt bench` / `molt profile`: wrappers over `tools/bench.py` and `tools/profile.py`.
- `molt doctor`: toolchain readiness checks (uv/cargo/clang/locks).
- `molt vendor --extras <name>`: materialize Tier A sources into `vendor/` with a manifest.
- `molt clean`: remove build caches (`MOLT_CACHE`) and transient artifacts (`MOLT_HOME/build`).
- `molt completion --shell bash|zsh|fish`: emit shell completions.
- `molt package` / `molt publish` / `molt verify`: bundle and verify `.moltpkg` archives (local registry only).

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
- **Differential suite**: CI runs `python tests/molt_diff.py tests/differential/basic` on CPython 3.12.

### Local Commands
- Python: `tools/dev.py test` (runs `pytest -q` via `uv run` on Python 3.12/3.13/3.14)
- Rust: `cargo test`
- Differential: `python tests/molt_diff.py <case.py>`
- Bench setup (optional): `uv sync --group bench --python 3.12` (Numba requires <3.13)
- Codon baseline (optional): install `codon` and run benches with an arm64 interpreter on Apple Silicon (e.g., `uv run --python /opt/homebrew/bin/python3.14 python3 tools/bench.py --json-out bench/results/bench.json`); see `bench/README.md` for current skips.
- WASM build (linked): `python3 -m molt.cli build --target wasm --linked examples/hello.py` (emits `output.wasm` + `output_linked.wasm`; linked requires `wasm-ld` + `wasm-tools`).
- WASM build (custom linked output): `python3 -m molt.cli build --target wasm --linked --linked-output dist/app_linked.wasm examples/hello.py`.
- WASM build (require linked): `python3 -m molt.cli build --target wasm --require-linked examples/hello.py` (linked output is primary; unlinked artifact removed).
- WASM run (Node/WASI): `node run_wasm.js /path/to/output.wasm` (prefers `*_linked.wasm` when present; disable with `MOLT_WASM_PREFER_LINKED=0`).
- WASM bench: `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`, then compare against the native CPython baselines in `bench/results/bench.json`.
- WASM linked bench: `uv run --python 3.14 python3 tools/bench_wasm.py --linked --json-out bench/results/bench_wasm_linked.json` (requires `wasm-ld` + `wasm-tools`; add `--require-linked` to fail fast when linking is unavailable).

## Performance & Comparisons

After major features or optimizations, run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` and
`uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`, then run
`uv run --python 3.14 python3 tools/bench_report.py --update-readme` to refresh this section with a short summary
(date/host, top speedups, regressions, and any build failures) for both native and WASM, including WASM vs CPython ratios.
Install optional baselines with `uv sync --group bench --python 3.12` to enable Cython/Numba
columns. PyPy baselines use `uv run --no-project --python pypy@3.11` to bypass
`requires-python` and remain comparable.
Codon baselines require the `codon` CLI; on Apple Silicon, run the bench harness
under an arm64 interpreter so Codon can link against its runtime.
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
Build/run failures: Cython/Numba baselines unavailable; Codon skipped for `bench_async_await.py`, `bench_bytearray_find.py`, `bench_bytearray_replace.py`, `bench_channel_throughput.py`, `bench_matrix_math.py`, `bench_memoryview_tobytes.py`, `bench_parse_msgpack.py`, `bench_ptr_registry.py`, and 2 more.
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
