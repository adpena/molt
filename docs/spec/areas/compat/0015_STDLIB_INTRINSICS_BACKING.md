# Stdlib Intrinsics Backing Tracker
**Spec ID:** 0015-IB
**Status:** Active (generated-gate driven)
**Owner:** stdlib + runtime + tooling

## Canonical Source Of Truth
This tracker is no longer maintained as a hand-edited per-module table.

Canonical intrinsic-backing status now comes from:
- gate script: `tools/check_stdlib_intrinsics.py`
- generated audit: `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`

The gate computes `intrinsic-backed`, `intrinsic-partial`, `probe-only`, and
`python-only` directly from `src/molt/stdlib/**` source and intrinsic usage.

## Coverage Baseline
Top-level + submodule name coverage is enforced against the CPython
3.12/3.13/3.14 union baseline:
- baseline: `tools/stdlib_module_union.py`
- generator: `tools/gen_stdlib_module_union.py`
- stub sync: `tools/sync_stdlib_top_level_stubs.py`
- submodule stub sync: `tools/sync_stdlib_submodule_stubs.py`
- workflow doc: `docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md`

## Daily Commands
- Audit + lint:
  - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
- Critical strict roots:
  - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
- Ratchet budget check (explicit file override lane):
  - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only --intrinsic-partial-ratchet-file tools/stdlib_intrinsics_ratchet.json`
- Regenerate audit doc:
  - `python3 tools/check_stdlib_intrinsics.py --update-doc`

## Ratchet Policy
- Ratchet source: `tools/stdlib_intrinsics_ratchet.json`
- Field: `max_intrinsic_partial`
- Rule: `intrinsic-partial` count must never exceed the ratchet.
- Expected workflow: lower modules first, then reduce the ratchet in the same change.

## Full-Coverage Contract
- Full-coverage attestation source: `tools/stdlib_full_coverage_manifest.py`
- `STDLIB_FULLY_COVERED_MODULES`: modules/submodules explicitly attested as
  full CPython 3.12+ API/PEP coverage (for Molt-supported semantics).
- `STDLIB_REQUIRED_INTRINSICS_BY_MODULE`: required intrinsic contract for each
  attested module.
- Gate rules enforced by `tools/check_stdlib_intrinsics.py`:
  - every attested module must be `intrinsic-backed`
  - every attested module must have a contract entry
  - every contract intrinsic must exist in runtime manifest and be wired in-module
  - non-attested modules are classified as `intrinsic-partial` by default

## Too-Dynamic Differential Policy
- Intentional unsupported dynamism cases are tracked in
  `tools/stdlib_full_coverage_manifest.py` via
  `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`.
- `tests/molt_diff.py` auto-applies expected-failure behavior for listed tests:
  - Molt fail + CPython pass => `[XFAIL]` (counted as pass)
  - Molt pass + CPython pass => `[XPASS]` (counted as failure)
- Current high-confidence policy scope is `exec`/`eval` planned differential
  tests, matching the project break policy against maximal runtime dynamism.
