# Molt Package ABI
**Spec ID:** 0018
**Status:** Draft (implementation-targeting)
**Owner:** runtime + tooling
**Goal:** Define a stable package ABI for Molt-native and WASM packages with
capability gating and determinism guarantees.

---

## 1. Package Types
### 1.1 Native packages (Rust)
- Built as Rust crates and linked into the final binary.
- Export a minimal C-ABI surface for metadata and entrypoints.

### 1.2 WASM packages
- Compiled to WASM and embedded or loaded at runtime.
- Follow `docs/spec/0400_WASM_PORTABLE_ABI.md` for buffer conventions.

## 2. ABI Versioning
- ABI version is a semantic version string (e.g., `0.1`).
- Runtime refuses packages with incompatible major versions.
- Minor versions may add optional exports or metadata fields.

## 3. Required Exports (v0.1)
All packages must export:
- `molt_pkg_manifest()` -> returns a byte buffer (MsgPack preferred, JSON ok)
  describing metadata and capabilities.
- `molt_pkg_init()` -> initializes package state; idempotent.
- `molt_pkg_shutdown()` -> optional cleanup hook.

WASM packages follow the portable ABI signatures and return status codes plus
out-parameters.

## 4. Manifest Schema (v0.1)
Required fields:
- `name`, `version`, `abi_version`
- `target` (native triple or `wasm32-unknown-unknown`/`wasm32-wasip1`)
- `capabilities`: list of required capabilities
- `deterministic`: boolean
- `effects`: conservative effect declaration (see `docs/spec/0202_FOREIGN_FUNCTION_BOUNDARY.md`)

Optional fields:
- `exports`: list of callable entrypoints
- `checksum`: package digest for verification

## 5. Capability Gating
- No ambient capabilities by default.
- Loader checks manifest capabilities against app manifest (`molt.toml` or
  `pyproject.toml`) before initialization.
- Missing capabilities cause a hard error at load time.

## 6. Determinism Rules
- Tier 0 requires `deterministic=true` packages only.
- Packages declaring `nondet` effects must be explicitly opted in by policy.
- Time/randomness/network are capability-gated and forbidden by default.

## 7. Data Encoding
- Prefer MsgPack or CBOR for package boundary payloads.
- JSON is permitted for debugging or compatibility.
- WASM packages must use ptr+len buffers per `docs/spec/0400_WASM_PORTABLE_ABI.md`.

## 8. Verification
- Package checksums must match the manifest.
- Tooling validates `abi_version`, capabilities, and determinism flags at build
  and load time.

## 9. Non-Goals
- Loading CPython C-extensions (handled only by bridge tiers).
- Implicit network or filesystem access without explicit capabilities.
