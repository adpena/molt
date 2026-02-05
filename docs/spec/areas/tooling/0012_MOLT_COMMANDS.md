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
- `--module <name>` (use `name.__main__` when present)
- `--target {native,wasm,<triple>}`
- `--codec {msgpack,cbor,json}` (default: `msgpack`)
- `--type-hints {ignore,trust,check}` (default: `ignore`)
- `--type-facts <path>` (optional Type Facts Artifact from `molt check`)
- `--output <path>` (optional output path for the native binary, wasm artifact, or object file when `--emit obj`; relative paths resolve under `--out-dir` if set, otherwise the project root; directory paths use the default filename within that directory)
- `--out-dir <dir>` (optional output directory for final artifacts; intermediate build outputs stay under `$MOLT_HOME/build/<entry>`; native binaries otherwise default to `$MOLT_BIN`)
- `--sysroot <path>` (override sysroot for native linking; relative paths resolve under the project root)
- `--emit {bin,obj,wasm}` (select which artifact to emit)
- `--linked/--no-linked` (emit `output_linked.wasm` alongside `output.wasm` when targeting WASM; requires `wasm-ld` + `wasm-tools`)
- `--linked-output <path>` (override the linked wasm output path; requires `--linked`)
- `--require-linked/--no-require-linked` (require a linked wasm output; fails if linking is unavailable and removes the unlinked artifact on success)
- `--emit-ir <path>` (dump lowered IR JSON)
- `--profile {dev,release}` (default: `release`)
- `--deterministic/--no-deterministic` (lockfile enforcement)
- `--deterministic-warn/--no-deterministic-warn` (warn instead of failing on lockfile enforcement)
- `--trusted/--no-trusted` (disable capability checks for trusted native deployments)
- `--cache/--no-cache` (use `$MOLT_CACHE` for IR artifacts)
- `--cache-dir <dir>` (override the cache directory; defaults to `$MOLT_CACHE`)
- `--cache-report` (print cache hit/miss details)
- `--rebuild` (alias for `--no-cache`)
- `--respect-pythonpath/--no-respect-pythonpath` (include `PYTHONPATH` entries as module roots; default: off)
- `--capabilities <file|profile|list>` (capability manifest or profiles/tokens)
- `--pgo-profile <molt_profile.json>` (profile-guided optimization hints; expects MPA v0.1)

Outputs:
- `output.o` + linked binary (native, unless `--emit obj`)
- `output.wasm` (WASM)
- `output_linked.wasm` when `--linked` is enabled (single-module WASM)
- When `--require-linked` is enabled, the linked artifact becomes the primary output and the unlinked `output.wasm` is removed after linking.
- Intermediate artifacts (`main_stub.c`, importer stubs, `output.o` for `--emit bin`) live under `$MOLT_HOME/build/<entry>`.
- Final outputs (binary/wasm/object) are placed under `--out-dir` when provided; otherwise wasm/object outputs default to the project root and native binaries default to `$MOLT_BIN/<entry>_molt`.
- Native binary defaults to `$MOLT_BIN/<entry>_molt` when `--output` is not provided.
- `--emit obj` skips linking and returns only the object artifact.
- Cache reuse skips the backend compile step only; linking still runs when `--linked` is enabled. Use `--no-cache` for a full recompile.
- Cache keys are computed from the IR payload plus backend/runtime source fingerprints and rustc/RUSTFLAGS metadata; small timestamp-only changes should not invalidate the cache.
Environment defaults:
- `MOLT_HOME` (default `~/.molt`): base directory for Molt state, including build artifacts under `build/`.
- `MOLT_BIN` (default `$MOLT_HOME/bin`): default directory for compiled native binaries.
- `MOLT_CACHE` (default OS cache, e.g. `~/Library/Caches/molt` or `$XDG_CACHE_HOME/molt`): IR artifact cache.
- `MOLT_SYSROOT` / `MOLT_CROSS_SYSROOT`: default sysroot path for native linking (overridden by `--sysroot`).
- `MOLT_CARGO_TIMEOUT`: optional timeout in seconds for cargo build steps (backend/runtime/wasm).
- `MOLT_BACKEND_TIMEOUT`: optional timeout in seconds for backend compilation (IR -> object/wasm).
- `MOLT_LINK_TIMEOUT`: optional timeout in seconds for native linker invocations (cc/zig).
- `MOLT_BACKEND_PROFILE`: override backend binary profile (`release` default; set `dev` for debugging).

