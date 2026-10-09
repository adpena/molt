# CPython Stdlib Union Baseline
**Spec ID:** 0027
**Status:** Active
**Owner:** stdlib + tooling
**Version target:** CPython 3.12+ (currently 3.12/3.13/3.14 union)

## 1. Why This Exists
Molt previously tracked stdlib lowering status only for modules already present
in `src/molt/stdlib`. That left a blind spot: names missing entirely from the
tree did not appear in `probe-only`/`python-only`/`intrinsic-partial` counts.

This spec closes that gap with hard top-level and submodule name gates.

The gates enforce that Molt always has one canonical module/package entry for:
- every CPython stdlib top-level name in the supported-version union,
- every CPython stdlib `.py` submodule/subpackage name in the supported-version
  union.

## 2. Definitions
- **Top-level stdlib name**:
  - A name in `sys.stdlib_module_names` (for example `json`, `re`, `sqlite3`,
    `_socket`, `xml`). The baseline reads the list from
    `Python/stdlib_module_names.h` at the pinned CPython revision; CPython
    compiles `sys.stdlib_module_names` from that file.
- **Top-level module entry**:
  - A file `src/molt/stdlib/<name>.py`.
- **Top-level package entry**:
  - A package directory `src/molt/stdlib/<name>/__init__.py`.
- **Package-kind requirement**:
  - If CPython exposes `<name>` as a package, Molt must expose it as a package.
    The baseline counts `<name>` as a package when `Lib/<name>` is a directory
    at the pinned revision, which is what `importlib.util.find_spec` reports on
    an unmodified install.
- **Submodule stdlib name**:
  - A dotted `.py` module/package under a CPython stdlib top-level module (for
    example `asyncio.events`, `importlib.resources._common`, `json.tool`), read
    from the `Lib/` tree at the pinned revision. Test packages inside a stdlib
    package (`idlelib.idle_test`) count; the top-level `test` package does not,
    because `test` is not a stdlib module name.
- **Coverage baseline**:
  - The versioned union file `tools/stdlib_module_union.py`.

## 3. Hard Invariants
`tools/check_stdlib_intrinsics.py` now enforces all of the following:

1. Every baseline top-level name exists in Molt.
2. No duplicate top-level mapping:
   - forbidden: both `name.py` and `name/__init__.py`.
3. Package-kind parity:
   - names in baseline `STDLIB_PACKAGE_UNION` must be packages in Molt.
4. Every baseline submodule/subpackage name exists in Molt.
5. No duplicate submodule mapping:
   - forbidden: both `pkg/name.py` and `pkg/name/__init__.py`.
6. Subpackage-kind parity:
   - names in baseline `STDLIB_PY_SUBPACKAGE_UNION` must be packages in Molt.

Failure of any invariant is a hard CI failure.

## 4. Canonical Files
- Pinned CPython sources (one tag and commit per supported version, shared
  with regrtest):
  - `config/cpython_regrtest_sources.toml`
- Source snapshot with receipts (the header lines and the `Lib/` directories
  and `.py` files below stdlib names, each input with its git object id and
  sha256):
  - `config/cpython_stdlib_snapshot.json`
- Baseline data:
  - `tools/stdlib_module_union.py`
- Baseline generator (CI checks it offline in `repository.generators`):
  - `tools/gen_stdlib_module_union.py`
- Stub generator (one template for every stub; `tests/test_stdlib_stubs.py`
  pins it):
  - `tools/gen_stdlib_stubs.py`
- Enforcer (CI command `python.static.stdlib-intrinsics`):
  - `tools/check_stdlib_intrinsics.py`
