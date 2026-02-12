# 0028 Stdlib Intrinsics Execution Plan (2026-02-12)
**Status:** Active
**Owner:** stdlib + runtime + tooling
**Scope:** CPython 3.12/3.13/3.14 stdlib union, top-level + submodule coverage, intrinsic-first lowering

## Mission
Drive Molt to a strict stdlib posture where:
1. every required stdlib module/submodule is canonically present,
2. every required behavior is intrinsic-backed (or explicit fail-fast for unsupported dynamism),
3. CI enforces no silent fallback paths and no coverage regressions.

## Non-Negotiable Gates
1. `probe-only == 0` and `python-only == 0` hard fail.
2. top-level union coverage hard fail.
3. submodule union coverage hard fail.
4. module/package collision hard fail.
5. forbidden fallback patterns hard fail (strict roots + intrinsic-backed + all-stdlib mode).
6. intrinsic runtime pass-fallback hard fail.
7. intrinsic-partial ratchet hard fail (`tools/stdlib_intrinsics_ratchet.json`).

## Acceptance Template (Required For Each Module Conversion)
1. Rust intrinsic implementation.
2. manifest entry in `runtime/molt-runtime/src/intrinsics/manifest.pyi`.
3. regenerated bindings: `src/molt/_intrinsics.pyi` and `runtime/molt-runtime/src/intrinsics/generated.rs`.
4. thin Python shim wiring only (argument normalization/error mapping/capability gating).
5. targeted differential tests for edge semantics.
6. `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only` green for touched lane.
7. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist` green for touched lane.
8. native + wasm parity for touched module(s).

## Blocker-First Execution Order
1. Importlib blocker: `TypeError: module name must be str` + resolver precedence.
2. P2 fast close: `concurrent.futures`, `pickle`.
3. Runtime-heavy cluster: `_asyncio`, `smtplib`, `zipfile`, `zipimport`.
4. Data/number cluster: `decimal`, `statistics`, `locale`.
5. Metadata/email cluster: `importlib.metadata`, `email.header`, `email.message`, `email.policy`.
6. Tooling cluster: `ctypes`, `gettext`, `shelve`.
7. Final hardening: set `intrinsic-partial == 0` gate once queue reaches zero.

## Current Checkpoint (2026-02-12)
Status snapshot from stdlib classification scan:
- `intrinsic-backed`: `177`
- `intrinsic-partial`: `696`
- `probe-only`: `0`
- `python-only`: `0`

Cluster status:
- `concurrent.futures`: intrinsic-backed
- `pickle`: intrinsic-partial
- `_asyncio`, `smtplib`, `zipfile`, `zipimport`: intrinsic-partial
- `decimal`, `statistics`, `locale`: intrinsic-partial
- `importlib.metadata`, `email.header`, `email.message`, `email.policy`: intrinsic-partial
- `ctypes`, `gettext`, `shelve`: intrinsic-partial

Bootstrap/import-core status:
- `os`, `sys`, `time`, `threading`, `asyncio`, `multiprocessing`: intrinsic-backed
- `pathlib`, `socket`: intrinsic-partial
- current global checker blocker remains `re` not yet intrinsic-backed in bootstrap gate.

## Landed In This Tranche
1. importlib resolver blocker fix:
   - live resolver precedence in `importlib.util` (`sys.modules/meta_path/path_hooks/path_importer_cache` view),
   - one-shot default `PathFinder` bootstrap,
   - `importlib.machinery.PathFinder` implementation using intrinsic-backed path search path.
2. regression coverage:
   - `tests/test_stdlib_importlib_machinery.py` for module-name coercion (`module name must be str`) path.
   - differential pass confirmation:
     - `tests/differential/basic/importlib_find_spec_path_importer_cache_intrinsic.py`
     - `tests/differential/basic/importlib_find_spec_path_hooks_intrinsic.py`
3. checker hardening:
   - intrinsic-partial ratchet gate in `tools/check_stdlib_intrinsics.py`,
   - ratchet budget file: `tools/stdlib_intrinsics_ratchet.json`,
   - host fallback import pattern blocking for `_py_*` direct/dynamic imports.
4. regression tests for checker:
   - ratchet regression reject/allow tests,
   - host fallback import pattern rejection tests.

## Differential Execution Policy (Per Cluster)
Run in this order for each module cluster:
1. targeted native differential lane (new edge files first),
2. targeted wasm parity lane for touched modules,
3. full sweep for cluster stabilization.

Required run constraints:
- `MOLT_DIFF_MEASURE_RSS=1`
- 10GB/process limit (`MOLT_DIFF_RLIMIT_GB=10`)
- memory spikes treated as blockers, not follow-up work.

## Weekly Scoreboard (Required)
Publish/update weekly:
1. intrinsic-backed count
2. intrinsic-partial count
3. probe-only count
4. python-only count
5. missing required top-level entries
6. missing required submodule entries
7. native differential pass %
8. wasm parity pass %
9. memory regressions (count + top offenders)

Any PR that worsens the scoreboard must include explicit exception sign-off and rollback plan.

## Update Workflow (When Python Releases Advance)
1. refresh baseline: `python3 tools/gen_stdlib_module_union.py`
2. sync stubs:
   - `python3 tools/sync_stdlib_top_level_stubs.py --write`
   - `python3 tools/sync_stdlib_submodule_stubs.py --write`
3. run gates:
   - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
   - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
4. adjust ratchet downward only after real lowering progress:
   - edit `tools/stdlib_intrinsics_ratchet.json`
5. sync docs in same change:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/OPERATIONS.md`
   - `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
