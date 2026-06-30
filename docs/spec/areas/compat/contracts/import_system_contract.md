# Import System And Modules
**Spec ID:** 0213
**Status:** Draft
**Priority:** P1
**Audience:** compiler engineers, runtime engineers, tooling engineers
**Goal:** Define Molt's import system, module objects, and deterministic
resolution rules.

---

## 1. Scope
This spec defines:
- module loading and caching,
- import resolution and `sys.path` policy,
- package/module metadata expectations,
- compatibility boundaries for dynamic imports.

Implemented: project-root/package builds, relative imports (explicit and
implicit) with deterministic package resolution for the currently lowered
paths, `__init__` handling, covered namespace-package stubs/basics, a Rust
import transaction for the active importlib/`builtins.__import__` runtime paths,
ordinary source import payload lowering for the focused active paths,
transaction-owned graph-proven `fromlist` child auto-import/binding for the
covered native path, static package `__all__` child auto-import for source
`from package import *`, CPython 3.12 package-context resolution for the covered
relative `builtins.__import__` cases, public resolver validation for
`importlib.import_module` and `importlib.util.resolve_name`,
`FileLoader`/`SourceFileLoader.load_module` execution through the Rust
spec-execution transaction, and persisted module graph/import-scan caches keyed
by compiler/tooling policy inputs. Remaining transaction work is not closed:
public importlib API validation outside the covered import-module/resolve-name
resolver and load-module cases, dynamic/broader CPython `fromlist`
star/`__all__` expansion, and namespace-package edge cases still need structural
reconciliation against CPython 3.12.

---

## 2. Module Objects
Every module must expose:
- `__name__`, `__package__`, `__file__` (when applicable),
- `__spec__` with loader metadata,
- `__dict__` for module attributes.

Modules may be:
- compiled Molt modules,
- standard library shims,
- bridge modules (policy-gated).

---

## 3. Import Resolution

### 3.1 Deterministic `sys.path`
- `sys.path` is deterministic for a given build.
- Compiled binaries do not read ambient `PYTHONPATH` or `VIRTUAL_ENV` during
  runtime bootstrap. Build-time `--respect-pythonpath` may include
  `PYTHONPATH` entries in the compiled module graph; runtime source roots must
  be explicit through `MOLT_MODULE_ROOTS`.
- Runtime mutation of `sys.path` is allowed only when explicitly enabled.
- Resolution order is stable and documented in build metadata.

### 3.2 Allowed Forms
- `import x`, `import x as y`
- `from x import y`
- `from x import y as z`
- `from x import *` (module scope only; honors `__all__` when present, otherwise skips underscore-prefixed names)

### 3.3 Dynamic Imports
- Build-time graph discovery separates module-init closure from future runtime
  behavior. Entry modules and explicit nested-scan exceptions use full import
  discovery; transitive dependencies use module-init scanning so function-body
  lazy imports do not bloat binaries or startup.
- Build-time resolution and build-time admission are separate. Explicit
  external roots (`MOLT_MODULE_ROOTS`, `--lib-path`, respected `PYTHONPATH`, and
  auto site-packages) make modules resolvable, but only direct entry imports
  are admitted by default. Transitive closure for an external package requires
  an explicit `MOLT_EXTERNAL_STATIC_PACKAGES` package admission, and the module
  graph cache key includes that policy. Package-parent `__init__` files needed
  for an admitted leaf cannot backdoor additional external children unless the
  package is explicitly admitted.
- Explicit external package admission is also native-artifact custody. Any
  package-local `.so`/`.pyd` artifact discovered under an admitted package must
  have a nearby `extension_manifest.json` sidecar whose module name, extension
  path, extension SHA-256, ABI tag, target triple, platform tag, and
  capabilities match the actual artifact. Build admission fails closed before
  frontend lowering when the sidecar is missing or invalid, and graph, wrapper,
  and backend object-cache inputs include the artifact and manifest custody
  facts. Extension sidecars may declare `python_exports` as dotted package
  import names (for example a package-level function reexport) satisfied by the
  native artifact, and may declare `callable_exports` for direct native
  bindings with module/name, binding kind (`module_attr` or `direct_symbol`),
  ABI, required symbol for `direct_symbol`, effects, and determinism metadata.
  The native-artifact planner treats those names as the same reachability
  authority as the extension module name, and scoped lowering cache inputs carry
  the validated callable export map, so source package closure and native
  object closure cannot disagree. A callable export is the only authority that
  can lower a native package function such as
  `scipy.ndimage.distance_transform_edt` to native `invoke_ffi` ABI metadata;
  native package visibility alone must leave the call bound/dynamic. For WASM
  builds, an admitted external package containing native source or
  host-extension markers must publish wasm32 `static_link` `libmolt_source`
  artifacts before the graph scanner expands that package; raw
  NumPy/SciPy-style source roots are not a linkable substitute. Native builds
  must then publish the validated artifact,
  sidecar, package `__init__.py` chain, and existing runtime extension shim
  candidates under a deterministic `external_static_packages/<plan-digest>/`
  runtime root; generated native binaries must prepend that staged root to
  canonical `MOLT_MODULE_ROOTS` before runtime startup, and target modes without
  a runtime-custody consumer must fail closed. Final link reuse hashes those
  staged bytes, but runtime-loaded extensions are not appended to the linker
  command unless the extension ABI explicitly requires link-time linkage.
