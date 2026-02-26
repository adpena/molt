# Molt Extension Build Pipeline
**Spec ID:** 0215
**Status:** Partial (cross-target + verify/publish policy integration +
runtime load-time metadata enforcement + CI native/cross-host matrix lanes landed,
including verify-policy and wasm-rejection contract checks)
**Owner:** tooling + runtime
**Goal:** Define the build, packaging, and validation pipeline for C-extensions
recompiled against `libmolt`.

---

## 1. Principles
- `libmolt` is the primary C-extension compatibility path.
- Extensions must be recompiled; no CPython ABI compatibility.
- Capability and determinism policies apply at build and load time.
- Build outputs are reproducible and verifiable.

---

## 2. CLI Surface
### 2.1 `molt extension build`
Purpose: compile a C-extension against `libmolt` and emit a Molt-compatible wheel.

Status: Implemented (initial).

Flags (implemented):
- `--project <path>` (default: cwd)
- `--out-dir <path>` (default: `dist/`)
- `--molt-abi <ver>` (default: `[tool.molt.extension].molt_c_api_version` or `MOLT_C_API_VERSION`)
- `--target <native|triple>` (`wasm` rejected for native shared-library builds)
- `--capabilities <file|list|profiles>` (override extension capability metadata)
- `--deterministic/--no-deterministic`
- `--json` / `--verbose`

Outputs:
- `.whl` tagged with `py3-molt_abi<major>-<platform_tag>`.
- `extension_manifest.json` sidecar (ABI/capability metadata + checksums).

### 2.2 `molt extension audit`
Purpose: verify that an extension declares capabilities and matches the expected ABI.

Status: Implemented (initial).

Flags (implemented):
- `--path <wheel|manifest|dir>`
- `--require-capabilities`
- `--require-abi <ver>`
- `--require-checksum`
- `--json` / `--verbose`

---

## 3. ABI Tags (Proposed)
- `molt_c_api_version`: semantic version for the `libmolt` C-API (e.g., `0.1`).
- Wheel tags add `molt` ABI markers (e.g., `molt_abi0` + target triple).
- `molt` runtime rejects extensions with mismatched ABI tags.

---

## 4. Extension Metadata
Extensions declare Molt metadata in `pyproject.toml`:

```toml
[tool.molt.extension]
molt_c_api_version = "0.1"
capabilities = ["fs.read", "net"]
determinism = "nondet"
```

Required fields:
- `molt_c_api_version`
- `capabilities`

Optional:
- `determinism` (`deterministic` or `nondet`)
- `effects` (explicit effect contract for FFI boundary)

---

## 5. Build Flow
1. Resolve `libmolt` headers and link flags.
2. Compile C/C++ sources with pinned flags for reproducibility.
3. Link against `libmolt`.
4. Run symbol audit and ABI tag validation.
5. Emit wheel + `extension_manifest.json`.

---

## 6. Determinism + Security
- Build pipeline is reproducible when `--deterministic` is enabled.
- Extensions must declare capabilities and are blocked without explicit approval.
- `molt verify` checks wheel metadata and capability policies before load.
- Runtime import/load boundaries enforce extension metadata presence and
  validation (`molt_c_api_version`/`abi_tag`, declared capabilities, and
  checksum integrity for extension payloads; wheel checksum is validated for
  archive-backed loads). Successful checks are cached with path+manifest
  fingerprints so replaced artifacts are revalidated on the next import/load.

---

## 7. Integration Points
- `molt deps` should classify extensions as Tier B when `libmolt`-compiled.
- `molt build` rejects extensions with missing or mismatched ABI tags.
- `molt verify` enforces capability allowlists for extension loads.
- CI runs an extension publish dry-run matrix (native + cross-target) covering
  `molt extension build`, `molt extension audit --require-abi`,
  `molt verify --extension-metadata`, and `molt publish --dry-run`
  for extension wheels (`linux native`, `linux cross-musl`, `macos native`).
- CI also asserts the wasm build contract (`molt extension build --target wasm*`
  must fail with an explicit unsupported-target diagnostic).

---

## 8. TODOs
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): expand cross-target extension build coverage for additional linker/sysroot variants and publish readiness checks.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:partial): extend `molt verify` extension policy gates with signature/trust policy coupling and richer diagnostics.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