- Generated status artifact:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`

## 5. Standard Operator Workflows
### 5.1 Daily/Feature Work (No Version Change)
1. Verify that no union name is missing and no stub has drifted:
   - `python3 tools/gen_stdlib_stubs.py --check`
2. Verify intrinsic gates (the CI command):
   - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
3. Verify intrinsic-partial ratchet posture:
   - `cat tools/stdlib_intrinsics_ratchet.json`
4. Refresh audit after meaningful lowering change:
   - `python3 tools/check_stdlib_intrinsics.py --update-doc`

### 5.2 Add A New CPython Version Or Move A Pin (Example: 3.15)
1. Add the version to the target-Python authority (`src/molt/target_python.py`;
   `tools/check_table_drift.py` requires the baseline versions to equal it).
2. Pin its latest stable tag and that tag's commit in
   `config/cpython_regrtest_sources.toml`. A patch-release advance changes
   only the pin.
3. Fetch the pinned inputs (network; git verifies each tag against its pinned
   commit, and content at an unchanged commit must match its receipt):
   - `python3 tools/gen_stdlib_module_union.py --refresh-sources`
4. Regenerate the baseline offline:
   - `python3 tools/gen_stdlib_module_union.py --write`
5. Materialize stubs for the new union entries:
   - `python3 tools/gen_stdlib_stubs.py --write`
6. Run gates:
   - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
   - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
7. Regenerate audit:
   - `python3 tools/check_stdlib_intrinsics.py --update-doc`
8. Update documentation:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
   - this file (`0027`) if workflow semantics changed.

### 5.3 Inspect The Baseline Without Writing
- `python3 tools/gen_stdlib_module_union.py --check` names every stale output.
  It reads only committed files, so it gives the same answer on every host,
  and it fails closed when the snapshot disagrees with its receipts or pins.

## 6. Stub Policy (Non-Negotiable)
`tools/gen_stdlib_stubs.py` owns every stub. It writes one for each union
module Molt lacks and holds every existing stub to one template, so a hand
edit fails `--check`. The template fixes this contract:

1. Stubs are intrinsic-first:
   - importing a stub requires the `molt_capabilities_has` intrinsic; there is
     no host-stdlib import fallback.
2. Stubs bind nothing:
   - a stub has no public name and leaves no import helper in its namespace.
3. Stubs fail fast:
   - any attribute access raises the deterministic gap error
     `stdlib {module|package} "<name>" is not fully lowered yet; only an
     intrinsic-first stub is available.`, never a silent fallback.
   - a package stub raises `AttributeError` for its union submodules instead,
     because the import system asks for a submodule before it loads it
     (`from pkg import sub`), and only that error lets it go on.
4. Platform-only modules keep CPython's import outcome:
   - a module CPython ships on one platform raises `ModuleNotFoundError` on
     every other platform (`PLATFORM_ONLY` in the generator).
5. Stubs are counted debt:
   - each stub carries a grepable `TODO(stdlib-parity, ...)` marker, and the
     structural audit counts every stub.
6. The union decides kind:
   - the generator refuses a stub for a name outside the union or of the wrong
     kind (module versus package) and names the fix.
7. Promotion path:
   - lowering a module replaces its stub file with the real
     Rust-intrinsic-backed implementation; the generator then stops owning it.

## 7. Gate Failure Triage
### 7.1 Missing Top-Level Coverage
Message:
- `stdlib top-level coverage gate violated`

Action:
1. Run `python3 tools/gen_stdlib_stubs.py --write`.
2. Re-run checker.
3. If still missing, inspect baseline file for recent version additions.

### 7.2 Duplicate Top-Level Mapping
Message:
- `top-level module/package duplicate mapping`

Action:
1. Keep exactly one representation:
   - either `name.py` or `name/__init__.py`.
2. If name is in `STDLIB_PACKAGE_UNION`, keep package form.

### 7.3 Package-Kind Mismatch
Message:
- `stdlib package kind gate violated`

Action:
1. Convert `src/molt/stdlib/name.py` to `src/molt/stdlib/name/__init__.py`.
   For a stub, `python3 tools/gen_stdlib_stubs.py --check` names the `git mv`.
2. Update any path references in docs/tests as needed.

### 7.4 Missing Submodule Coverage
Message:
- `stdlib submodule coverage gate violated`

Action:
1. Run `python3 tools/gen_stdlib_stubs.py --write`.
2. Re-run checker.

### 7.5 Subpackage-Kind Mismatch
Message:
- `stdlib subpackage kind gate violated`

Action:
1. Convert `src/molt/stdlib/pkg/name.py` to
   `src/molt/stdlib/pkg/name/__init__.py`.
2. Re-run checker and update references if import paths changed.

## 8. Release Checklist
Before release or large lowering tranche merge:

1. `python3 tools/gen_stdlib_stubs.py --check`
2. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist` (the CI
   command: every gate plus the strict closure of the critical roots)
3. `python3 tools/check_stdlib_intrinsics.py --update-doc`
4. Confirm `docs/spec/STATUS.md` and `ROADMAP.md` reflect current counts and
   gate posture.
5. Confirm `tools/stdlib_intrinsics_ratchet.json` is tightened when real
   lowering progress lands. It rises only when union modules that Molt lacked
   become counted stubs, and the change must say so.

## 9. Design Notes
- The baseline uses the union across supported CPython versions to avoid
  accidental regression when a name exists only in one supported minor.
- Baseline is versioned in-repo so CI is deterministic and does not depend on
  runtime host Python state.
- The baseline comes from pinned CPython sources, not from an interpreter.
  Interpreter builds differ: uv's 3.12 build omits `msilib`, and some builds
  strip `idlelib/idle_test`. A live-interpreter baseline therefore changed
  with the host, and its `--check` could not pass in CI.
- Platform-specific names are intentionally included if present in the baseline;
  they remain subject to intrinsic-first stub policy until fully lowered.

## 10. Related Specs
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
- `docs/spec/areas/compat/plans/stdlib_lowering_plan.md`
