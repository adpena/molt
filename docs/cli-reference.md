# Molt CLI Reference

Molt is a Python compiler that produces native binaries, WebAssembly modules, and Luau scripts.
It follows the same design principles as `cargo`, `go`, and `uv`: fast, predictable, and minimal configuration.

## Quick Start

```bash
# Build and run a Python file
molt run app.py

# Build an optimized binary
molt build app.py --release

# Deploy to Cloudflare Workers
molt deploy cloudflare app.py
```

---

## Command Reference

### Core Commands

#### `molt build`

Compile a Python file to a native binary, WASM module, or Luau script.

```bash
molt build app.py                        # Build with default settings
molt build app.py --release              # Optimized release build
molt build app.py --target wasm          # Build for WebAssembly
molt build app.py --target luau          # Build for Luau/Roblox
molt build --module mypackage            # Build a package by module name
molt build app.py --output dist/app      # Custom output path
molt build app.py --profile cloudflare   # Platform-optimized build
```

| Flag | Description |
|------|-------------|
| `--target TARGET` | Build target: `native` (default), `wasm`, `luau`, or a target triple (e.g. `aarch64-unknown-linux-gnu`). |
| `--release` | Optimized release build (alias for `--build-profile release`). |
| `--module MODULE` | Entry module name. Uses `pkg.__main__` when present. |
| `--output PATH` | Output path for the artifact. |
| `--out-dir DIR` | Output directory for final artifacts. |
| `--profile PLATFORM` | Deployment platform profile: `cloudflare`, `browser`, `wasi`, `fastly`. Sets optimization defaults. |
| `--rebuild` | Disable build cache (alias for `--no-cache`). |
| `--json` | Emit JSON output for tooling integration. |
| `--verbose` | Emit verbose diagnostics. |

<details>
<summary>Advanced flags (hidden from <code>--help</code> by default)</summary>

| Flag | Description |
|------|-------------|
| `--codec {msgpack,cbor,json}` | Structured codec for parse calls. |
| `--type-hints {ignore,trust,check}` | Apply type annotations to guide lowering. |
| `--fallback {error,bridge}` | Fallback policy for unsupported constructs. |
| `--type-facts PATH` | Path to type facts JSON from `molt check`. |
| `--pgo-profile PATH` | Path to PGO profile artifact for optimization hints. |
| `--pgo-collect` | Instrument the binary to collect PGO counters at runtime. |
| `--emit {bin,obj,wasm}` | Select artifact format. |
| `--linked / --no-linked` | Emit linked WASM artifact alongside output. |
| `--split-runtime` | Produce separate runtime and app WASM modules. |
| `--wasm-opt-level {Oz,O3}` | WASM optimization profile: `Oz` for size, `O3` for speed. |
| `--precompile` | Produce a precompiled `.cwasm` for faster startup. |
| `--snapshot` | Generate snapshot header for sub-millisecond cold starts. |
| `--portable` | Use baseline ISA (no host-specific CPU features). |
| `--deterministic / --no-deterministic` | Require deterministic inputs (lockfiles). |
| `--build-profile {dev,release}` | Build profile for backend/runtime. |
| `--stdlib-profile {full,micro}` | Runtime stdlib profile (`micro` for smallest binary). |
| `--wasm-profile {full,pure}` | WASM import profile. |
| `--cache / --no-cache` | Enable/disable build cache. |
| `--cache-dir DIR` | Override build cache directory. |
| `--trusted / --no-trusted` | Disable capability checks. |
| `--capabilities SPEC` | Capability profiles/tokens or path to manifest. |
| `--sysroot PATH` | Sysroot path for native cross-compilation. |
| `--lib-path DIR` | Additional Python package search directories (repeatable). |
| `--emit-ir PATH` | Write lowered IR JSON to a file. |
| `--diagnostics / --no-diagnostics` | Enable compile diagnostics. |
| `--diagnostics-file PATH` | Path for diagnostics JSON output. |
| `--diagnostics-verbosity {summary,default,full}` | Stderr diagnostics detail level. |
| `--respect-pythonpath / --no-respect-pythonpath` | Include PYTHONPATH entries as module roots. |
| `--runtime-feedback PATH` | Path to runtime feedback artifact for hot-function hints. |

