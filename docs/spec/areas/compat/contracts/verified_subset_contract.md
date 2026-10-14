# Verified Subset Contract

**Spec ID:** 0215
**Status:** Active
**Owner:** frontend + runtime + tooling

## Definition

Molt's Python contract defines the language, runtime and standard-library
semantics it commits to implement correctly within explicit restrictions on
excessive dynamism that obstruct efficient ahead-of-time compilation and
standalone execution. The [dynamic-semantics policy](dynamic_execution_policy_contract.md)
owns those restrictions. Each restriction needs a concrete performance or
compilation rationale; implementation difficulty, a slow implementation, missing
support or a failing test does not establish a design exclusion. Scoped mutation,
callbacks and introspection remain subject to their admitted Python semantics.

Verification describes evidence for this contract, not its intended scope. A
required coordinate is verified only when the exact source revision has passing
receipts for every required observation in that coordinate. Missing features,
incorrect behavior, performance failures and unverified coordinates remain
engineering and release obligations inside the contract; they cannot be
reclassified as outside the subset to obtain a passing release.

Version-pinned [CPython source](https://github.com/python/cpython) and applicable
accepted [official PEPs](https://peps.python.org/) are the semantic authorities.
Use the selected target version's implementation and applicable PEP revisions;
historical proposals do not override later Python behavior. Preserve specified
observable behavior, including errors, side effects and callback order, while
optimizing the implementation. Differential execution verifies the implementation
against that authority.

Target availability and capabilities are explicit deployment coordinates, not
additional language exclusions. Unsupported requests fail early under their
own policies. Compiled outputs never depend on a host or fallback Python
interpreter. Source comparison, conformance receipts and release checks run on
the development side; emitted binaries retain only the checks required by their
actual runtime semantics and safety contracts.

The current required matrix is the cross-product of:

- target-language versions 3.12, 3.13, and 3.14, each with an exact pinned
  reference-CPython micro version;
- every Windows, macOS, and Linux architecture in the release-target authority;
- LLVM, native and WASM backends;
- `dev` and `release` guest build profiles;
- the CPython-language ABI and GIL concurrency mode.

`config/verified_subset.toml` selects executable policy. `TargetPythonVersion` owns
language-version support, and `config/release_targets.toml` owns platform,
architecture, runner, and Rust-target coordinates. Generated projections do not
redeclare those facts. Human status and coverage-index documents are
presentations, not receipt inputs or hidden support authorities.

Python 3.15 is also required before v1.0 release. Its target admission, semantic
and ABI migration, and complete release-matrix qualification remain open under
V1-35 in the [release ledger](../../../../agent/V1_HANDOFF_FINDINGS.md#open-release-requirements).
The current executable matrix above must not be interpreted as completing that
obligation. A future version enters the existing authority only with its pinned
CPython source and complete producer/consumer migration; no unknown version is
accepted by extrapolation.

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

Each validation transaction captures one explicit source inventory: one physical
traversal and one stable byte read per test. Metadata and the source digest come
from those same bytes. Suite floors and all coordinate projections consume that
inventory; receipt batches and release assembly share it instead of rescanning or
using process-global source caches. A new transaction always captures fresh
source state and binds it to its repository root.
The policy is likewise parsed from one stable byte capture. Its captured digest
must equal the receipt's policy input record, and release evidence roles derive
from that same policy generation rather than a second default-root parse.

Inventory checks fence selected directory membership, ancestor identity, and
file generations. Execution checks the inventory before dispatch and after the
harness returns; receipt and bundle publication recheck before admission. A
detected source change fails closed. These are metadata-generation fences, not
an immutable execution snapshot or a guarantee against hostile filesystem
mutation. Unrelated writes outside selected suites do not change their source
inventory.

Tests whose observable behavior intentionally follows Molt capability or dynamic
execution policy instead of CPython equivalence carry the typed
`verified_subset_scope=capability_policy` or
`verified_subset_scope=dynamic_execution_policy` source fact. The policy file
explicitly admits those two scopes as projection exclusions. This source-local
classification is distinct from failure reason and cannot be inferred from a
passing or failing result. Every expected failure in the default
`cpython_equivalence` scope is conformance debt. Neither an XFAIL nor an XPASS
can produce a passing verified-subset receipt.

## Exception observations and transformation parity

A pending-call/eval-breaker marker adds an asynchronous role to an existing
exception observation; it does not replace that observation's synchronous
transfer or SSA payload. Generator fusion may retire a removed loop's exclusive
polling role, including at a predecessor of a split latch. Call-return and
surviving-loop roles remain. Only the shared exception-check elimination oracle
may discard a synchronous observation after proving it redundant.

The pending-call fixture builds the same manifest-declared C source against
the active reference CPython and Molt. Native and WASM outputs are compared to
that CPython execution, not merely to each other or to handwritten expected
text. Every build receives the admitted reference language version. Mocked
harness tests prove routing and mismatch detection only; they do not close
runtime matrix cells. The differential corpus separately checks consumer
arithmetic exceptions before a generator's post-yield side effects.

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
source revision. The coordinate selects the guest build profile; execution
records the selected runtime and compiler profile labels.

The current execution record does not bind the actual Molt compiler binary
digest, profile and feature set, or selected runtime cell/generation identity.
Close these through existing build diagnostics and runtime inventories, and
require agreement with the assembled candidate. A matching
source SHA and one candidate smoke guest do not establish that semantic
acceptance exercised the shipped compiler/runtime. Target-dependent arithmetic
also needs an explicit ABI/oracle rule for WASM; the host CPython alone does
not establish the target ABI. These are qualification gaps, not additions to
verified coverage.

The comparison law still allows test metadata to ignore stderr, which is the
default. Recording stderr hashes does not establish equality or deterministic
guest output. Release qualification must enforce the required guest-stderr
comparison and attribute host diagnostics separately through the existing
comparison authority.

`tools/release_criterion_receipt.py` validates against the captured coordinate
projection and rejects missing, duplicate, excluded, malformed, stale, or
self-inconsistent outcomes. `tools/release_exit_gate.py` requires the exact
receipt closure; one receipt or one locally green host is not E3.
Standalone receipt checks capture their own inventory; batch consumers own the
shared inventory's final unchanged check.

Receipt object keys derive from their typed records. Release assembly consumes
the same scalar and object validators; it must not mirror those predicates.
Malformed JSON-shaped outcome values produce diagnostics, not coercions or
incidental exceptions. In particular, booleans are not schema versions or return
codes, and finite integer metrics retain arbitrary precision. Nightly bundle
manifests and their embedded link metadata use `molt.exact_json` for duplicate-key
and non-finite-number rejection; validated file records stay typed through
extraction instead of reinterpreting strings and integers at each consumer.

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
runner requirement. Release staging must repeat that provenance verification for
every E3 receipt inside the source-addressed release-exit archive before any
candidate build may proceed. The release workflow's plan step performs that
provenance admission before candidate builds, as specified by the
[packaging acceptance contract](../../../../../packaging/PACKAGING.md).
Declared LLVM cells still need cross-host SDK provisioning and an admitted
compiler with LLVM enabled. Wiring the gate does not supply missing receipts
or establish conformance.
The verified-subset workflow is the execution authority; the local
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
