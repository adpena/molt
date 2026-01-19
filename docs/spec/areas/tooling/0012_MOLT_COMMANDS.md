# Molt CLI Commands
**Spec ID:** 0012
**Status:** Draft (implementation-targeting)
**Audience:** compiler engineers, runtime engineers, tooling owners
**Goal:** Define the canonical CLI surface for Molt, including current and planned commands.

---

## 1. Principles
- Deterministic by default.
- Commands must fail on missing lockfiles in deterministic mode.
- All commands support `--verbose` and `--json` output where practical.
- JSON output is schema-first: `schema_version`, `command`, `status`, `data`, `warnings`, `errors`.

---

## 2. Core Commands
### 2.1 `molt build`
**Status:** Implemented (native), WASM supported.

Purpose: Compile Python source to native or WASM artifacts.

Key flags:
- `--target {native,wasm,<triple>}`
- `--codec {msgpack,cbor,json}` (default: `msgpack`)
- `--type-hints {ignore,trust,check}` (default: `ignore`)
- `--type-facts <path>` (optional Type Facts Artifact from `molt check`)
- `--output <path>` (optional output path for the native binary, wasm artifact, or object file when `--emit obj`; relative paths resolve under `--out-dir` if set, otherwise the project root)
- `--out-dir <dir>` (optional output directory for artifacts and binaries; default: `$MOLT_HOME/build/<entry>` for artifacts and `$MOLT_BIN` for native binaries)
- `--emit {bin,obj,wasm}` (select which artifact to emit)
- `--linked/--no-linked` (emit `output_linked.wasm` alongside `output.wasm` when targeting WASM; requires `wasm-ld` + `wasm-tools`)
- `--linked-output <path>` (override the linked wasm output path; requires `--linked`)
- `--require-linked/--no-require-linked` (require a linked wasm output; fails if linking is unavailable and removes the unlinked artifact on success)
- `--emit-ir <path>` (dump lowered IR JSON)
- `--profile {dev,release}` (default: `release`)
- `--deterministic/--no-deterministic` (lockfile enforcement)
- `--trusted/--no-trusted` (disable capability checks for trusted native deployments)
- `--cache/--no-cache` (use `$MOLT_CACHE` for IR artifacts)
- `--cache-dir <dir>` (override the cache directory; defaults to `$MOLT_CACHE`)
- `--cache-report` (print cache hit/miss details)
- `--rebuild` (alias for `--no-cache`)
- `--capabilities <file|profile|list>` (capability manifest or profiles/tokens)
- `--pgo-profile <molt_profile.json>` (planned)
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): enable PGO profile ingestion.)

Outputs:
- `output.o` + linked binary (native, unless `--emit obj`)
- `output.wasm` (WASM)
- `output_linked.wasm` when `--linked` is enabled (single-module WASM)
- When `--require-linked` is enabled, the linked artifact becomes the primary output and the unlinked `output.wasm` is removed after linking.
- Artifacts are placed under `--out-dir` when provided; otherwise they default to `$MOLT_HOME/build/<entry>` (including `main_stub.c`).
- Native binary defaults to `$MOLT_BIN/<entry>_molt` when `--output` is not provided.
- `--emit obj` skips linking and returns only the object artifact.
- Cache reuse skips the backend compile step only; linking still runs when `--linked` is enabled. Use `--no-cache` for a full recompile.
Environment defaults:
- `MOLT_HOME` (default `~/.molt`): base directory for Molt state, including build artifacts under `build/`.
- `MOLT_BIN` (default `$MOLT_HOME/bin`): default directory for compiled native binaries.
- `MOLT_CACHE` (default OS cache, e.g. `~/Library/Caches/molt` or `$XDG_CACHE_HOME/molt`): IR artifact cache.

### 2.2 `molt check`
**Status:** Implemented.

Purpose: Generate a Type Facts Artifact (TFA) for optimization and guard reduction.

Key flags:
- `--output <path>` (default: `type_facts.json`)
- `--strict` (mark facts as trusted for strict-tier builds)
 - `ty` is used as a validator when available; failing checks block strict facts