</details>

#### `molt run`

Build and execute a Python program. Supports native, WASM (via wasmtime), and Luau (via lune) targets.

```bash
molt run app.py                          # Build and run natively
molt run app.py --release                # Optimized build and run
molt run app.py --target wasm            # Build and run with wasmtime
molt run app.py --target luau            # Build and run with lune
molt run app.py -- --arg1 val            # Pass args to your script
molt run --module mypackage              # Run a package
```

| Flag | Description |
|------|-------------|
| `--target TARGET` | Build target: `native`, `wasm`, `luau`, or a target triple. |
| `--release` | Optimized release build. |
| `--module MODULE` | Entry module name. |
| `--profile {dev,release}` | Build profile (default: `dev`). |
| `--rebuild` | Disable build cache. |
| `--timing` | Emit timing summary (compile + run). |
| `--capabilities SPEC` | Capability profiles or manifest path. |
| `--trusted / --no-trusted` | Disable capability checks. |
| `--build-arg ARG` | Extra args passed to `molt build` (repeatable). |

#### `molt test`

Discover and run test suites. Supports Molt's built-in dev suite, CPython differential tests, and pytest.

```bash
molt test                                # Run the default dev test suite
molt test --suite diff                   # Run differential tests vs CPython
molt test --suite pytest                 # Run tests with pytest
molt test tests/test_math.py             # Run a specific test file
molt test --suite diff --profile release # Diff tests with release builds
```

| Flag | Description |
|------|-------------|
| `--suite {dev,diff,pytest}` | Test suite to run (default: `dev`). |
| `--python-version VERSION` | Python version for diff suite (e.g. `3.13`). |
| `--profile {dev,release}` | Build profile for Molt builds in diff suite. |
| `--trusted / --no-trusted` | Disable capability checks. |

#### `molt bench`

Run performance benchmarks using the native or WASM harness.

```bash
molt bench                               # Run all benchmarks
molt bench --wasm                        # Run WASM benchmarks
molt bench --script bench/fib.py         # Benchmark a custom script
molt bench -- --filter sort              # Pass args to bench tool
```

| Flag | Description |
|------|-------------|
| `--wasm` | Use the WASM bench harness. |
| `--script PATH` | Benchmark a custom script path (repeatable). |

#### `molt check`

Analyze a Python file or package and emit type facts without compiling.
Type facts can be fed into `molt build --type-facts` for guided specialization.

```bash
molt check src/app.py                    # Type-check a file
molt check src/                          # Type-check a package directory
molt check src/app.py --strict           # Emit strict-tier type facts
molt check src/app.py --output facts.json # Write facts to custom path
```

| Flag | Description |
|------|-------------|
| `--output PATH` | Output path for type facts JSON (default: `type_facts.json`). |
| `--strict` | Mark facts as trusted (strict tier). |
| `--deterministic / --no-deterministic` | Require deterministic inputs. |

#### `molt deploy`

Build and deploy to a target platform. Automatically sets the correct build target and optimization defaults.

```bash
molt deploy cloudflare app.py            # Deploy to Cloudflare Workers
molt deploy roblox app.py                # Deploy to Roblox Studio
molt deploy cloudflare app.py --release  # Optimized production deploy
molt deploy cloudflare app.py --dry-run  # Build only, skip wrangler
molt deploy roblox app.py --roblox-project ./my-game
```

| Flag | Description |
|------|-------------|
| `--release` | Optimized release build. |
| `--build-profile {dev,release}` | Build profile (default: `release`). |
| `--output PATH` | Output path for the build artifact. |
| `--out-dir DIR` | Output directory for build artifacts. |
| `--roblox-project DIR` | Roblox project directory to copy Luau output into. |
| `--wrangler-args ARGS` | Extra arguments passed to `wrangler deploy`. |
| `--dry-run` | Build only; do not run wrangler or copy to project. |
| `--build-arg ARG` | Extra args passed to `molt build` (repeatable). |

**Platforms:**

| Platform | Description |
|----------|-------------|
| `cloudflare` | Build as WASM with `--split-runtime`, deploy via wrangler. |
| `roblox` | Build as Luau, optionally copy to a Roblox project directory. |

---

### Package Commands

#### `molt package`