Deterministic enforcement:
- Requires `uv` and `cargo` on PATH.
- Runs `uv lock --check` to verify `uv.lock` is current.
- Runs `cargo metadata --locked` to verify `Cargo.lock` is current.
- When `--deterministic-warn` is set, lockfile verification errors become warnings.

### 2.2 `molt check`
**Status:** Implemented.

Purpose: Generate a Type Facts Artifact (TFA) for optimization and guard reduction.

Key flags:
- `--output <path>` (default: `type_facts.json`)
- `--strict` (mark facts as trusted for strict-tier builds)
- `--deterministic/--no-deterministic`
- `--deterministic-warn/--no-deterministic-warn`
 - `ty` is used as a validator when available; failing checks block strict facts

Outputs:
- `type_facts.json`

### 2.3 `molt run`
**Status:** Implemented (compiled-by-default).

Purpose: Compile with Molt and run the native binary.

Key flags:
- `--module <name>` (use `name.__main__` when present)
- `--build-arg <arg>` (forwarded to `molt build`)
- `--rebuild` (disable build cache for the compiled run)
- `--timing` (emit compile/run timing)
- `--capabilities <file|profile|list>` (capability profiles/tokens or manifest path)
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).
- Script args are forwarded by default; use `--` to separate.

### 2.4 `molt compare`
**Status:** Implemented (initial).

Purpose: Compare CPython vs Molt compiled output and timings (separates build vs run).

Key flags:
- `--python <exe|version>`
- `--module <name>` (use `name.__main__` when present)
- `--build-arg <arg>` + `--rebuild` (forward to the Molt build)
- `--capabilities <file|profile|list>`
- `--trusted/--no-trusted`

### 2.5 `molt test`
**Status:** Implemented (initial).

Purpose: Run Molt-aware test suites (`tools/dev.py test` by default, or diff/pytest suites).

Key flags:
- `--suite {dev,diff,pytest}`
- `--python-version <ver>` (diff suite)
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).

### 2.6 `molt diff`
**Status:** Implemented (initial; wraps `tests/molt_diff.py`).

Purpose: Differential testing against CPython using the Molt compiler.

Key flags:
- `--python-version <ver>`
- `--trusted/--no-trusted` (disable capability checks for trusted deployments).

### 2.7 `molt profile`
**Status:** Implemented (initial; wraps `tools/profile.py`).

Purpose: Capture runtime traces into `molt_profile.json` for PGO and guard synthesis.

### 2.8 `molt bench`
**Status:** Implemented (initial; wraps `tools/bench.py` or `tools/bench_wasm.py`).

Purpose: Run curated benchmarks with regression tracking.

Key flags:
- `--script <path>` (benchmark one or more custom scripts; repeatable)

---

## 3. Packaging and Distribution
### 3.1 `molt package`
**Status:** Implemented (initial; local packaging).

Purpose: Bundle a manifest + artifact into a `.moltpkg` archive with checksum.

