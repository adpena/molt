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

---

## 2. Core Commands
### 2.1 `molt build`
**Status:** Implemented (native), WASM supported.

Purpose: Compile Python source to native or WASM artifacts.

Key flags:
- `--target {native,wasm}`
- `--codec {msgpack,cbor,json}` (default: `msgpack`)
- `--type-hints {ignore,trust,check}` (default: `ignore`)
- `--profile <molt_profile.json>` (planned)
- `--release` (planned)

Outputs:
- `output.o` + linked binary (native)
- `output.wasm` (WASM)

### 2.2 `molt run`
**Status:** Planned.

Purpose: Run Python code via the slow-path interpreter or via a debug build for parity testing.

Key flags:
- `--profile` (planned)
- `--trace` (planned)

### 2.3 `molt test`
**Status:** Planned.

Purpose: Run Molt-aware test suites (e.g., `molt-diff` parity + native tests).

### 2.4 `molt diff`
**Status:** Planned (currently `tests/molt_diff.py`).

Purpose: Differential testing against CPython using the Molt compiler.

### 2.5 `molt profile`
**Status:** Planned.

Purpose: Capture runtime traces into `molt_profile.json` for PGO and guard synthesis.

### 2.6 `molt bench`
**Status:** Planned.

Purpose: Run curated benchmarks with regression tracking.

---

## 3. Packaging and Distribution
### 3.1 `molt package`
**Status:** Planned.

Purpose: Build Molt Packages (Rust/WASM) with metadata and capability manifests.

### 3.2 `molt publish`
**Status:** Planned.

Purpose: Publish a Molt Package to a registry with signing and SBOM metadata.

---

## 4. Tooling and Diagnostics
### 4.1 `molt lint`
**Status:** Planned.

Purpose: Run repo linting and formatting checks.

### 4.2 `molt doctor`
**Status:** Planned.

Purpose: Validate toolchain, lockfiles, and target compatibility.

---

## 5. Determinism and Security Flags
All commands that produce artifacts must support:
- `--deterministic` (default on in CI)
- `--capabilities <file>` (explicit capability grants)

---

## 6. Compatibility Notes
- `molt_json` is a compatibility/debug package; production codecs default to MsgPack/CBOR.
- `molt-diff` remains the source-of-truth parity harness until `molt diff` is implemented.
