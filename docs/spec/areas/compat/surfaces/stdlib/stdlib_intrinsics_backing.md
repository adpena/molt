# Stdlib Intrinsics Backing Tracker
**Spec ID:** 0015-IB
**Status:** Active (generated-gate driven)
**Owner:** stdlib + runtime + tooling

## Canonical Source Of Truth
This tracker is no longer maintained as a hand-edited per-module table.

Canonical intrinsic-backing status now comes from:
- gate script: `tools/check_stdlib_intrinsics.py`
- generated audit: `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`

The gate computes `intrinsic-backed`, `intrinsic-partial`, `intrinsic-support`,
`python-compiled`, `stub` and `policy-gate` directly from `src/molt/stdlib/**`
source. A module's intrinsic status rests on the intrinsics it reads: a
module-level binding it loads or exports, a requirement inside a function or
class body, a requirement an expression consumes, or a private binding another
stdlib module imports by name. A requirement nothing reads is a gate failure,
not backing (spec 0016). `python-compiled` is an admitted implementation: Molt
compiles the module's Python source like application code. `stub` is the
generated stand-in for an unlowered module. `policy-gate` is reserved for pure
fail-closed namespace reservations whose only executable statement is an
unconditional `ImportError`.

`src/molt/stdlib_intrinsic_policy.py` owns source relationship classification
for the audit and the core-lane gate. Builds do not classify: a compiled program
may import any implemented stdlib module. A wrapper can inherit intrinsic backing through
a proved import of an already backed module in the same package, or through
its exact top-level private provider (`io` importing `_io`). Private names,
prefix matches, provider children, missing providers, unresolved imports and
unanchored cycles do not establish that public/private relationship. Pure
reexports keep the stricter `intrinsic-support` rule: every resolved owner must
already have backing. These relationships classify source support; they do not
attest API parity or target execution.

Runtime publication and Python protocol adapters are distinct authorities.
For example, runtime initialization publishes native `sys` streams, but event
loop exception reporting must use the current replaceable Python streams and
their `write`/`flush` methods. That adapter lives with its sole intrinsic-backed
owner, `asyncio.events`; the retired private `asyncio._debug` module has no
independent provider or compatibility stub. A module import, builtin spelling,
runtime catalog entry or method protocol alone does not grant intrinsic backing.

Tk callable acquisition has one owner in `tkinter._support`. It returns the
actual `_tkinter` callable and rejects absent or non-callable providers, preserving
the wrapper's `TkappType`/raw-handle conversion. Widget bindings live in
`tkinter.widgets`; parent-only aliases are removed. Both consumers explicitly
import that shared provider. Source discovery does not establish this relationship,
and unrelated unresolved relative operands retain their runtime custody obligations.

## Coverage Baseline
Top-level + submodule name coverage is enforced against the CPython
3.12/3.13/3.14 union baseline:
- baseline: `tools/stdlib_module_union.py`
- generator: `tools/gen_stdlib_module_union.py`
- stub generator: `tools/gen_stdlib_stubs.py`
- workflow doc: `docs/spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md`

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
- Fields: `max_intrinsic_partial` and `max_stub`
- Rule: neither the `intrinsic-partial` nor the `stub` count may exceed its budget.
- Expected workflow: lower modules first, then reduce the ratchet in the same change.

## Full-Coverage Contract
- Full-coverage attestation source: `tools/stdlib_full_coverage_manifest.py`
- `STDLIB_FULLY_COVERED_MODULES`: modules/submodules explicitly attested as
  full CPython 3.12+ API/PEP coverage (for Molt-supported semantics).
- `STDLIB_REQUIRED_INTRINSICS_BY_MODULE`: required intrinsic contract for each
  attested module.
- Gate rules enforced by `tools/check_stdlib_intrinsics.py`:
  - every attested module must be `intrinsic-backed` or `python-compiled`
  - every attested module must have a contract entry (empty for `python-compiled`)
  - every contract intrinsic must exist in the runtime manifest, and the module must read it
  - a non-attested module that reads intrinsics is `intrinsic-partial`

## Too-Dynamic Differential Policy
- Intentional unsupported dynamism cases declare their policy at the test source:
  `# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy`.
- `tools/compat/test_policy.py` projects that scope for verified-subset and
  suite-honesty consumers; there is no separate stdlib path manifest.
- `tests/molt_diff.py` applies expected-failure behavior from the metadata:
  - Molt fail + CPython pass => `[XFAIL]` (counted as pass)
  - Molt pass + CPython pass => `[XPASS]` (counted as failure)
- Current high-confidence policy scope is `exec`/`eval` differential
  tests, matching the project break policy against maximal runtime dynamism.
