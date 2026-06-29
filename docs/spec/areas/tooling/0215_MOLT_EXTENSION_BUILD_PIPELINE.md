# Molt Extension Build Pipeline
**Spec ID:** 0215
**Status:** Partial (cross-target + verify/publish policy integration +
build-admission sidecar custody and deterministic build-artifact publication
for admitted external packages, runtime load-time metadata enforcement + CI
native/cross-host matrix lanes landed, including verify-policy and
wasm-rejection contract checks)
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
- Build-time external package admission enforces the same sidecar direction for
  `MOLT_EXTERNAL_STATIC_PACKAGES`: source-recompiled package roots such as
  NumPy/SciPy require at least one package-local native/static artifact
  candidate before module-graph discovery, and their package `__init__.py`
  sources are native runtime support custody rather than source-closure
  authority. Reachable package/subpackage artifacts
  (`.so`/`.pyd`/`.molt.wasm`/`.o`/`.a`) must have nearby
  `extension_manifest.json` metadata with matching module, extension path,
  checksum, ABI, target, platform, capabilities, optional `python_exports`
  entries that map package-level imports to the owning native artifact, and
  optional `callable_exports` entries that name direct native call bindings
  before backend dispatch. Each callable export declares `module`, `name`,
  `binding` (`module_attr` or `direct_symbol`), `abi`, optional `effects`,
  optional `deterministic`, and a required native `symbol` for `direct_symbol`.
  The `abi` token must be one of the canonical native callable ABI contracts:
  `molt.object_call_v1` for boxed object-call dispatch or
  `molt.forward_f32_v1` for the unary bytes-backed Float32Array/browser lane.
  The validated callable export map is the native ABI dispatch authority; import
  visibility through `known_modules` cannot create Python `module__function`
  symbols for native packages. WASM lowers reachable `direct_symbol`
  `molt.object_call_v1` exports into deterministic `molt_native` imports and
  direct call edges; `molt.forward_f32_v1` uses the same import namespace with
  one Float32Array payload and a boxed bytes result so browser hosts and linked
  wasm objects can share one callable-export contract. The corresponding
  `invoke_ffi` IR stores callable identity only in native callable metadata; its
  `args` vector is the ABI payload, never a synthesized Python callee/module
  attribute. `module_attr` callable
  exports are descriptive import metadata until a target owns native loader
  custody; calls to them fail closed instead of falling back to `CALL_BIND` or a
  fabricated Python function symbol. Split-runtime browser
  packages project reachable direct-symbol callable exports into
  `manifest.json` at `abi.browser_embed.native_callables.symbols` with the
  canonical ABI signature (`molt.value... -> molt.value` for object-call and
  `bytes.float32 -> bytes.float32` for `forward_f32`), and the browser embed
  rejects packaged `molt_native` imports absent from that manifest table or
  whose signature does not match the ABI token. The split-runtime package
  manifest is tree-shaken from actual `app.wasm` `molt_native` imports; any
  imported native symbol missing staged artifact-plan custody fails packaging
  before delivery. Admission also proves direct-symbol custody for static-link
  artifacts:
  `wasm_relocatable_object` artifacts must export the declared function symbol,
  and `static_archive` artifacts must list it in
  `object_closure.defined_symbols`. Sidecar object closure also carries two
  reachable symbol boards. The ABI board classifies non-`Py*`
  `undefined_symbols` as project-defined through `defined_symbols` or
  runtime-backed through `runtime_symbols` only when the symbol is present in
  the generated WASM runtime/link import surface. Unknown runtime claims and
  generated runtime imports missing `runtime_symbols` custody fail admission.
  The C/API board classifies `required_c_api_symbols` and `Py*`/NumPy
  `undefined_symbols` as runtime-backed, source-compile-only, project-defined,
  fail-fast, or missing; undefined C/API symbols cannot contain
  source-compile-only NumPy inline/macro APIs, fail-fast symbols, or unknown
  gaps. For `wasm_relocatable_object`, `object_closure.undefined_symbols` must
  exactly match the artifact's function imports: binary imports missing from the
  sidecar and stale sidecar names missing from the binary both fail admission.
  Each accepted C/API symbol is bucketed by reusable primitive class such as
  object/type lifecycle, module state, capsules, exceptions, refcount, buffer
  protocol, iterator/mapping helpers, numeric scalars, or NumPy C-API. General
  ndarray storage and multi-buffer tensor ABI custody are separate contracts.
  Reachable native-artifact tree shaking is provider-closed: filtering to the
  user's graph, explicit imports, and runtime dispatch roots must retain every
  artifact that provides a capsule required by a reachable artifact.
  Graph, wrapper-build, and backend object-cache identities include the
  validated artifact/manifest custody facts. WASM package
  admission fails closed before graph expansion when an admitted package
  contains native-source or host-extension markers but has no wasm32
  `static_link` `libmolt_source` artifact manifest; source roots alone are not
  linkable package evidence. Native builds publish the validated artifact,
  sidecar, package `__init__.py` chain, and runtime extension shim candidates
  into a deterministic `external_static_packages/<plan-digest>/` runtime root.
  Native binaries inject that staged root before runtime startup and include
  staged bytes in final link reuse fingerprints without adding runtime-loaded
  extensions to the linker command. Linked WASM builds pass staged
  `wasm_relocatable_object` and `static_archive` artifacts to `wasm-ld` as
  validated native object/archive inputs and include the staged artifact,
  manifest, and support-file bytes in the link fingerprint. Target modes
  without a runtime-custody consumer fail closed when external native artifacts
  are admitted.

---

## 7. Integration Points
- `molt deps` should classify extensions as Tier B when `libmolt`-compiled.
- `molt build` rejects source-recompiled external package admission with no
  native/static artifact candidates before module graph discovery, rejects
  WASM native-source packages without staged wasm static-link artifacts, rejects
  reachable external package extensions with missing or mismatched sidecar
  metadata before backend dispatch, uses sidecar `python_exports` to bind
  package-level imports to native artifacts, threads sidecar
  `callable_exports` into scoped lowering/cache facts and `invoke_ffi` native
  callable metadata, routes supported direct-symbol object-call exports into
  backend native import tables, fails closed when backend ABI dispatch for that
  metadata is absent, and publishes validated native artifacts plus sidecars and runtime
  shims into deterministic build artifacts for native runtime import custody.
- `molt verify` enforces capability allowlists for extension loads.
- CI runs an extension publish dry-run matrix (native + cross-target) covering
  `molt extension build`, `molt extension audit --require-abi`,
  `molt verify --extension-metadata`, and `molt publish --dry-run`
  for extension wheels (`linux native`, `linux cross-musl`, `macos native`).
- CI also asserts the wasm build contract (`molt extension build --target wasm*`
  must fail with an explicit unsupported-target diagnostic).

---

## 8. TODOs
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): expand cross-target extension build coverage for additional linker/sysroot variants and source-recompiled package publish readiness checks.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:partial): extend `molt verify` extension policy gates with signature/trust policy coupling and richer diagnostics.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