- Core stdlib closure must use the same explicit nested-scan exception set as
  normal stdlib discovery. Disabling those exceptions for bootstrap/core
  closure can leave compiled stdlib function bodies with dangling direct module
  symbols, even when the entry program never calls the affected method.
- Backend-facing IR must not contain direct calls to module-owned symbols whose
  modules are outside the materialized module graph. The CLI validates this
  immediately after IR finalization and before codegen/shared-cache
  publication, reporting the first function/op coordinates so graph-closure
  drift is reproducible without a slow link or runtime failure. Lazy
  `MODULE_IMPORT` remains a runtime boundary for optional code paths and must
  raise deterministically when the module is absent. Split-runtime isolate
  import dispatch is bounded by the explicit import set, plus required parent
  packages for those imports; graph-only runtime support modules must not become
  ambient isolate-loadable roots.
- Shared stdlib cache identity must use the same stdlib module-init roots as
  backend dead-function elimination. Reuse is valid only when the key, CLI
  manifest, and backend-written partition manifest sidecar match; key+manifest
  sidecars without the partition manifest are stale. The shared cache key
  includes the sorted stdlib module-symbol partition, and
  `MOLT_STDLIB_MODULE_SYMBOLS` is the canonical serialized module-symbol
  authority for that partition; all backend consumers must parse it through one
  strict parser, and malformed values must fail closed rather than reverting to
  heuristic ownership. A reachable-empty stdlib partition still publishes a real
  parseable object plus count, key, manifest, and partition sidecars; absence of
  functions is cache content, not permission to skip cache emission.
- Build-time graph materialization has one immutable binary image closure plan.
  The resolved entry scope records whether the image came from a CLI script,
  CLI module/package, or configured project entry; the final image root set
  includes the entry plus explicit static import roots. The import plan
  classifies declared roots, entry-reachable modules, runtime support, stdlib
  support, package parents, namespace/generated modules, and external native
  artifacts.
  `known_modules` is the whole admitted runtime import-visibility closure;
  `direct_call_modules` is the Python `module__function` link authority; and
  `compile_modules` is the sole authority for modules lowered into the binary.
  Native artifacts and package parents may appear in `known_modules` so imports
  can resolve, but they must not leak fake Python direct-call symbols unless
  they are also present in `direct_call_modules`. Native executable entrypoints
  are governed by validated `callable_exports`, not by `known_modules`.
  `from package import child` records `child` as a module binding only when
  `package.child` is itself an exact admitted module; calls such as
  `child.native_export(...)` may then route through validated
  `callable_exports`, while ordinary from-imported attributes remain attribute
  bindings and cannot mint direct-call symbols.
  Dead-module elimination may narrow `compile_modules`, but it must not mutate
  the known closure, direct-call authority, runtime import dispatch roots, or
  wrapper-cache dependency graph. Wrapper build manifests and diagnostics must
  carry and fingerprint the same closure plan, including dead-module-elimination
  mode, rather than exposing a selector-only payload or rediscovering a parallel
  graph.
- Build diagnostics carry a versioned `binary_image_analysis` envelope beside
  the closure plan. It bridges source/AST metrics, module schedule hashes,
  lowering policy, backend IR/TIR-input shape, and final artifact/link evidence
  without becoming a second semantic authority. Cache keys use stable closure
  and toolchain identities; volatile timing/allocation samples remain evidence
  that joins back to those identities, not cache-key inputs.
- The frontend `source_identity` projection is the SourceSite digest family:
  source hashes, span-derived AST site digests, binary-image module roles, and a
  semantic identity digest that IR, TIR, backend, allocation, and binary
  projections can join against without embedding raw source text or duplicating
  TIR facts. Backend IR diagnostics now carry the matching `source_sites`
  projection from the lowered op stream: attributed-op coverage, per-line
  operation counts, and a stable digest over `source_line`/column coordinates.
  `allocation_ownership` joins that same carrier to heap/stack allocation roots,
  retain/release ops, heap-exposure ops, arena eligibility, and
  finalizer-sensitive results, so memory-pressure diagnostics share the binary
  image identity without becoming another allocation authority. Those
  allocation/refcount categories are generated from `op_kinds.toml`; frontend
  `borrow`/`release` aliases canonicalize to `inc_ref`/`dec_ref` before the
  diagnostics consume them. This is evidence over the compiler carrier, not a
  second AST parser or CLI-local allocation table.