Key flags:
- `--deterministic/--no-deterministic`
- `--deterministic-warn/--no-deterministic-warn`
- `--capabilities <file>`
- `--require-signature/--no-require-signature` (require a package signature)
- `--verify-signature/--no-verify-signature` (verify signatures before publishing)
- `--trusted-signers <path>` (trust policy for allowed signers)
- `--signer {auto,cosign,codesign}` (verification tool)
- `--signing-key <path>` (cosign key for verification; or set `COSIGN_KEY`)
- `--sbom/--no-sbom` (emit SBOM sidecar)
- `--sbom-output <path>` (override SBOM sidecar path)
- `--sbom-format cyclonedx|spdx` (SBOM format)
- `--signature <path>` (attach an existing signature file)
- `--signature-output <path>` (override signature sidecar path)
- `--sign/--no-sign` (sign the artifact via cosign/codesign)
- `--signer {auto,cosign,codesign}` (select signing tool)
- `--signing-key <path>` (cosign key path; or `COSIGN_KEY`)
- `--signing-identity <id>` (codesign identity; or `MOLT_CODESIGN_IDENTITY`)

Outputs:
- `dist/<name>-<version>-<target>.moltpkg` (default) containing `manifest.json`, `artifact/*`, plus `sbom.json` and `signature.json` when enabled.
- Sidecar files `<artifact>.sbom.json` and `<artifact>.sig.json` next to the package output.
- `<artifact>.sig` sidecar when `--signature` is provided or when `--sign` uses cosign.
Environment:
- `COSIGN_KEY`: cosign key path
- `MOLT_CODESIGN_IDENTITY`: codesign identity
- `MOLT_COSIGN_TLOG=1`: upload signatures to the transparency log (default off)

### 3.2 `molt publish`
**Status:** Implemented (local + remote HTTP(S) registry).

Purpose: Copy a `.moltpkg` archive into a local registry path or upload to an HTTP(S)
registry. When the registry URL ends with `/` (or has no path), the package filename
is appended; otherwise the URL is treated as the destination object. SBOM/signature
sidecars are copied/uploaded when present.
Remote uploads use HTTP `PUT` per artifact and expect a 2xx response.
Remote publishes default to enforcing signature verification + trust policy (disable
explicitly with `--no-require-signature`/`--no-verify-signature` for local/dev flows).

Key flags:
- `--registry <path|url>`
- `--registry-token <token>` (or `MOLT_REGISTRY_TOKEN`; `@path` to read from file)
- `--registry-user/--registry-password` (or `MOLT_REGISTRY_USER`/`MOLT_REGISTRY_PASSWORD`)
- `--registry-timeout <seconds>` (or `MOLT_REGISTRY_TIMEOUT`)
- `--deterministic/--no-deterministic`
- `--deterministic-warn/--no-deterministic-warn`
- `--require-signature/--no-require-signature`
- `--verify-signature/--no-verify-signature`
- `--trusted-signers <path>`
- `--signer {auto,cosign,codesign}`
- `--signing-key <path>`
- `--capabilities <file>`
- `--require-signature/--no-require-signature`
- `--verify-signature/--no-verify-signature`
- `--trusted-signers <file>`

Environment:
- `MOLT_REGISTRY_TOKEN`
- `MOLT_REGISTRY_USER`
- `MOLT_REGISTRY_PASSWORD`
- `MOLT_REGISTRY_TIMEOUT`
- `MOLT_TRUSTED_SIGNERS`
- `MOLT_REQUIRE_SIGNATURE`
- `MOLT_VERIFY_SIGNATURE`

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
- `--deterministic/--no-deterministic`
- `--deterministic-warn/--no-deterministic-warn`

### 4.5 `molt clean`
**Status:** Implemented (initial).

Purpose: Remove Molt caches (`$MOLT_CACHE`), Molt build artifacts (`$MOLT_HOME/build`), Molt binaries (`$MOLT_BIN`), repo-local artifacts (vendor/logs/output*.wasm/cache dirs), and optional Cargo build artifacts.

Key flags:
- `--cache/--no-cache`
- `--artifacts/--no-artifacts`
- `--bins/--no-bins`
- `--repo-artifacts/--no-repo-artifacts` (skips virtualenvs by default)
- `--include-venvs` (include virtualenv caches when cleaning repo artifacts)
- `--cargo-target/--no-cargo-target` (remove Cargo `target/` artifacts in the repo root)
- `--all` (enable all cleanup targets, including `target/`)

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
- `--require-signature/--no-require-signature` (require a package signature)
- `--verify-signature/--no-verify-signature` (verify signatures when present)
- `--trusted-signers <path>` (trust policy for allowed signers)
- `--signer {auto,cosign,codesign}` (verification tool)
- `--signing-key <path>` (cosign key for verification; or set `COSIGN_KEY`)

