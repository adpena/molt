# Verified Subset Contract

**Spec ID:** 0215
**Status:** Active
**Owner:** frontend + runtime + tooling

## Definition

The verified subset is the source-bound set of Python language, runtime, and
stdlib behaviors for which Molt has demonstrated CPython-equivalent observable
behavior. A required coordinate is not verified merely because it appears in a
configuration file: verification exists only when the exact source revision has
a passing receipt for every required coordinate.

Behavior that requires unrestricted dynamic execution, runtime monkeypatching,
or a hidden host-CPython fallback remains outside this contract and must fail
closed under Molt's explicit dynamic and capability policies.

The current required matrix is the cross-product of:

- target-language versions 3.12, 3.13, and 3.14, each with an exact pinned
  reference-CPython micro version;
- every Windows, macOS, and Linux architecture in the release-target authority;
- native and WASM backends;
- the CPython-language ABI and GIL concurrency mode.

`config/verified_subset.toml` selects executable policy. `TargetPythonVersion` owns
language-version support, and `config/release_targets.toml` owns platform,
architecture, runner, and Rust-target coordinates. Generated projections do not
redeclare those facts. Human status and coverage-index documents are
presentations, not receipt inputs or hidden support authorities.

## Test projection

`tools/compat/test_policy.py` is the sole authority for differential-test
discovery, portable path identity, `MOLT_META` parsing, version/platform/
architecture/backend applicability, and expected-failure classification.
`tests/molt_diff.py`, coverage and honesty tooling, and
`tools/verified_subset.py` all consume it.

The source closure is collected directly from the physical basic-language and
stdlib suite directories under each row's typed recursive policy. Generated
`TESTS.txt` lane manifests are scheduling projections and never select release
receipt evidence. Each physical test has one suite owner; links, junctions,
reparse points, repository escapes, cross-suite duplicates, and portable path
collisions fail closed. A coordinate projection records every source test as
either applicable or excluded with a deterministic reason and binds the test
bytes into its digest. The harness receives the projection's exact applicable
path list through `--files-from`; runtime skips are failures, not coverage. Each
suite row carries a nondecreasing CPython-equivalence source floor, so policy
scope exclusions cannot pad conformance evidence and one suite cannot pad
another.

Tests whose observable behavior intentionally follows Molt capability or dynamic
execution policy instead of CPython equivalence carry the typed
`verified_subset_scope=capability_policy` or
`verified_subset_scope=dynamic_execution_policy` source fact. The policy file
explicitly admits those two scopes as projection exclusions. This source-local
classification is distinct from failure reason and cannot be inferred from a
passing or failing result. Every expected failure in the default
`cpython_equivalence` scope is conformance debt. Neither an XFAIL nor an XPASS
can produce a passing verified-subset receipt.

## Pass law and evidence

A coordinate passes only when all applicable tests:

- ran exactly once;
- matched CPython before any expected-failure overlay;
- resolved to pass;
- passed on the selected backend under the canonical comparison law;
- have no applicable expected-Molt-failure marker.

The receipt contains the complete sorted outcome rows, including raw, resolved,
and backend status, return codes, normalized-output hashes, expected-failure
classification, and comparison-law identity. Counts are derived from those
rows. It also binds the projection digest, policy inputs, exact reference
CPython executable/version, GIL and pointer-width state, Rust host/toolchain,
backend runner, GitHub Actions run identity, runner OS/architecture/label, and
source revision.

`tools/release_criterion_receipt.py` independently reconstructs the coordinate
projection and rejects missing, duplicate, excluded, malformed, stale, or
self-inconsistent outcomes. `tools/release_exit_gate.py` requires the exact
receipt closure; one receipt or one locally green host is not E3.

## Commands and CI

```text
uv run --python 3.12 python tools/verified_subset.py check
uv run --python <exact-reference-version> python tools/verified_subset.py run \
  --coordinate <coordinate-id> --receipt <path> --source-sha <git-object-id>
uv run --python 3.12 python tools/verified_subset.py verify-receipts \
  --source-sha <git-object-id> --receipt-root <directory>
```

`.github/workflows/verified-subset.yml` obtains its dynamic matrix only from
the policy tool, executes all coordinates, signs every passing receipt with the
workflow's GitHub OIDC identity, preserves each receipt as a separate artifact,
and verifies both Sigstore provenance and the exact receipt closure. Receipt
verification pins the signer workflow, source digest, repository, and hosted
runner requirement. Release staging repeats that provenance verification for
every E3 receipt inside the source-addressed release-exit archive before any
candidate build may proceed. The workflow is the execution authority; the local
`check` command validates policy and reports remaining expected-failure debt but
does not claim conformance.

## Promotion and regression

Promotion requires a supported-surface/status entry, a differential witness,
and a passing full receipt closure. A regression in a proven coordinate is P0.
Exclusions may not be added to hide an implementation failure; a new scope
requires an exact policy-schema and contract change. Native and WASM are
co-equal, and backend-specific semantic workarounds are forbidden.

Binary C-API/ABI coverage is a separate ecosystem-support dimension. Its
version-specific facts live in the CPython ABI coverage authority and must not
be confused with language-level verified-subset proof.
