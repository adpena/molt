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
    `_socket`, `xml`).
- **Top-level module entry**:
  - A file `src/molt/stdlib/<name>.py`.
- **Top-level package entry**:
  - A package directory `src/molt/stdlib/<name>/__init__.py`.
- **Package-kind requirement**:
  - If CPython exposes `<name>` as a package, Molt must expose it as a package.
- **Submodule stdlib name**:
  - A dotted `.py` module/package under a CPython stdlib top-level module (for
    example `asyncio.events`, `importlib.resources._common`, `json.tool`).
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
- Baseline data:
  - `tools/stdlib_module_union.py`
- Baseline generator:
  - `tools/gen_stdlib_module_union.py`
- Stub synchronizer:
  - `tools/sync_stdlib_top_level_stubs.py`
  - `tools/sync_stdlib_submodule_stubs.py`
- Enforcer:
  - `tools/check_stdlib_intrinsics.py`
- Generated status artifact:
  - `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`

## 5. Standard Operator Workflows
### 5.1 Daily/Feature Work (No Version Change)
1. Verify no missing names:
   - `python3 tools/sync_stdlib_top_level_stubs.py`
   - `python3 tools/sync_stdlib_submodule_stubs.py`
2. Verify intrinsic gates:
   - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
3. Verify intrinsic-partial ratchet posture:
   - `cat tools/stdlib_intrinsics_ratchet.json`
4. Refresh audit after meaningful lowering change:
   - `python3 tools/check_stdlib_intrinsics.py --update-doc`

### 5.2 Add A New CPython Version (Example: 3.15)
1. Ensure interpreter is available to `uv`.
2. Regenerate baseline with explicit versions:
   - `python3 tools/gen_stdlib_module_union.py --python 3.12 --python 3.13 --python 3.14 --python 3.15`
3. Materialize missing top-level entries:
   - `python3 tools/sync_stdlib_top_level_stubs.py --write`
   - `python3 tools/sync_stdlib_submodule_stubs.py --write`
4. Run gates:
   - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
   - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
5. Regenerate audit:
   - `python3 tools/check_stdlib_intrinsics.py --update-doc`
6. Update documentation:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`
   - this file (`0027`) if workflow semantics changed.

### 5.3 Regenerate Baseline To Alternate Path (Dry/Inspection)
- `python3 tools/gen_stdlib_module_union.py --output /tmp/stdlib_module_union.py`

## 6. Stub Policy (Non-Negotiable)
When `sync_stdlib_top_level_stubs.py --write` or
`sync_stdlib_submodule_stubs.py --write` creates missing entries:

1. Stubs must remain intrinsic-first:
   - load required intrinsic, no host-stdlib import fallback.
2. Stubs must fail fast on unsupported behavior:
   - raise deterministic runtime errors, never silent fallback.
3. Stubs are temporary:
   - each file must carry grepable `TODO(...)` marker with milestone/owner.
4. Promotion path:
   - replace stub behavior with real Rust-intrinsic-backed implementation.

## 7. Gate Failure Triage
### 7.1 Missing Top-Level Coverage
Message:
- `stdlib top-level coverage gate violated`

Action:
1. Run `python3 tools/sync_stdlib_top_level_stubs.py --write`.
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
2. Update any path references in docs/tests as needed.

### 7.4 Missing Submodule Coverage
Message:
- `stdlib submodule coverage gate violated`

Action:
1. Run `python3 tools/sync_stdlib_submodule_stubs.py --write`.
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

1. `python3 tools/sync_stdlib_top_level_stubs.py`
2. `python3 tools/sync_stdlib_submodule_stubs.py`
3. `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
4. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
5. `python3 tools/check_stdlib_intrinsics.py --update-doc`
6. Confirm `docs/spec/STATUS.md` and `ROADMAP.md` reflect current counts and
   gate posture.
7. Confirm `tools/stdlib_intrinsics_ratchet.json` is tightened only when real
   lowering progress landed in the same change.

## 9. Design Notes
- The baseline uses the union across supported CPython versions to avoid
  accidental regression when a name exists only in one supported minor.
- Baseline is versioned in-repo so CI is deterministic and does not depend on
  runtime host Python state.
- Platform-specific names are intentionally included if present in the baseline;
  they remain subject to intrinsic-first stub policy until fully lowered.

## 10. Related Specs
- `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`
- `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
- `docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md`