Outputs:
- `type_facts.json`

### 2.3 `molt run`
**Status:** Implemented (initial).

Purpose: Run Python code via CPython with Molt shims for parity testing.

Key flags:
- `--python <exe|version>`
- `--no-shims`
- `--compiled` + `--build-arg <arg>`
- `--rebuild` (disable cache for `--compiled`)
- `--compiled-args` (pass argv through to compiled binary; initializes `sys.argv`).
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).

### 2.4 `molt test`
**Status:** Implemented (initial).

Purpose: Run Molt-aware test suites (`tools/dev.py test` by default, or diff/pytest suites).

Key flags:
- `--suite {dev,diff,pytest}`
- `--python-version <ver>` (diff suite)
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).

### 2.5 `molt diff`
**Status:** Implemented (initial; wraps `tests/molt_diff.py`).

Purpose: Differential testing against CPython using the Molt compiler.

Key flags:
- `--python-version <ver>`
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).

### 2.6 `molt profile`
**Status:** Implemented (initial; wraps `tools/profile.py`).

Purpose: Capture runtime traces into `molt_profile.json` for PGO and guard synthesis.

### 2.7 `molt bench`
**Status:** Implemented (initial; wraps `tools/bench.py` or `tools/bench_wasm.py`).

Purpose: Run curated benchmarks with regression tracking.

---

## 3. Packaging and Distribution
### 3.1 `molt package`
**Status:** Implemented (initial; local packaging).

Purpose: Bundle a manifest + artifact into a `.moltpkg` archive with checksum.

Key flags:
- `--deterministic/--no-deterministic`
- `--capabilities <file>`

### 3.2 `molt publish`
**Status:** Implemented (initial; local registry path).

Purpose: Copy a `.moltpkg` archive into a local registry path (signing/SBOM pending).
  (TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): signing + SBOM support for publish.)

Key flags:
- `--deterministic/--no-deterministic`
- `--capabilities <file>`

---

## 4. Tooling and Diagnostics
### 4.1 `molt lint`
**Status:** Implemented (initial; wraps `tools/dev.py lint`).

Purpose: Run repo linting and formatting checks.

### 4.2 `molt doctor`
**Status:** Implemented (initial toolchain checks).

Purpose: Validate toolchain, lockfiles, and target compatibility.

### 4.3 `molt deps`
**Status:** Implemented (initial).

Purpose: Show dependency compatibility tiers based on `uv.lock`.

### 4.4 `molt vendor`
**Status:** Implemented (initial).

Purpose: Vendor Tier A dependencies into `vendor/` with a manifest.

Key flags:
- `--include-dev`
- `--extras <name>` (include optional-dependency groups)
- `--allow-non-tier-a` (proceed with blockers)

### 4.5 `molt clean`
**Status:** Implemented (initial).

Purpose: Remove build caches (`$MOLT_CACHE`), transient build artifacts (`$MOLT_HOME/build`), and optional Cargo build artifacts.

Key flags:
- `--cargo-target` (remove Cargo `target/` artifacts in the repo root)

### 4.6 `molt config`
**Status:** Implemented (initial).

Purpose: Show merged Molt config defaults and resolved build/run/test/diff settings.

### 4.7 `molt completion`
**Status:** Implemented (initial).

Purpose: Emit shell completion scripts for bash/zsh/fish.

---

### 4.8 `molt verify`
**Status:** Implemented (initial).

Purpose: Validate package manifests and checksums (`.moltpkg` or manifest+artifact).

Key flags:
- `--require-checksum`
- `--require-deterministic`
- `--capabilities <file|profile|list>`

---

## 5. Determinism and Security Flags
All commands that produce artifacts must support:
- `--deterministic` (default on in CI; planned)
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:planned): enforce deterministic flag across artifact commands.)
- `--capabilities <file|profile|list>` (explicit capability grants; planned)
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:planned): capability manifest enforcement across CLI.)

---

## 6. Compatibility Notes
- `molt_json` is a compatibility/debug package; production codecs default to MsgPack/CBOR.
- `molt-diff` remains the source-of-truth parity harness until `molt diff` is implemented.