- `__import__` and `importlib.import_module` share the same Rust-owned runtime
  import transaction. Source-language imports call
  `molt_importlib_import_transaction` directly with explicit
  `name`/`globals`/`locals`/`fromlist`/`level` payloads; the public
  `importlib.import_module` shim calls the narrower
  `molt_importlib_import_module(name, package)` wrapper so CPython public API
  argument validation and relative-name resolution stay isolated from ordinary
  import payload lowering. That wrapper must delegate into the same transaction
  path after resolving the public API name; it must not become a second module
  cache or resolved-name import authority. `importlib.util.resolve_name`
  remains a public helper over the same private relative-name rules. Public
  argument validation stays API-specific even when helpers share resolver logic;
  CPython 3.12 gives
  `resolve_name(".x", None)` and `import_module(".x", None)` different error
  classes, and the covered validation matrix preserves that split for
  non-string names/packages, missing packages, empty names, and beyond-top-level
  relative imports. Current native differential evidence covers
  `import_module("math", 1)`, relative non-string package `TypeError`, relative
  `package=None` `TypeError`, package-relative success, importlib bootstrap
  submodule identity, and the public resolver-validation transcript through
  this path.
- `importlib.import_module` has no alias side table in the Python shim. The old
  empty `_MODULE_ALIASES` map was a dead second source of truth and must not be
  restored. Frontend literal and direct-call folds for
  `importlib.import_module("literal")` must call the public
  `molt_importlib_import_module(name, None)` wrapper rather than a private
  Python alias or a duplicated resolved-name shortcut. The frontend proves
  callable identity and literal absolute name only; runtime import success,
  missing-module errors,
  version-gated absence, cache custody, fromlist behavior, and module
  provenance remain owned by the Rust transaction. Rebinding through
  `importlib` or any module alias records a module-attribute mutation; while
  that attribute is unstable, both the transaction fold and cross-module static
  direct-call lowering must be refused so runtime dispatch observes the user
  replacement. Ordinary source-language imports carry explicit
  `name`/`fromlist`/`level` payloads into the same Rust transaction path;
  bootstrap and importlib implementation modules keep the private
  cycle-breaking `MODULE_IMPORT` boundary. Public importlib APIs do not bypass
  the transaction.
- Source `from ... import ...` child preparation is runtime-owned. The Rust
  transaction imports graph-proven child modules only when the parent package
  lacks the requested export, binds successful child modules onto the parent,
  preserves existing package exports, converts an absent requested child into
  the final `IMPORT_FROM` `ImportError`, and propagates dependency import errors
  without broad suppression.
- Source `from package import *` with a statically proven package `__all__`
  extends the build-time import scan with resolvable child modules named by that
  `__all__`, records those imports in persisted import-scan/module-analysis
  cache payloads, and prepares the child modules through the same Rust
  transaction `fromlist=["*"]` path before `MODULE_IMPORT_STAR` performs the
  binding copy. Dynamic `__all__` values and unresolved child names remain
  runtime-visible: unresolved names are not added to the graph and the final
  star binding raises the normal CPython-shaped missing-attribute error.
- Relative `builtins.__import__` package-context calculation is transaction
  owned for the covered CPython 3.12 cases. Non-dict `globals` raises
  `TypeError`; non-`None` `__package__` must be a string; `__package__ is None`
  consults `__spec__.parent`, preserves missing-parent `AttributeError`, and
  validates parent type; the fallback requires string `__name__`, treats a
  present non-`None` `__path__` as package context, and otherwise uses the
  dotted-name parent. Empty package context raises the normal relative-import
  no-known-parent `ImportError`.
- `FileLoader` and `SourceFileLoader.load_module` delegate module materializing,
  `sys.modules` preinsert, rollback/pop on failed new loads, existing-module
  reload no-rollback behavior, loader execution, and successful
  `sys.modules` substitution return selection to the shared Rust
  spec-execution transaction. Python loader code may still normalize arguments
  and build specs, but it must not own the module-cache transaction.
- Target/device-specific lazy imports, such as GPU backend families, must be
  represented as explicit runtime/device policy edges before they are admitted
  to the compiled graph. Non-admitted imports raise deterministic errors.

---

## 4. Caching And Reload
- Modules are cached in `sys.modules`.
- Reload behavior is explicit; `importlib.reload` is gated.
- Cache invalidation requires explicit tooling support.

## 5. Validation Anchors
Import/bootstrap changes are expected to be covered by the existing in-tree regression lanes documented in
[0008_MINIMUM_MUST_PASS_MATRIX.md](../../testing/0008_MINIMUM_MUST_PASS_MATRIX.md):