Trust policy file (TOML/JSON):
- `cosign.keys`: list of trusted cosign key SHA-256 digests (hex or `sha256:<hex>`).
- `cosign.certificates`: list of substrings to match against cosign certificate PEM.
- `codesign.team_ids`: list of trusted Apple Team IDs.
- `codesign.identifiers`: list of trusted bundle identifiers.
- `codesign.authorities`: list of trusted authority strings.

---

### 4.9 CPython regrtest harness (`tools/cpython_regrtest.py`)
**Status:** Implemented (initial).

Purpose: Run CPython's regression test suite against Molt with reporting,
coverage, and stdlib matrix exports.

Key flags:
- `--clone` (fetch CPython checkout when missing)
- `--molt-cmd <cmd...>` (command used by `tools/molt_regrtest_shim.py` to run each test file; defaults to `python -m molt.cli run`)
- `--molt-capabilities <csv>` (comma-separated `MOLT_CAPABILITIES` for Molt test runs; default `fs.read,env.read`)
- `--molt-shim <path>` (override the shim path)
- `--skip-file <path>` (skip list, one module per line)
- `--coverage` (coverage run + HTML/JSON output)
- `--rust-coverage` (run `cargo llvm-cov` for Rust runtime coverage)
- `--uv --uv-python <ver> --uv-prepare` (use uv run + install Python + add deps)
- `--diff/--no-diff` + `--diff-path` (run Molt differential suite alongside regrtest)
- `--diff-python-version <ver>` (override diff target version)
- `--type-matrix-path` / `--semantics-matrix-path` (override matrix sources)
- `--core-only --core-file <path>` (run curated core-only list via regrtest `--fromfile`)

Outputs:
- Logs under `logs/cpython_regrtest/<timestamp>/`
- Per-run `summary.json` + `summary.md` (plus root `summary.json`/`summary.md`)
- `junit.xml` (regrtest results)
- `stdlib_matrix.json`/`.csv`
- `diff_summary.json`/`.md` and `type_semantics_matrix.json`/`.md`
- Coverage artifacts when enabled
- Rust coverage artifacts under `rust_coverage/` when enabled
- Multi-version runs clone under `third_party/cpython-<ver>/`

Notes:
- The shim treats `MOLT_COMPAT_ERROR` results as skipped and records the reason
  in `junit.xml`.
- The shim sets `MOLT_PROJECT_ROOT` to the Molt repo so compiled runs link
  against `target/<profile>/libmolt_runtime.a` even when test sources live
  under `third_party/`.
- The shim sets `MOLT_MODULE_ROOTS` and `MOLT_REGRTEST_CPYTHON_DIR` to the
  CPython `Lib` directory so `test.*` resolves without polluting host
  `PYTHONPATH`.
- `--coverage` combines host regrtest coverage with Molt subprocess coverage;
  use a Python-based `--molt-cmd` to capture Molt coverage.
- The shim forwards interpreter flags from regrtest to the Molt command.
- `tools/cpython_regrtest_skip.txt` currently skips `test_future_stmt` until
  dynamic execution builtins (`eval`/`exec`/`compile`) land.

## 5. Determinism and Security Flags
All commands that produce artifacts must support:
- `--deterministic` (default on in CI; planned; enforced for build/check/package/publish/vendor)
- `--deterministic-warn` (warn instead of failing on lockfile enforcement)
- `--capabilities <file|profile|list>` (explicit capability grants; supports allow/deny/packages/effects manifests).

---

## 6. Compatibility Notes
- `molt_json` is a compatibility/debug package; production codecs default to MsgPack/CBOR.
- `molt-diff` remains the source-of-truth parity harness until `molt diff` is implemented.
