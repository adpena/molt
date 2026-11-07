# Python version authority

Status: design, 2026-11-07. Owner: integrator.

## Problem (measured)

The supported CPython minors and their pinned patches are spelled in many places:

- Six minor tuples: `src/molt/target_python_policy.py` (29 consumer files),
  `src/molt/release_matrix.py`, `tools/compatibility_error_protocol.py`,
  `tools/gen_compat_platform_availability.py`, `tools/regrtest_adapter.py`,
  `tools/stdlib_module_union.py`.
- Four patch spellings: `.python-version` (3.12.16), `config/verified_subset.toml`
  `reference_cpython`, `config/cpython_regrtest_sources.toml` tags, and the hand-written
  `tools/proof_plan.toml` `python` policy (`version_pattern`, `setup_value`, `setup_evidence`).
- `tools/pin_freshness.py` keeps the copies in step by regex surgery on `proof_plan.toml`
  (`rewrite_policy`), the signature of a missing authority.

On 2026-11-07 the integrator twice ran gates under a stale 3.12.15 image. The proof-plan
contract refused it, but only after the run, and only because one copy happened to be current.

## Prior art

- `rust-toolchain.toml`: one declaration that rustup, Cargo and CI all read.
- rules_python `python.toolchain(python_version=...)`: versions declared once; every
  consumer resolves through the registered toolchain.
- CPython `Include/patchlevel.h`: the version exists once; every other spelling is generated.
- uv: `.python-version` is the selector uv and CI read, so it stays, as a projection.

## Mechanism

1. Minors: `target_python_policy.py` stays the one authority for supported target minors and
   the default minor. Each duplicate tuple imports it. A site whose meaning differs (for
   example a release claim that may lag a target) keeps its own name, documented as a
   different fact; none is merged by spelling alone.
2. Patches: `config/cpython_regrtest_sources.toml` already pins each minor to an exact tag and
   commit (a lock). It becomes the patch authority for every supported minor; a pending row
   (3.15) is not admitted to the target catalog.
   - `verified_subset.toml` drops `reference_cpython`; its loader derives the reference patch
     of each supported minor from the pin rows.
   - `.python-version` becomes a generated projection of the default minor's pin (uv, CI and
     `checkout_provisioning` keep reading the file).
   - The `proof_plan.toml` `python` policy drops its literals; the plan loader derives
     `version_pattern`, `setup_value` and `setup_evidence` from the default pin, as it already
     derives node-package and target-Python policies.
3. `pin_freshness` moves a Python pin by editing one pin row (tag and commit) and regenerating
   projections. `rewrite_policy`'s Python path is deleted.
4. Gate reruns go through the landing executor (`tools/trunk_landing.execute`), which prepares
   each gate's interpreter with `prepare_gate_environment`; no caller passes an interpreter.

## Invariants

- One edit moves a pin; the generator check fails a stale projection.
- No consumer reads a version literal that is not the authority or a generated projection.

## Proof

- The plan loader derives the same `python` policy the literal block held (unit test).
- A pin-row change flows to `.python-version`, the reference list and the plan policy
  (generator round trip), and a hand edit of `.python-version` fails the generator check.
- `pin_freshness` tests move the Python pin through the row only.