- Native bootstrap/package-entry regressions: `tests/test_native_import_bootstrap_regressions.py`
- WASM import bootstrap smoke and package-relative imports: `tests/test_wasm_importlib_smoke.py`, `tests/test_wasm_importlib_package_bootstrap.py`
- Binary image closure authority: `tests/cli/test_cli_binary_image_closure.py`
  covers configured entry-file/entry-module image scopes, CLI selector
  override, ambiguous configured selectors, import-plan closure payload
  classification, fail-closed compile modules outside the admitted closure, and
  diagnostics closure/analysis payloads, DME-aware wrapper-cache identity,
  backend IR/artifact analysis projections, and wrapper-cache static-import
  closure fingerprinting.
- Module graph authority guards: `tests/cli/test_cli_module_graph_authority.py`
  keeps wrapper build cache dependency fingerprints routed through
  `_prepare_entry_module_graph` instead of direct discovery/static-import
  rediscovery; `tests/cli/test_cli_build_inputs_authority.py` keeps entry
  selector and binary image kind resolution in the build-input authority.
- Differential import semantics: `tests/differential/stdlib/importlib_basic.py`, `tests/differential/stdlib/importlib_from_bootstrap_submodules.py`, `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_dotted.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_import_module_relative_package_typeerror.py`, `tests/differential/stdlib/importlib_relative_import_from_package.py`, `tests/differential/stdlib/importlib_runtime_state_payload_intrinsic.py`, `tests/differential/stdlib/importlib_support_bootstrap.py`
- Focused active transaction/fromlist slice: `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_dunder_import_fromlist.py`; run this slice with `tests/molt_diff.py --stdlib-profile full` because the importlib discovery path intentionally pulls full-profile `zipfile`/`csv`/compression support. This is a focused regression slice for transaction/cache changes, not a replacement for the full IB2 matrix when declaring import semantics matrix-green.
- Static package `__all__` star-child slice: `tests/cli/test_cli_import_collection.py::test_from_import_star_graph_admits_static_all_child_module`, `tests/test_native_import_star_all_regressions.py`, and `tests/differential/basic/import_star_package_all_child.py`. Keep this paired with `tests/differential/basic/import_star.py` when changing `MODULE_IMPORT_STAR`, import-scan caches, or the Rust transaction `fromlist=["*"]` path.
- Package-context slice: `tests/test_native_import_package_context_regressions.py` and `tests/differential/basic/import_dunder_package_context.py`; the differential receipt is `logs/import_dunder_package_context_diff.log` plus `logs/import_dunder_package_context_diff_results.jsonl`. Keep this paired with transaction/fromlist tests when changing `importlib_transaction_package_from_globals` or relative `__import__` resolution.
- Public importlib resolver-validation slice: `tests/test_native_importlib_public_api_regressions.py` and `tests/differential/stdlib/importlib_public_api_validation.py`; the differential receipt is `logs/importlib_public_api_validation_diff.log` plus `logs/importlib_public_api_validation_diff_results.jsonl`. Keep this paired with transaction tests when changing `molt_importlib_resolve_name`, `molt_importlib_import_module_resolve_name`, or `importlib.import_module` shim wiring.
- Load-module spec-execution slice: `tests/test_native_importlib_load_module_transaction.py`, `tests/differential/stdlib/importlib_load_module_transaction.py`, and the existing spec/module differential shard (`importlib_module_from_spec.py`, `importlib_spec_from_file.py`, `importlib_util_spec_module.py`, `importlib_util_exec_module.py`, `importlib_sourcefileloader_restricted_exec.py`). The differential receipts are `logs/importlib_load_module_transaction_diff.log`, `logs/importlib_load_module_transaction_diff_results.jsonl`, `logs/importlib_spec_execution_transaction_regression_diff.log`, and `logs/importlib_spec_execution_transaction_regression_diff_results.jsonl`.

---

## 6. Build-Time Manifest
Build emits an import manifest:
- list of resolved modules,
- module origin (compiled/stdlib/bridge),
- import scan mode and reason/profile impact for each admitted support edge,
- hash or version for each module.

This manifest is part of reproducible builds.

---

## 7. Errors
Import errors must include:
- target module name,
- resolution path attempted,
- whether the failure is policy or missing-module.

---

## 8. Open Questions
- Complete dynamic/broader CPython `fromlist` star/`__all__` expansion and
  namespace-package edge cases inside the Rust transaction while keeping
  compile-time graph discovery separate.
- Remaining namespace-package edge-case policy.
- Editable installs and dev-mode behaviors.

`__spec__` is populated for compiled modules using `importlib.machinery.ModuleSpec` with Molt loader metadata.