Bundle a distributable `.moltpkg` package from a build artifact and manifest.

```bash
molt package output.wasm manifest.json
molt package output.wasm manifest.json --output dist/app.moltpkg
molt package output.wasm manifest.json --sign
```

#### `molt publish`

Publish a `.moltpkg` to a registry (local directory or remote HTTP).

```bash
molt publish dist/app.moltpkg
molt publish dist/app.moltpkg --registry https://registry.example.com
molt publish dist/app.moltpkg --dry-run
```

#### `molt deps`

Show dependency information for the current project.

```bash
molt deps
molt deps --include-dev
molt deps --json
```

#### `molt vendor`

Vendor pure-Python dependencies into the project.

```bash
molt vendor
molt vendor --dry-run
molt vendor --output vendor/
molt vendor --extras dev
```

---

### Toolchain Commands

#### `molt clean`

Remove build artifacts and caches.

```bash
molt clean                               # Remove caches and build artifacts
molt clean --all                         # Remove everything (caches, bins, cargo)
molt clean --bins                        # Also remove compiled binaries
molt clean --cargo-target                # Also remove Cargo target/ dir
```

#### `molt doctor`

Check that the Molt toolchain is installed and configured correctly.

```bash
molt doctor
molt doctor --strict                     # Non-zero exit on missing requirements
```

#### `molt config`

Show resolved Molt configuration for the current project.

```bash
molt config
molt config --json
molt config --file src/app.py
```

#### `molt completion`

Generate shell completions.

```bash
molt completion --shell bash >> ~/.bashrc
molt completion --shell zsh >> ~/.zshrc
molt completion --shell fish > ~/.config/fish/completions/molt.fish
```

---

### Development Commands

#### `molt compare`

Build and run a Python file with both CPython and Molt, then compare output side by side.

```bash
molt compare app.py
molt compare app.py --python 3.13
molt compare app.py -- --flag
```

#### `molt diff`

Run differential tests that compare Molt output against CPython.

```bash
molt diff
molt diff tests/parity/
molt diff --python-version 3.13
```

#### `molt parity-run`

Run the entrypoint with CPython only (no Molt compilation). Useful for establishing baseline behavior.

```bash
molt parity-run app.py
molt parity-run app.py --python 3.12
```

#### `molt profile`

Profile Molt benchmarks with detailed performance instrumentation.

```bash
molt profile
molt profile -- --filter hot_loop
```

#### `molt lint`

Run Molt-specific linting checks on the project.

```bash
molt lint
```

#### `molt extension`

Build and audit C extensions compiled against `libmolt`.

```bash
molt extension build                     # Build a C extension
molt extension audit                     # Audit extension ABI compatibility
molt extension scan                      # Scan for C API usage
```

#### `molt verify`

Verify a package manifest and checksum.

```bash
molt verify --package dist/app.moltpkg
molt verify --manifest manifest.json --artifact output.wasm
molt verify --package dist/app.moltpkg --require-signature
```

---

## Configuration

Molt reads configuration from `pyproject.toml` under `[tool.molt]` or from a standalone `molt.toml`.

### `pyproject.toml`

```toml
[tool.molt]
# Default build target
target = "native"

# Default build profile
build-profile = "release"

# Capability profiles
capabilities = ["core", "fs", "net"]

# Deterministic builds (require lockfiles)
deterministic = true

[tool.molt.build]
# Build-specific overrides
target = "wasm"
release = true
codec = "msgpack"
type-hints = "trust"
fallback = "error"
split-runtime = true
stdlib-profile = "micro"
portable = false

[tool.molt.run]
# Run-specific overrides
trusted = true
timing = true

[tool.molt.test]
# Test-specific overrides
suite = "diff"

[tool.molt.deploy]
# Deploy-specific overrides
build-profile = "release"

[tool.molt.publish]
# Registry configuration
registry = "https://registry.example.com"

[tool.molt.deps]
# Dependency tier classifications
tier_a = ["attrs", "click", "pydantic", "requests", "rich"]
tier_b = ["orjson", "polars"]
native_wheels = ["cbor2", "cryptography", "msgpack"]

[tool.molt.extension]
# C extension configuration
molt_c_api_version = "0.1"
```

---

## Environment Variables

### Core

