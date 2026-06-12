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
covered native path, and persisted module graph/import-scan caches keyed by
compiler/tooling policy inputs. Remaining transaction work is not closed: public
importlib API validation shapes, `__package__`/`__spec__.parent`
package-context calculation, CPython `fromlist` star/`__all__` expansion, and
namespace-package edge cases still need structural reconciliation against
CPython 3.12.

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
  facts. Native builds must then publish the validated artifact, sidecar,
  package `__init__.py` chain, and existing runtime extension shim candidates
  under a deterministic `external_static_packages/<plan-digest>/` runtime root;
  generated native binaries must prepend that staged root to canonical
  `MOLT_MODULE_ROOTS` before runtime startup, and target modes without a
  runtime-custody consumer must fail closed. Final link reuse hashes those
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
  raise deterministically when the module is absent.
- Shared stdlib cache identity must use the same stdlib module-init roots as
  backend dead-function elimination. Reuse is valid only when the key, CLI
  manifest, and backend-written partition manifest sidecar match; key+manifest
  sidecars without the partition manifest are stale. The shared cache key
  includes the sorted stdlib module-symbol partition, and
  `MOLT_STDLIB_MODULE_SYMBOLS` is the canonical serialized module-symbol
  authority for that partition; all backend consumers must parse it through one
  strict parser, and malformed values must fail closed rather than reverting to
  heuristic ownership.
- Build-time graph materialization has one immutable `ImportPlan`. Entry
  planning owns runtime-import support closure; materialization owns namespace
  stubs, generated importer modules, known-module sets, allowlist snapshots, and
  graph metadata before frontend lowering consumes the graph.
- `__import__` and `importlib.import_module` share the single Rust-backed
  runtime import transaction intrinsic, `molt_importlib_import_transaction`, for
  modules present in the compiled module registry and required support surface.
  Do not reintroduce `molt_importlib_import_module`; the old
  resolved-name-only intrinsic split import authority and is intentionally
  deleted. `importlib.util.resolve_name` remains a public helper over the same
  private relative-name rules. Public argument validation must stay API-specific
  even when helpers share resolver logic; CPython 3.12 gives
  `resolve_name(".x", None)` and `import_module(".x", None)` different error
  classes.
- `importlib.import_module` has no alias side table in the Python shim. The old
  empty `_MODULE_ALIASES` map was a dead second source of truth and must not be
  restored. Frontend literal and direct-call folds for
  `importlib.import_module("literal")` must call the same
  `molt_importlib_import_transaction(name, None, None, ("*",), 0)` intrinsic as
  the public importlib shim. The frontend proves callable identity and literal
  absolute name only; runtime import success, missing-module errors,
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
- Differential import semantics: `tests/differential/stdlib/importlib_basic.py`, `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_dotted.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_import_module_relative_package_typeerror.py`, `tests/differential/stdlib/importlib_relative_import_from_package.py`, `tests/differential/stdlib/importlib_runtime_state_payload_intrinsic.py`, `tests/differential/stdlib/importlib_support_bootstrap.py`
- Focused active transaction/fromlist slice: `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_dunder_import_fromlist.py`; this is a focused regression slice for transaction/cache changes, not a replacement for the full IB2 matrix when declaring import semantics matrix-green.

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
- Complete CPython 3.12 package-context calculation for `builtins.__import__`,
  including `globals=None`, missing `__name__`, `__package__ is None`, and
  `__spec__.parent`.
- Complete CPython `fromlist` star/`__all__` expansion and namespace-package
  edge cases inside the Rust transaction while keeping compile-time graph
  discovery separate.
- Remaining namespace-package edge-case policy.
- Editable installs and dev-mode behaviors.

`__spec__` is populated for compiled modules using `importlib.machinery.ModuleSpec` with Molt loader metadata.
