# 0017 Stdlib Lowering Sweep (Intrinsic-First)

Generated: 2026-02-12
Scope: `src/molt/stdlib/**`, runtime intrinsic manifest/wiring, checker gates, differential parity.

## Canonical References
- Execution plan: `docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md`
- Gate script: `tools/check_stdlib_intrinsics.py`
- Audit doc: `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
- Union baseline: `tools/stdlib_module_union.py`
- Ratchet budget: `tools/stdlib_intrinsics_ratchet.json`

## Current Snapshot
Classification snapshot:
- `intrinsic-backed`: `177`
- `intrinsic-partial`: `696`
- `probe-only`: `0`
- `python-only`: `0`

Top blocker:
- `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
  fails at bootstrap gate because `re` is still `intrinsic-partial`.

## Gate Posture
Hard gates currently enforced:
1. zero `probe-only`
2. zero `python-only`
3. top-level union coverage
4. submodule union coverage
5. module/package collision and package-kind checks
6. strict fallback pattern checks
7. intrinsic runtime pass-fallback checks
8. intrinsic-partial ratchet budget (`max_intrinsic_partial`)

## Blocker-First Progress (This Tranche)
Landed:
1. importlib resolver blocker hardening in `importlib.util`/`importlib.machinery`.
2. one-shot default `PathFinder` bootstrap wiring.
3. checker ratchet budget enforcement and `_py_*` host-fallback pattern blocking.
4. regression tests:
   - `tests/test_stdlib_importlib_machinery.py`
   - `tests/test_check_stdlib_intrinsics.py` (ratchet + fallback-pattern lanes)
5. targeted differential confirmations:
   - `tests/differential/basic/importlib_find_spec_path_importer_cache_intrinsic.py`
   - `tests/differential/basic/importlib_find_spec_path_hooks_intrinsic.py`

## Active Queue
1. `pickle` (`intrinsic-partial`) to intrinsic-backed completion.
2. runtime-heavy cluster: `_asyncio`, `smtplib`, `zipfile`, `zipimport`.
3. data/number cluster: `decimal`, `statistics`, `locale`.
4. metadata/email cluster: `importlib.metadata`, `email.header`,
   `email.message`, `email.policy`.
5. tooling cluster: `ctypes`, `gettext`, `shelve`.
6. final gate hardening: flip to `intrinsic-partial == 0` hard fail once queue reaches zero.

## Differential Discipline
Per cluster:
1. targeted native differential lane first,
2. targeted wasm parity lane second,
3. full sweep only after targeted lanes are green.

Required settings:
- `MOLT_DIFF_MEASURE_RSS=1`
- `MOLT_DIFF_RLIMIT_GB=10`
- memory regressions treated as blockers.