| Variable | Description |
|----------|-------------|
| `MOLT_HOME` | Root directory for Molt data (build artifacts, caches). Defaults to OS-specific app data. |
| `MOLT_CACHE` | Override the build cache directory. |
| `MOLT_BIN` | Directory for compiled binaries. |
| `MOLT_PROJECT_ROOT` | Override project root detection. |
| `MOLT_ENTRY_MODULE` | Override the entry module name. |

### Build

| Variable | Description |
|----------|-------------|
| `MOLT_HASH_SEED` | Override the hash seed for deterministic builds. |
| `MOLT_STDLIB_PROFILE` | Default stdlib profile (`full` or `micro`). |
| `MOLT_MODULE_ROOTS` | Colon-separated additional module search roots. |
| `MOLT_PORTABLE` | Set to `1` for baseline ISA codegen. |
| `MOLT_SPLIT_RUNTIME` | Set to `1` to enable split-runtime WASM by default. |
| `MOLT_DEAD_MODULE_ELIMINATION` | Set to `1` to enable dead module elimination. |
| `MOLT_BUILD_STATE_DIR` | Override the build state directory. |
| `MOLT_BUILD_LOCK_TIMEOUT` | Timeout in seconds for build lock acquisition. |
| `MOLT_SYSROOT` | Sysroot path for native linking. |
| `MOLT_CROSS_SYSROOT` | Sysroot path for cross-compilation. |
| `MOLT_CROSS_CC` | Cross-compiler path for cross-compilation. |
| `MOLT_ARCH` | Override target architecture. |
| `MOLT_MACOSX_DEPLOYMENT_TARGET` | macOS deployment target version. |

### Backend / Codegen

| Variable | Description |
|----------|-------------|
| `MOLT_BACKEND_OPT_LEVEL` | Backend optimization level. |
| `MOLT_BACKEND_REGALLOC_ALGORITHM` | Register allocation algorithm. |
| `MOLT_BACKEND_ENABLE_VERIFIER` | Enable backend IR verification. |
| `MOLT_BACKEND_PROFILE` | Backend build profile override. |
| `MOLT_BACKEND_DAEMON` | Set to `0` to disable the backend daemon. |
| `MOLT_BACKEND_DAEMON_SOCKET` | Override the daemon socket path. |
| `MOLT_BACKEND_DAEMON_START_TIMEOUT` | Timeout for daemon startup. |
| `MOLT_DISABLE_STRUCT_ELIDE` | Set to `1` to disable struct elision optimization. |
| `MOLT_DEV_LINKER` | Override the linker selection (`auto`, `mold`, `lld`). |
| `MOLT_USE_SCCACHE` | sccache mode (`auto`, `0`, `1`). |

### WASM

| Variable | Description |
|----------|-------------|
| `MOLT_WASM_DATA_BASE` | WASM data segment base address. |
| `MOLT_WASM_MIN_PAGES` | Minimum WASM memory pages. |
| `MOLT_WASM_LINK` | Set to `1` to enable WASM linking. |
| `MOLT_WASM_TABLE_BASE` | WASM table base index. |
| `MOLT_WASM_RUNTIME_DIR` | Directory for WASM runtime artifacts. |
| `MOLT_WASM_CARGO_PROFILE` | Cargo profile for WASM runtime build. |
| `MOLT_WASM_LINKED` | Set to `0` to disable linked WASM output. |
| `MOLT_EXT_ROOT` | Root directory for extension modules. |

### Frontend / Midend

| Variable | Description |
|----------|-------------|
| `MOLT_FRONTEND_TIMINGS` | Enable frontend phase timing output. |
| `MOLT_FRONTEND_PHASE_TIMEOUT` | Timeout for individual frontend phases. |
| `MOLT_FRONTEND_PARALLEL_MODULES` | Enable parallel module compilation (`0` or `1`). |
| `MOLT_FRONTEND_PARALLEL_MIN_MODULES` | Minimum module count to trigger parallelism. |
| `MOLT_MIDEND_PROFILE` | Override midend optimization profile. |
| `MOLT_MIDEND_BUDGET_MS` | Midend optimization time budget in milliseconds. |

### Diagnostics

