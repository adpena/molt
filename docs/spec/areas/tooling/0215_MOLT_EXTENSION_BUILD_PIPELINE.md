# Molt Extension Build Pipeline
**Spec ID:** 0215
**Status:** Draft
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

## 2. CLI Surface (Planned)
### 2.1 `molt extension build`
Purpose: compile a C-extension against `libmolt` and emit a Molt-compatible wheel.

Flags:
- `--project <path>` (default: cwd)
- `--out-dir <path>` (default: `dist/`)
- `--molt-abi <ver>` (default: `MOLT_C_API_VERSION`)
- `--capabilities <file|list>` (capability manifest or profile list)
- `--deterministic/--no-deterministic`
- `--json` (machine-readable output)

Outputs:
- `.whl` tagged for `molt` + target triple.
- `extension_manifest.json` sidecar (capabilities + ABI metadata).

### 2.2 `molt extension audit`
Purpose: verify that an extension declares capabilities and matches the expected ABI.

Flags:
- `--path <wheel|dir>`
- `--require-capabilities`
- `--require-abi <ver>`

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

---

## 7. Integration Points
- `molt deps` should classify extensions as Tier B when `libmolt`-compiled.
- `molt build` rejects extensions with missing or mismatched ABI tags.
- `molt verify` enforces capability allowlists for extension loads.

---

## 8. TODOs
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:missing): implement `molt extension build` with `libmolt` headers + ABI tagging.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:missing): implement `molt extension audit` and wire into `molt verify`.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
