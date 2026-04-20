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

TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery, `__init__` handling, namespace packages, deterministic dependency graph caching).
Implemented: relative imports (explicit and implicit) with deterministic package resolution, honoring `__package__`/`__spec__` metadata and namespace packages.

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
- Runtime mutation of `sys.path` is allowed only when explicitly enabled.
- Resolution order is stable and documented in build metadata.

### 3.2 Allowed Forms
- `import x`, `import x as y`
- `from x import y`
- `from x import y as z`
- `from x import *` (module scope only; honors `__all__` when present, otherwise skips underscore-prefixed names)

### 3.3 Dynamic Imports
- `__import__` and `importlib` are supported only when the bridge policy is
  enabled and the target is allowlisted.
- Non-allowlisted imports raise a deterministic error.

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
- Differential import semantics: `tests/differential/stdlib/importlib_basic.py`, `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_relative_import_from_package.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_support_bootstrap.py`

---

## 6. Build-Time Manifest
Build emits an import manifest:
- list of resolved modules,
- module origin (compiled/stdlib/bridge),
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
- Policy for namespace packages.
- Editable installs and dev-mode behaviors.

`__spec__` is populated for compiled modules using `importlib.machinery.ModuleSpec` with Molt loader metadata.