| Variable | Description |
|----------|-------------|
| `MOLT_BUILD_DIAGNOSTICS` | Enable build diagnostics (`1` or `true`). |
| `MOLT_BUILD_DIAGNOSTICS_FILE` | Path for diagnostics JSON output. |
| `MOLT_BUILD_DIAGNOSTICS_VERBOSITY` | Diagnostics detail level. |
| `MOLT_BUILD_ALLOCATIONS` | Enable allocation tracking. |

### Timeouts

| Variable | Description |
|----------|-------------|
| `MOLT_CARGO_TIMEOUT` | Cargo build timeout. |
| `MOLT_BACKEND_TIMEOUT` | Backend compilation timeout. |
| `MOLT_LINK_TIMEOUT` | Linker timeout. |

### Registry / Publishing

| Variable | Description |
|----------|-------------|
| `MOLT_REGISTRY_TOKEN` | Bearer token for registry authentication. |
| `MOLT_REGISTRY_USER` | Username for basic auth to registry. |
| `MOLT_REGISTRY_PASSWORD` | Password for basic auth to registry. |
| `MOLT_REGISTRY_TIMEOUT` | Registry request timeout in seconds. |
| `MOLT_CODESIGN_IDENTITY` | Code signing identity for macOS codesign. |
| `MOLT_COSIGN_TLOG` | Enable transparency log upload for cosign. |

### Testing

| Variable | Description |
|----------|-------------|
| `MOLT_REGRTEST_CPYTHON_DIR` | Path to CPython source for regression tests. |

---

## Target Guide

Molt compiles Python to three backends, selected with `--target`:

### Native (default)

Produces a native binary for the host platform. Fastest execution, full system access.

```bash
molt build app.py                        # Host platform
molt build app.py --target aarch64-unknown-linux-gnu  # Cross-compile
molt build app.py --portable             # Baseline ISA for portability
```

**Cross-compilation** requires a sysroot (`--sysroot` or `MOLT_SYSROOT`) and optionally a cross-compiler (`MOLT_CROSS_CC`).

### WASM

Produces a WebAssembly module. Runs in wasmtime, browsers, or edge platforms.

```bash
molt build app.py --target wasm          # Standard WASM module
molt build app.py --target wasm --split-runtime  # Split runtime + app
molt build app.py --target wasm --profile cloudflare  # Cloudflare-optimized
molt build app.py --target wasm --precompile  # Precompiled .cwasm
```

**Size optimization:**
- `--split-runtime` splits into `app.wasm` (~50-100KB) + `molt_runtime.wasm` (~1-2MB).
- `--stdlib-profile micro` includes only core modules.
- `--wasm-opt-level Oz` (default) optimizes for size; `O3` optimizes for speed.

### Luau

Produces a Luau script for the Roblox platform.

```bash
molt build app.py --target luau          # Luau script
molt deploy roblox app.py                # Build + deploy to Roblox
```

---

## Deployment Guide

### Cloudflare Workers

Deploy Python as a WASM-powered Cloudflare Worker.

```bash
# Quick deploy
molt deploy cloudflare app.py

# Production deploy with optimizations
molt deploy cloudflare app.py --release

# Build only (inspect artifacts before deploying)
molt deploy cloudflare app.py --dry-run

# Pass extra wrangler arguments
molt deploy cloudflare app.py --wrangler-args "--env production"
```

The `cloudflare` deploy target automatically:
1. Builds with `--target wasm --split-runtime`.
2. Generates `worker.js`, `app.wasm`, `molt_runtime.wasm`, and `manifest.json`.
3. Runs `wrangler deploy` (requires `wrangler` in PATH and a `wrangler.toml`).

### Roblox

Deploy Python as Luau for Roblox experiences.

```bash
# Quick deploy
molt deploy roblox game.py

# Deploy and copy to project directory
molt deploy roblox game.py --roblox-project ./my-game

# Build only (inspect Luau output)
molt deploy roblox game.py --dry-run
```

The `roblox` deploy target automatically:
1. Builds with `--target luau`.
2. Optionally copies the Luau output to a Roblox project directory.

---

## Global Flags

Every command supports these flags:

| Flag | Description |
|------|-------------|
| `--json` | Emit machine-readable JSON output. |
| `--verbose` | Emit verbose diagnostics to stderr. |
| `-h, --help` | Show help for the command. |
