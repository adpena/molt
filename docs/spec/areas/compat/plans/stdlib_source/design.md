# Python 3.15 target policy and versioned CPython stdlib source

Author: Alejandro Peña. Date: 2026-11-16.
State: approved with the Integrator Correction (2026-11-16) below; implementation in progress.
Architecture owner: [Closed-world stdlib](../stdlib_lowering_plan.md#closed-world-stdlib-architecture).
Open obligations: HF-820 and HF-855 in `docs/agent/V1_HANDOFF_FINDINGS.md`.

## Owner decisions and delivery order (2026-11-16)

> "We primarily care about supporting the latest minor version" for the first
> release; supporting more minors "will involve a lot of minutiae" and comes later.

Python 3.15 is the default and the only Tier 1 (release-blocking) Python.
Python 3.12, 3.13 and 3.14 remain buildable and tested as Tier 2, never
release-blocking. Part 1 admits 3.15, changes the default and migrates its
consumers; it lands before any source replacement. Part 2 builds version-keyed
source selection but vendors only CPython 3.15 for v1, with asyncio first and
then complete module families ranked by use. Admission, source availability and
qualified behavior are separate facts. These decisions supersede this draft's
earlier four-minor vendoring proposal and pending-admission question.

> "Some of the asyncio and related tests have caused out of memory and crashed
> my computer in the past, and we don't want that to happen."

The explicit caps and isolated hazard procedure below are binding for all
proofs, including tests selected indirectly by a consumer or landing gate.
The requested review artifact is
`logs/agents/lane-stdlib-source/artifacts/design.md`, a byte-identical review
copy of this committed design, not a second authority. Repository hygiene keeps
raw evidence and review copies under ignored `logs/`; this maintained design
and its architecture pointer are the exact commit scope. The integrator recorded the binding review in the dated Correction below;
this worker launches no reviewer or other agent.

## Problem and evidence

Molt compiles a second Python implementation of much of CPython's standard
library. This loses APIs, changes semantics and adds runtime work. The existing
AST audit reports 131 pure-Python rewrites, 42 accelerated-Python rewrites and
172 pure-Python stubs for its 3.15 baseline. Those are retained audit results,
not a fresh census of this proposal. Its classifier accepts 90% identical
callables as CPython source and classifies intrinsic-reading rewrites separately;
neither label proves source provenance. All Python-implemented modules, including
the current Rust-backed category, belong to the migration.

HF-855 records release LLVM measurements on macOS arm64 against isolated CPython
3.12: await of a non-suspending coroutine uses 23,498 versus 1,976 instructions;
sleep(0), 90,619 versus 38,531; gather of ten tasks, 10,398,430 versus 594,946.
This proposal does not repeat those performance runs or claim a speedup.
The previous draft reported an installed-source identity probe, but this
relaunch has not recovered its receipt; it does not claim a fresh interpreter
verification. The pin authority records CPython 3.12.16, 3.13.18, 3.14.10 and
3.15.2. The retained HF-855 3.12 comparison says asyncio's entry has 47 lines and
base_events has 2,024. Molt's entry instead owns event-loop implementation.

The existing `_asyncio.py` proxies Task and Future back to the Python rewrite.
`tests/test_asyncio_private_module_surface.py` fakes that circular boundary and
therefore cannot qualify real accelerated types. Rust Future storage already
exists in `runtime/molt-runtime-asyncio/src/asyncio_core.rs`; task execution,
running-loop and context ownership also exist in `molt-runtime/src/async_rt`.
Reuse and reconcile them rather than adding another scheduler or Future store.

## Goals and limits

Every module implemented in upstream Python is selected through the typed target
from exact pinned source, plus explicitly recorded subset patches. For v1 the
vendored release is 3.15 only; this is not a claim of older-minor source parity. Source
availability, semantic support and executed qualification remain different facts.
CPython C implementations use Rust primitives and thin glue. A completed family
deletes its Molt Python implementation and every consumer of retired interfaces.
Installed compilers, repository builds, development and release use the same
selection and bytes. No generated program uses a host Python interpreter.

This design covers source acquisition, selection, packaging, provenance and the
first asyncio migration. It does not redesign optimization passes, coroutine
splitting, image heaps or whole-program summaries. Reserved frontend and compiler
paths are excluded. A reproducer requiring their change is returned to its owner;
the affected family cannot be labelled complete. Other independent families may
proceed after the integrator settles this design.

There is an explicit admission discrepancy: `CPYTHON_RELEASES` contains all four
pins, while `target_python_policy.py` admits only 3.12, 3.13 and 3.14 and defaults
to 3.12. The owner has settled admission; Part 1 corrects that discrepancy.

## Part 1: target admission and default, landed independently

`src/molt/target_python_policy.py` remains the one admission/default authority.
Represent its finite spellings with a closed `Literal` and annotated constants;
the admitted tuple gains 3.15 and the default becomes 3.15. Reuse the existing
`TargetPythonVersion` record and generated `CPYTHON_RELEASES` to resolve exact
release facts. Do not add a second version table or infer defaults from the host.
The tier owner remains `config/release_targets.toml`: remove its two 3.12
`tier1_cell` rows and retain the two 3.15 rows; regenerate `release_matrix.py`.
Do not add a competing tier field to target policy. Older targets remain in the
admitted set and the supported-target test families.

Resolve frontend and differential oracle commands through `python_interpreter`
and `PythonIdentityVerifier`, requesting the exact pinned 3.15.2 release for
qualification. The existing `PythonInterpreterRequest` already models an exact
patch; reuse it. `verify_target_python_command` currently accepts a matching
minor regardless of patch: the release-qualified path must require the exact
request, with wrong-patch and foreign-executable refusal tests. Keep a general
frontend minimum of 3.12+ distinct from release qualification on 3.15. Repository
drivers still run under the selected slot image; never change `.python-version`
or the active environment as a shortcut to choosing the oracle or frontend.
If the existing selected-image mechanism cannot execute the 3.15 frontend, fix
its owned interpreter boundary; if a change in `src/molt/frontend` is needed,
stop that item and return evidence to the integrator.

Migrate `tools/proof_plan.toml` by each cell's purpose, not a global text replace:
the Tier 1 witness paths and default-target native/LLVM/WASM differentials use
3.15; host-only tooling cells need no semantic-target claim. Preserve the serial
`r6.target-version-parity.py312/313/314` programs as cheap Tier 2 evidence and add
the equivalent 3.15 producer through the existing named-lane mechanism. Remove
old default assumptions from matrix IDs, job references, command templates and
fixtures together. A matrix row's Python value is insufficient if its argv still
forces 3.12. Preserve tests explicitly exercising old-minor behavior. Generate
workflow/proof projections through `tools/generators.py write/check`; never edit
their output manually. Release acceptance derives required Python cells solely
from the release matrix, so a Tier 2 failure remains visible without blocking a
release or silently dropping its tests.

The acceptance oracle is a build with the target selector omitted, on native
and LLVM, whose output and exact interpreter identity match isolated CPython
3.15.2. Run the existing differential basic smoke against the same oracle; an
explicit `--python-version 3.15` run alone cannot prove the default. Retain
target-metadata, wrong-oracle, removed-module and older-minor cases. Record
3.15-only semantic failures through `tools/ledger_edit.py open`, with source,
command and result evidence; ledger entry creation is not a passing result.
Reserved-path failures stop that item. No version-specific stdlib workaround
or verified-subset expansion belongs in this slice.

Update `docs/design/v0_1_support_matrix.md` with the dated owner quotations and
revised cells, plus README and getting-started default/explicit-target examples.
The checked-out AGENTS.md already says "pinned source" without the obsolete
one-pin-per-minor phrase; no gratuitous AGENTS edit is needed. That phrase is
in `stdlib_lowering_plan.md` and is reconciled in this design change.

## Architecture

### One source chain

Reuse `config/cpython_regrtest_sources.toml` for upstream commits and
`tools/gen_stdlib_module_union.py` for source capture and release generation.
`src/molt/_python_release_generated.py` stays a projection of receipted
`Include/patchlevel.h`, never a separately edited pin list.

Extend the existing `config/cpython_stdlib_snapshot.json` capture with exact
3.15 Lib bytes and their upstream Git-object/SHA-256 receipts, plus the license texts
and file modes. Keep original bytes, including encodings and line endings;
encode non-UTF-8 data losslessly. Installed uv Lib files are acquisition inputs
and independent witnesses, not proof of a complete cross-platform source tree.
Verify the interpreter's exact release through `python_interpreter` and
`PythonIdentityVerifier`, then compare each selected installed file with the
same pinned Git blob. A difference fails capture and names both identities;
never silently choose one. Files absent from the installed distribution, such as
another OS's modules, must come from the pinned upstream tree and carry that
explicit acquisition origin. Exclude site-packages, bytecode and host-generated
sysconfig data. Preserve package data required by the upstream modules.

Capture the full Python library inventory, including private helpers not rooted
in `sys.stdlib_module_names`. Keep that inventory distinct from the public module
union. The current union generator deliberately filters to public top-level
members; using that filtered inventory as the vendoring closure would lose
private dependencies. `Lib/test` is a test resource, not shipped production
stdlib; test-support use is selected explicitly by the existing regrtest runner.

The generator writes version-keyed consumed sources to
`src/molt/stdlib_cpython/3.15/`, with license and provenance data. Its schema is
version-keyed; no 3.12-3.14 source trees are captured for this v1 migration. Original
bytes live in the existing snapshot; generated consumed files are projections,
not a second implementation. Extend this generator's manifest entry and use
`tools/generators.py write/check`. Build and installation never fetch upstream,
patch source, or regenerate these files. The source snapshot is already omitted
from compiler bundles; consumed sources and their compact generated receipts
must be shipped. This keeps ordinary builds offline and avoids rehashing an
entire raw upstream tree on each user build.

### Patches and migration ownership

Store patches under `config/cpython_stdlib_patches/<construct>/<minor>.patch`.
Each construct has one record in that directory's `manifest.toml`: the existing
verified-subset rule, affected source paths, exact preimage and postimage hashes,
and ordered patch identities. A patch may have per-release hunks but cannot
combine unrelated constructs. Apply with exact context and zero fuzz into
private staging, check the postimage, then publish through existing atomic file
publication. No shell expansion, path traversal, external symlinks, executable
source imports or partial writes are permitted. Pin refresh requires explicit
reconciliation of every patch; unchanged pins with changed bytes fail closed.
No concrete subset patch is approved by this design.

For each module and semantic target the snapshot records a closed source-owner
kind: upstream Python, C glue, not yet migrated, or unavailable. This is source
custody, not the full-coverage attestation. Capture derives availability from
the pinned tree. Family migration changes custody to upstream Python only when
its replacement and consumer deletion land together. Remaining rewrites are
explicit open defects, never fallback providers for migrated names. At the final
outcome there are no not-yet-migrated Python rows and `src/molt/stdlib` contains
only C glue or compiler-private support with a documented producer.

Generate a compact source catalog alongside the consumed sources. Dense
`StdlibModuleId` and `StdlibReleaseId` index flat tuples of source records.
Generate their closed Python enum types from the captured catalog through the
existing generator. Source spellings enter once at the import-scanner boundary;
arbitrary user package names remain open-ended inputs. They do not become
string-keyed optimizer facts. No handwritten family list duplicates this catalog.

### Target selection and import topology

Add `src/molt/stdlib_source.py` as the sole compiler-side source-selection owner.
It consumes `TargetPythonVersion` and the generated source catalog and returns
`StdlibSourceSelection`. Do not read `sys.version_info`, host `sys.path`, host
`sysconfig`, `os.name` or environment selectors here. No new environment variable
is needed. Resolve target Python before stdlib selection in `cli/build_inputs.py`.

The semantic target and source release are different typed fields. For a
migrated family, explicit catalog records select the 3.15 source release; no
missing-version fallback searches for the nearest release. Tier 2 targets retain
their own target facts and removed-module availability. Qualification on 3.12-3.14
is limited to the existing cheap cases, with failures reported as Tier 2 debt.
Do not claim these records are source from an older CPython release. Once a
family migrates, its old Python implementation is deleted for all targets.
This is the principal review risk: an older target may need a construct or
semantic difference that 3.15 source cannot express within the existing contract.
Minimize that case and return it for a dated owner decision before deleting a
needed provider; do not silently raise the target, keep a compatibility rewrite,
vendor another minor, or broaden subset patches. Part 1 remains independently
landable even if this Part 2 boundary needs a Correction.

Replace the assumption of a single unversioned `Path` with this selection across
CLI build records, import resolution, graph discovery, source scanning and
fingerprinting. The selection has explicit roots for versioned Python and common
C glue, with one source record per known stdlib module. It is not an ordered
search fallback: the record names exactly one file or an unavailable result.
An absent owned file is a corrupt compiler installation and fails closed.
Normal user-module shadowing keeps the existing import-topology semantics;
once an import resolves to stdlib custody, the catalog controls its source.

Relative package imports, `from ... import ...`, submodule discovery, module-name
reconstruction, resources and traceback paths must use the selected package
identity. They cannot reconstruct paths beneath the old single root. A migrated
package must migrate its whole source package; do not load its parent from one
minor and children from another. A reachable private helper is part of the same
closure even if its public module union omits it. Version-absent modules cannot
fall through into the old union's copies.

Reuse `PythonSourceSnapshot`, source leases and `PythonImportTopology` for reads
and mutation detection. Bind catalog digest, selected release and actual source
digests into existing source closure and artifact fingerprints. The emitted
program gets existing typed target facts; it never carries the catalog or does
module-source selection at runtime. Platform branches use the existing target
plan. A failure to fold an unsupported target branch is compiler evidence, not
permission to rewrite upstream Python around it.

### Asyncio's boundary

Migrate the full target-version asyncio package first. All Python algorithms,
including event-loop policy, queues, locks, task groups, streams and callbacks,
come from upstream. `_asyncio.py` becomes thin bindings to Rust Task and Future
types and running-loop/task-context primitives. Remove its lazy proxy classes;
the new types must preserve identity, subclassing, callbacks, context, cancellation
and exception behavior. CPython's optional pure-Python reference implementations
remain upstream source; they cannot hide a missing required accelerator.

Reuse Future storage from `molt-runtime-asyncio`, task/coroutine execution and
`TaskContextBinding` from `molt-runtime`, and existing selector/socket/platform
primitives. One task owns its context and result references; its existing GC
visitation and shutdown path must enumerate every retained edge. Future completion
is a single pending-to-result/exception/cancelled transition; callbacks are
scheduled on the owning loop in registration order under captured contexts.
Callbacks may re-enter and register callbacks: detach the completed callback
batch before invoking Python; do not hold a state mutex across callbacks.
Cancellation, task wakeup and loop restoration must retain exception ordering.

Retire Rust implementations of upstream Python algorithms only after tracing
all callers and retaining a before/after cost comparison. Likely candidates
include asyncio queue, lock, semaphore and helper algorithm entries. The existing
Rust Future store is a donor, not automatically dead. `async_rt` also implements
Python coroutine protocols independent of asyncio; do not delete it as a family.
No second scheduler, name cache, method-name dispatch or pointer-keyed state store
is introduced. Fixed registered names use generated `NameId` only. Changes to
runtime bindings originate in `manifest.pyi`/`runtime_abi.toml` and regenerate
consumers. If a generator requires changes to a reserved backend, return the
cross-lane dependency before implementation.

## Global invariants

1. **Pin custody:** every upstream byte traces to the existing source pin and an
   exact blob; installed interpreter release and source origin are recorded.
2. **Target custody:** every selected source is a function of the admitted target
   and catalog, never host state; source capture alone admits no language target.
   Semantic target and source release are distinct typed identities. V1 vendors
   only 3.15; explicit Tier 2 mappings confer no older-source parity claim.
3. **Single owner:** a module/release has exactly one source owner; migrated
   modules have no legacy candidate, proxy or silent fallback. Package closure
   cannot mix minors. Unavailable and corrupt are distinct outcomes.
4. **Patch custody:** output equals exact original bytes plus the recorded ordered
   subset patches. A compiler workaround cannot be registered as a subset patch.
5. **Publication:** capture and generation are failure-atomic, deterministic and
   unchanged-input writes are zero. Compilation consumes immutable projections.
6. **Artifact closure:** source selection, package resources, licenses and source
   digests survive wheels, source distributions, bundles and shared stdlib objects.
7. **Runtime ownership:** existing task/loop/context stores have one owner;
   Future completion schedules each registered callback once without losing GC
   edges or invoking Python under a state lock. Registered names are `NameId`.
8. **Qualification:** source equality is not behavioral parity; a family lands
   only with affected consumer, differential and applicable regrtest evidence.
   Unqualified target/backend/profile cells and reserved defects stay explicit.
9. **Admission and tiers:** the default is 3.15 and the only release-blocking
   Python is 3.15; 3.12-3.14 stay admitted and tested Tier 2. Policy, exact
   interpreter selection and generated release/proof consumers agree. Existing
   generator checks and release-matrix tests enforce the single tier authority.
10. **Bounded execution:** affected concurrent/process tests have at most two
    workers, explicit 6/24/48 GiB limits and timeouts. The spinning-loop hazard
    runs alone for at most 60 seconds. A guard trip stops the family until its
    cause is fixed; raising a cap cannot qualify a run.

## Binding skeleton (illustrative, not production code)

The generated catalog owns the closed IDs and `SourceOwner` enum. `SourceOwner`
has UPSTREAM_PYTHON, C_GLUE, UNMIGRATED and UNAVAILABLE variants. `SourceRelease`
is captured release metadata independent of target admission. `VerifiedSubsetRule`
is projected from `config/verified_subset.toml`, not a free-text patch reason.
SHA-256 and Git object IDs use distinct immutable value types. Source paths are
`PurePosixPath`, admitted beneath the owning root. The wire decoder uses typed
records and exhaustive cases; no `Any` enters these interfaces.

```python
# src/molt/target_python_policy.py; sole admission and default authority.
TargetPythonShortVersion = Literal["3.12", "3.13", "3.14", "3.15"]
SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS: tuple[TargetPythonShortVersion, ...] = (
    "3.12",
    "3.13",
    "3.14",
    "3.15",
)
DEFAULT_TARGET_PYTHON_SHORT_VERSION: TargetPythonShortVersion = "3.15"


# src/molt/python_interpreter.py; retain the existing request and verifier.
def verify_target_python_command(
    command: Sequence[str],
    *,
    target_python: TargetPythonVersion,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
    verifier: PythonIdentityVerifier,
) -> tuple[bool, str]:
    """Verify the physical oracle against the pinned target (I1, I9).

    Requires: admitted target with release facts from CPYTHON_RELEASES; argv
    is a physical command, and env is the existing open OS environment boundary.
    Ensures: true only for the exact CPython release and admitted executable;
    preserve the existing false-plus-diagnostic return for failed verification.
    Raises: no new public exception; request/identity failures become diagnostics.
    """
    ...


# src/molt/stdlib_source.py; imports and generated definitions elided.
@dataclass(frozen=True, slots=True)
class StdlibSourceSelection:
    target: TargetPythonVersion
    release: StdlibReleaseId
    catalog_digest: Sha256
    compiler_root: Path
    records: tuple[StdlibSourceRecord, ...]


@dataclass(frozen=True, slots=True)
class StdlibSourceRecord:
    module: StdlibModuleId
    owner: SourceOwner
    relative_path: PurePosixPath | None
    source_digest: Sha256 | None


def select_stdlib_source(
    target: TargetPythonVersion,
    compiler_root: Path,
) -> StdlibSourceSelection:
    """Select the installed catalog under invariants 1, 2, 3 and 6.

    Requires: compiler_root is the selected compiler's admitted source root.
    Ensures: explicit target-to-source-release mapping, immutable dense records,
    no host discovery and no implicit nearest-version selection.
    Raises: unsupported-target or corrupt-installation error before graph work.
    """
    ...


def module_source(
    selection: StdlibSourceSelection,
    module: StdlibModuleId,
) -> Path:
    """Project exactly one source record, preserving invariants 2, 3 and 6.

    Requires: module belongs to this catalog; normal import precedence resolved.
    Ensures: a contained owned path, never another release or a fallback copy.
    Raises: unavailable-module for absent target membership; corrupt-installation
    for missing or invalid owned input. Existing source leases own byte reads.
    """
    ...


# tools/gen_stdlib_module_union.py; extend the existing generator owner.
def capture_stdlib_sources(
    source: CPythonSource,
    interpreter: PythonInterpreter,
    upstream: Path,
) -> CapturedStdlib:
    """Capture all relevant Lib inputs under invariants 1 and 5.

    Requires: upstream revision and exact interpreter identity are verified.
    Ensures: sorted typed file receipts, complete private-helper inventory,
    licenses, and explicit upstream-only origins for absent installed files.
    Raises: source mismatch, unsafe path, changed input or incomplete capture;
    the previous snapshot remains intact on every refusal.
    """
    ...


def render_stdlib_sources(
    captured: CapturedStdlib,
    patches: tuple[SubsetPatch, ...],
) -> tuple[GeneratedSource, ...]:
    """Render source bytes, receipts and types under invariants 3, 4 and 5.

    Requires: exact pin receipts and admitted subset-rule patch records.
    Ensures: deterministic outputs, one owner per module and zero patch fuzz.
    Raises: preimage/postimage mismatch or duplicate owner before publication.
    """
    ...
```

The asyncio implementation retains the existing ABI entry signatures until the
review settles the full accelerator surface; this skeleton binds the internal
completion boundary. `FutureRef` is a typed borrow of existing Future storage,
not a second allocation or registry. `FutureCompletion` is a generated closed
sum with result, exception and cancellation payloads. Add its declaration to the
owning runtime lifecycle schema when that schema is identified during review;
do not invent a parallel generator. This schema owner is an explicit review
question, not an implementation-ready claim.

```rust
// runtime/molt-runtime-asyncio/src/asyncio_core.rs
/// Complete the existing Future storage and detach its callback batch (I7).
/// Requires: a live typed reference and owned payload under its runtime token.
/// Ensures: one terminal transition and one ordered callback batch; no Python
/// callback runs while the state is borrowed or locked. Callback dispatch uses
/// the existing loop, carries contexts, and visits retained edges for GC.
/// Errors: InvalidState for an already terminal Future; failure preserves the
/// prior state and payload custody. Cancellation's bool API wraps this result.
fn complete_future(
    py: &molt_runtime_core::PyToken,
    future: FutureRef<'_>,
    completion: FutureCompletion,
) -> Result<ReadyCallbacks, FutureTransitionError> {
    todo!()
}
```

String audit: upstream source text, user-visible diagnostics, filesystem paths,
Git URLs and arbitrary user import spellings are open data and remain text at
their boundaries. Release/module identities, provenance roles, owner kinds,
patch rules, future state and error kinds are closed typed values. JSON/TOML
keys are wire syntax only. Digests are newtypes, not generic strings. No new
string-keyed compiler fact, untyped signature, cache or environment variable is
part of this design. `Sequence[str]` is OS argv and `Mapping[str, str]` is the
existing open process environment, not a closed compiler fact map; the boolean
and diagnostic pair preserves the existing verification interface. Version
spellings use the one policy's closed Literal and release records, not free text.
The existing pin parser's string fields are decoded once
into typed capture records; this does not authorize a project-wide parser rewrite.

## GoldfishContext and consumers

Read these files from the assigned source revision. This is a proposed consumer
map, not permission to edit reserved paths. Generated siblings come from their
listed generator. At implementation, a new consumer discovered by the same
mechanism joins the slice before landing; an unreconciled consumer blocks it.

| Boundary | Existing consumers and intended change |
| --- | --- |
| Target default and exact interpreter | `src/molt/target_python_policy.py`, `src/molt/target_python.py`, `src/molt/python_interpreter.py`, `src/molt/python_environment_identity.py`, `src/molt/host_python.py`, `src/molt/cli/build_inputs.py`, `build_pipeline.py`, `external_native.py`, `tests/molt_diff.py`, `tools/dev.py`, `.github/actions/setup-project/action.yml`: distinguish frontend execution from semantic target, route default/oracle through the policy, and preserve exact physical interpreter identity. `.python-version` and `uv.lock` retain development-image custody. |
| Tier and proof projections | `config/release_targets.toml`, `config/verified_subset.toml`, `tools/gen_release_matrix.py`, generated `src/molt/release_matrix.py`, `src/molt/verified_subset.py`, `tools/verified_subset.py`, `tools/release/consumer_replay.py`, `tools/proof_plan.py`, `tools/proof_plan.toml`, `tools/gen_proof_plan.py`, `.github/proof-plan.generated.json`, generated workflows declared by `tools/generator_manifest.toml`: remove 3.12 Tier 1 cells, preserve admitted Tier 2 targets, migrate default-target cells and their references together. |
| Admission-dependent generators | `tools/gen_stdlib_module_union.py`, `tools/gen_host_capabilities.py`, `tools/gen_token_metadata.py`, `tools/gen_python_numeric_errors.py`, `tools/check_table_drift.py`, `tools/generator_manifest.toml` and all their manifest-declared output siblings: generate 3.15 projections from pinned source and selected interpreters, without borrowing host metadata. Reserved output changes are returned to their owner. |
| Policy fixtures and direct readers | `tests/cli/test_cli_target_python.py`, `test_cli_target_python_coverage.py`, `tests/test_target_python_oracle.py`, `test_python_runtime_identity.py`, `test_module_registry_gates.py`, `test_proof_plan.py`, `test_table_drift_gate.py`, `test_generator_manifest.py`, `tests/tools/test_release_supply_chain.py`, `test_setup_project_inputs.py`, `test_dev_py.py`, `test_verified_subset.py`, `test_gen_python_numeric_errors.py`, `test_gen_token_metadata.py`, `tests/cli/test_cli_cache_fingerprint.py`, `test_cli_module_graph_cache_authority.py`, `test_cli_import_collection.py`, `test_cli_lowering_cache_tooling_scope.py`: separate default assumptions from intentional version tests and update fake release/interpreter records at their real boundary. |
| Memory and timeout readers | `src/molt/environment_registry.toml` and generated `_environment_registry.py`, `tools/harness_memory_guard.py`, `tools/memory_guard_core/harness_outcomes.py`, `tools/ci_resource_env.py`, `tests/conftest.py`, `tests/process_guard_common.py`, `tests/test_molt_diff_memory_guard.py`, `tests/test_cpython_regrtest_memory_guard.py`, `tests/test_harness_memory_guard.py`, `tests/tools/test_ci_resource_env.py`: reuse the existing registered limits, verify effective child environments, and retain guard receipts; no new environment selector or guard registry. |
| Pins and capture | `config/cpython_regrtest_sources.toml`, `config/cpython_stdlib_snapshot.json`, `tools/gen_stdlib_module_union.py`, `tools/cpython_regrtest.py`, `src/molt/_python_release_generated.py`, `src/molt/target_python.py`, `src/molt/target_python_policy.py`: preserve pin/admission split; extend existing capture and typed source projections. |
| Generated source and patches | New `src/molt/stdlib_cpython/`, `src/molt/_stdlib_source_generated.py`, `src/molt/stdlib_source.py`, `config/cpython_stdlib_patches/manifest.toml` and construct patches: source selection and reproducible subset changes. `tools/generator_manifest.toml` owns all output registration. |
| Resolution and package closure | `src/molt/cli/module_resolution.py`, `build_inputs.py`, `models.py`, `module_graph_discovery.py`, `module_graph.py`, `module_dependencies.py`, `module_import_scanner.py`, `module_stdlib_policy.py`, `python_import_resolution.py`, `python_import_topology.py`, `frontend_pipeline.py`: pass selection instead of assuming one unversioned path; keep existing topology/lease authority. |
| Source identity and artifacts | `src/molt/cli/module_source.py`, `module_graph_cache.py`, `cache_fingerprints.py`, `python_source_closure.py`, `python_source_closure.toml`, `backend_cache_setup.py`, `src/molt/logical_source_paths.py`, `src/molt/typed_cache.py`: update role tables, source captures and shared-stdlib identity consumers; no new cache. |
| Packaging | `pyproject.toml`, `tools/release/compiler_payload.py`, `tools/release/build_bundle.py`, `packaging/bootstrap.py`: package versioned source, resources and notices; preserve snapshot omission; test installed paths without checkout borrowing. |
| Audits and inventories | `src/molt/stdlib_intrinsic_policy.py`, `tools/check_stdlib_intrinsics.py`, `tools/check_stdlib_intrinsic_surface.py`, `tools/stdlib_full_coverage_manifest.py`, `tools/stdlib_intrinsics_ratchet.json`, `tools/gen_stdlib_stubs.py`, generated `tools/stdlib_module_union*.py`: iterate target-selected sources; refuse provenance-free Python ownership and stub regeneration over migrated modules. Intrinsic backing is not source origin. |
| Asyncio Python | Delete migrated `src/molt/stdlib/asyncio/*.py`; replace `src/molt/stdlib/_asyncio.py` proxy boundary with real typed bindings. Capture every upstream 3.15 package file, including private dependencies. |
| Asyncio Rust donors | `runtime/molt-runtime-asyncio/src/{asyncio_core,asyncio_queue,asyncio_helpers,bridge,lib}.rs`, `runtime/molt-runtime/src/asyncio_bridge.rs`, `async_rt/{awaitable,cancellation,generators_async_future,task}.rs`, `async_rt/scheduler/{task_state,mod}.rs`, `async_rt/scheduler/asyncio_runtime/{loops,task_registry,event_waiters,token}.rs`, `async_rt/event_loop/` and `async_rt/io_poller/`: reuse existing Future, task and I/O ownership; retire only proven dead Python-algorithm helpers. |
| Runtime registrations | `runtime/molt-runtime/src/intrinsics/{manifest.pyi,public_objects.toml,categories.toml}`, `runtime/runtime_abi.toml`, `runtime/molt-ir/src/tir/runtime_names.toml`, `tools/gen_runtime_names.py`, `tools/gen_intrinsics.py`, generated attached exports and resolvers: migrate signatures, names, public identities and retired producers together. Reserved generated outputs require integrator routing. |
| Test fakes and path readers | `tests/test_asyncio_private_module_surface.py`, `tests/stdlib_intrinsic_registry.py`, `tests/test_native_async_shutdown_refcount.py`, `tests/test_native_async_protocol.py`, `tests/test_wasm_async_protocol.py`, `tests/test_stdlib_stubs.py`, `tests/test_check_stdlib_intrinsics.py`, `tests/test_stdlib_intrinsic_import_evidence.py`, `tests/test_stdlib_intrinsic_registry.py`, `tests/conftest.py`: replace circular fake success and migrate source-path/finalizer lists. Keep independent lifecycle oracles. |
| Compiler/source tests | `tests/cli/test_cli_module_resolution_authority.py`, `test_cli_module_stdlib_policy_authority.py`, `test_compiler_source_root.py`, `test_cli_cache_fingerprint.py`, `test_cli_source_closure_receipt.py`, `test_cli_python_source_closure.py`, `test_source_scan_authority_closure.py`, `test_installed_compiler.py`, `test_cli_shared_stdlib_cache.py`, `test_cli_native_link_stdlib_cache.py`, `tests/test_target_python_oracle.py`, `tests/test_cpython_regrtest_memory_guard.py`, `tests/tools/test_gen_stdlib_module_union_memory_guard.py`: extend existing source/target and pin fixture boundaries. |
| Proof selection | `tools/proof_plan.toml`, `tools/gen_proof_plan.py`, `.github/proof-plan.generated.json`, `tools/gen_diff_lanes.py`, `tests/differential/stdlib/TESTS.txt`, `tests/differential/basic/CORE_TESTS.txt`, `tests/differential/COVERAGE_INDEX.yaml`, `tools/cpython_regrtest_core.txt`, `tools/cpython_regrtest_skip.txt`, `tools/suite_honesty/differential_expectations.json`: register new source globs and changed readers; select real producers and remove only proven obsolete skips. |
| Public contracts | `README.md`, `docs/getting-started.md`, `docs/design/v0_1_support_matrix.md`, `docs/spec/areas/compat/plans/stdlib_lowering_plan.md`, `docs/internals/source_scan_authority.md`, stdlib surface/audit generated documents and `docs/agent/V1_HANDOFF_FINDINGS.md`: quote the owner decisions in changed owning docs, explain selected source and qualify only demonstrated cells. No parallel burndown. |

The retained `logs/agents/stdlib-ast-diff/{stdlib_ast_diff.py,stdlib_report.py}`
are audit inputs, not production runners. Their flat-root and handwritten
four-version assumptions must consume explicit selected source roots and pins
when rerun. Keep historical similarity counts for comparison and add exact
source/patch provenance classification. Do not hide the Rust-backed Python
rewrites, zero-callable modules or unresolved dynamic surfaces behind a green
similarity count. Keep C-module stubs in a separately listed residual inventory.

Extensions beyond the starting aperture are `pyproject.toml`, `packaging/`,
`.github/` generated workflows and setup readers, `runtime/molt-runtime-asyncio/`,
`runtime/molt-runtime/`, `runtime/runtime_abi.toml` and the existing runtime name
authority `runtime/molt-ir/src/tir/runtime_names.toml`, with generated outputs
owned by these inputs. `.python-version` and `uv.lock` are read-only environment
authorities in this proposal. No reserved path or descendant is writable.

## Verification and rollout

1. After integrator review, implement and land Part 1 with default native/LLVM
   builds, existing basic smoke and serial older-minor Tier 2 cases, then the
   generators check, exact-path consumer gate and landing precheck. Only then
   make source capture/selection and the asyncio closure concrete. Use the
   exact 3.15 source pin and interpreter identity. Keep
   capture independent from target admission. Stop at a source mismatch before
   generating replacements. Measure snapshot and package bytes so the duplicate
   original/projection storage cost is visible.
2. Extend existing source/generator tests: wrong tag/revision, wrong interpreter
   patch, modified installed source, missing cross-OS file, non-UTF-8 resource,
   unsafe path, patch preimage mismatch, failed publication, duplicate ownership,
   removed module and unchanged-input no-write. Test that source capture cannot
   itself change admission. A crafted fixture must fail for a plausible provenance defect.
3. Exercise versioned resolution with a host/target mismatch, relative imports,
   package resources, negative membership, corrupted installs and stale shared
   stdlib artifacts. Build from installed payloads as well as the checkout.
   No test may fake a generated source catalog with an impossible record shape.
4. Qualify asyncio with isolated exact-release CPython as the independent oracle.
   Start with existing `asyncio_basic.py`, `asyncio_sleep_result.py`,
   `asyncio_future_protocol_identity.py`, `asyncio_future_callback_context.py`,
   `asyncio_future_invalid_state_messages.py`, `asyncio_gather_cancel_order.py`,
   `asyncio_task_current_identity.py` and task-group exception-order cases, then
   the remaining affected asyncio differential family. Exercise native and LLVM
   using one `tests/molt_diff.py --build-profile release --target native,llvm
   --jobs 2` invocation per selected batch under the caps below, and the owning
   WASM cell under the same limits. One diff
   run per checkout; sources remain fixed while any dependent build runs.
5. Select CPython `test_asyncio` cases admitted by spec 0216, including Futures,
   Tasks, contexts and cancellation. Existing broad asyncio skips are not proof.
   Inspect the runner's selected tests and actual result records; a skipped or
   host-only run proves no compiled behavior. Reconcile `host_python_cmd`'s
   legacy project-scoped uv selection with the canonical resolved interpreter
   before using it. Reserved compiler failures get a minimized input, exact
   source/tool/target identities and diagnostic; no stdlib workaround.
6. Runtime changes select whole-library tests of touched crates and reverse
   dependencies, Clippy and builds in their declared feature/profile cells.
   Reuse matching receipts and `check_changed_crate_tests.py`; a focused filter
   or a zero-test producer is insufficient. Include GC cycles, repeated loop
   construction, cancellation races and shutdown/refcount behavior.
7. Compare the same whole programs before/after and against CPython `-I`:
   retired instructions and sampled runtime calls per iteration for await,
   sleep(0) and gather; output, wall time, peak process-tree RSS, allocations,
   startup, binary/package size and cold/no-change compile time. Use existing
   `opt_explorer`/performance authorities and platform time tools. Do not claim
   that source vendoring by itself fixes direct coroutine-call lowering.
8. Rerun the AST audit per family with exact selected version roots. Require
   byte/patch provenance and deletion of the old owner as well as falling
   rewrite counts. Reconcile required intrinsics and generated audits. Run the
   exact-path consumer gate, then the mandatory plain-environment landing
   precheck. Commit each green family and use the authorized leased landing;
   the integrator reviews landing and owns lane retirement. No family is
   reported landed without observing its SHA on main.

After asyncio, rank families by imports in the existing differential corpus and
benchmarks, recording static versus observed import evidence separately. Do not
invent the ranking now. Advance complete package closures; where a reserved
defect blocks one, preserve its evidence and return that item to the integrator.
No extra runner, proof registry or standing benchmark gate is proposed.

### Mandatory memory envelope

Every differential or test run executing asyncio, threading, socket, subprocess
or event-loop programs has at most two jobs (pytest `-n 2`, regrtest `--workers 2`,
or the runner's equivalent). Use these existing environment variables explicitly:

```text
MOLT_DIFF_MAX_PROCESS_RSS_GB=6
MOLT_DIFF_MAX_TOTAL_RSS_GB=24
MOLT_DIFF_MAX_GLOBAL_RSS_GB=48
MOLT_MAX_PROCESS_RSS_GB=6
MOLT_MAX_TOTAL_RSS_GB=24
MOLT_MAX_GLOBAL_RSS_GB=48
MOLT_TEST_PROCESS_TIMEOUT_SEC=120
```

Inspect effective child limits before a run, including runner-specific timeout
overrides; setting a variable unused by a runner proves nothing. The shared
fallbacks carry the same caps. A suite's shorter per-test timeout is retained.
Never run the full asyncio corpus at default parallelism. The known
`asyncio_close_running_loop` hazard spins when `loop.stop` is called by a
`call_soon` callback inside `run_forever`: until fixed, exclude it from family
batches and execute it alone with one job and a verified 60-second per-test
timeout under the same RSS limits. After a guard trip, stop that family, retain
the guard receipt and fix the cause before resuming; never raise a cap. Do not
kill ambiguous host processes. No asyncio execution is needed for this design
artifact, and none is claimed here.

Invoke tools from this checkout through `env -u VIRTUAL_ENV
-u UV_PROJECT_ENVIRONMENT uv run --no-project python tools/venv_exec.py ...`.
For landing precheck also unset `CARGO_TARGET_DIR` and select a data-volume
`TMPDIR`, using the runner's plain environment. Never pass uv `--python`, call a
bare `python3`, or source a lane environment script. Save receipts under
`logs/agents/lane-stdlib-source/artifacts`. Freeze sources during dependent
builds; execute one differential run per checkout. The pre-review commit is
design-only, not a qualified implementation or release.

## Alternatives and risks

**Read the build host's Lib on every compile:** smaller checkout, but changes
semantics with host installation, misses cross-platform files and fails offline
installed-compiler custody. Rejected for invariants 1, 2 and 6.

**Four vendored minor trees now:** maximizes exact source correspondence but
multiplies patch reconciliation and qualification ahead of the first release.
The owner explicitly chose 3.15-only vendoring for v1. The mechanism remains
version-keyed; adding another source release later needs its own receipts.

**An implicit shared-default tree:** smaller catalog, but masks older-target
source provenance and missing/removed APIs. Reject silent fallback. Explicit
target-to-source records expose the chosen 3.15 source and Tier 2 limits; if a
required older-minor behavior cannot be preserved, obtain a Correction rather
than calling it supported or retaining the rewrite.

**Download a content-addressed full tree at build time:** sound if all custody
and distribution machinery exists, but adds network/install failure modes and
build latency here. Checked-in generated consumed sources reuse the current
snapshot/generator/package mechanism. Revisit storage only with measured package
or checkout costs; do not add a second artifact store speculatively.

**Ordered overlay paths or retained proxy shims:** apparently easier incremental
migration, but silently selects legacy code and can mix package versions. Explicit
per-module ownership is required. Temporary unmigrated records expose debt and
are deleted with their last producer; they never shadow a migrated record.

**Replace Rust async wholesale:** discards working task, context, I/O and lifetime
mechanisms with no measured replacement. Reuse those donors; remove only the
algorithms whose Python producers have migrated and whose cost is recovered.

Principal risks are upstream constructs outside spec 0216, compiler limitations
on large upstream modules, incomplete `_asyncio` type identity, source-capture
size, and target/OS resource omissions. Exact source capture and failure-atomic
publication address supply-chain drift; adversarial source/installation tests
address missing inputs. Runtime ownership and differential tests address semantic
drift. The reserved-path rule and unresolved Tier 2 source-compatibility risk remain explicit limits,
not permission for patches. No implementation, parity, performance improvement
or family landing is claimed by this design.

## Primary sources and review questions

- CPython source identity is the 3.15 commit in
  `config/cpython_regrtest_sources.toml`; inspect `Lib/asyncio`,
  `Modules/_asynciomodule.c`, `Lib/test/test_asyncio` and `LICENSE` at that pin.
  Older pin records remain interpreter/metadata authorities, not a mandate to
  vendor older trees. The retained audit was read from the canonical checkout's
  `logs/agents/stdlib-ast-diff/report.md` on 2026-11-16; this slot has no copy of
  the old interpreter-probe receipt. Its historical counts are not rerun proof.
- [Python license](https://docs.python.org/3/license.html): retain upstream license
  and copyright notices, incorporated-code notices and a summary of changes in
  source and binary distribution. Ship the pinned notices, not this web page.
- [GraalVM Native Image build configuration](https://www.graalvm.org/latest/reference-manual/native-image/overview/BuildConfiguration/)
  documents configuration packaged with its owning artifact. The applicable
  principle here is self-contained compilation inputs; it does not prove Python
  compatibility or authorize runtime class loading.
- EGM's installed `resources/document_standards/feature-spec-standard.md` supplies
  the alternatives/risk bar. The dated Correction below records the integrator verdict.

The integrator's review must preserve settled 3.15 admission/default and tier
policy, and settle the exact generated lifecycle type owner for accelerated
Future/Task state and the accelerator's public type binding surface. Challenge
the explicit 3.15-source mapping for older semantic targets against the promise
to keep Tier 2 buildable, complete package/resource closure, target selection
ordering, migration rows as explicit debt rather than fallback, all direct path
readers, fake `_asyncio` success, and deletion of any fast path before cost proof.
The dated Correction below settles those questions for implementation.

## Relaunch evidence boundary

Inspected slot-04 on `lane/stdlib-source`, HEAD
`3720765f73bc4dac0343b0a0a1b29782c5f0c85c`. Its two existing dirty paths were this
draft and the lowering-plan pointer; both were reused. Relevant source history:
`0a9931e7fc` separates target admission from pinned release facts, `d0a4d83169`
owns the closed-world stdlib architecture, and `3720765f73` opens HF-855.
The startup context command exited 2: eight errors and one warning in the
existing proof-queue audit, with no active proof record or coordination claim.
This is not a compiler failure or a passed qualification. The startup upstream
tip `da713e6eb5e079a729c093b895c43866a031a53a` adds HF-858 in the ledger and
finding-ID projection. Preserve those independent changes on integration.

This revision reconciles four mechanisms in the prior draft: stale target
admission/default assumptions and their release/proof siblings; four-minor
capture scope versus the owner's 3.15-only source scope; unsafe four-job asyncio
proof guidance and its indirectly selected test siblings; and an unreceipted
interpreter probe stated as fresh verification. Only the design and owning
architecture pointer change now. All native/LLVM/WASM, Python-minor, platform,
package and runtime behavioral cells remain unverified by this relaunch.

## Integrator Correction (2026-11-16)

Design review by the integrator (Claude). The design is accepted with these settlements;
implementation starts with Part 1.

1. **Future/Task lifecycle owner.** One typed lifecycle state machine (AGENTS: typed
   identity and state machines from one generated source), owned by `molt-runtime-asyncio`,
   reusing its Future store. The `_asyncio` public types (Future, Task, the running-loop and
   task-context entry points) are runtime classes declared in
   `runtime/molt-runtime/src/intrinsics/manifest.pyi` / `runtime/runtime_abi.toml` and
   regenerated. They are bound by typed id and generated `NameId` only, never looked up by
   name.
2. **Older semantic targets (Tier 2: 3.12, 3.13, 3.14) under 3.15-only vendored source.** A
   family migrated to vendored source (asyncio first) is REFUSED for an older target, with a
   diagnostic naming the family and stating that it is available for target 3.15 in this
   release (AGENTS: refuse unsupported cells early with useful diagnostics). No Molt Python
   rewrite is retained to serve older targets, because that would be a second semantics.
   Older targets stay buildable for programs that do not use a migrated family. Their
   failing Tier 2 cells become owned expectations on an open ledger row, never silent drops.
   This also avoids parsing 3.15-only syntax with an older frontend. Per-minor vendored
   trees are post-v1 work (owner: "later we can explore supporting more minor versions").
3. **Accepted as written:** package and resource closure from the full inventory (not the
   public union); target selection by the typed target only; migration gaps as explicit
   ledger debt, never fallback; every direct path reader migrated; `_asyncio` must refuse,
   never fake success; Rust fast paths deleted only with a before-and-after cost comparison
   and every caller traced.

## Part 1 status (2026-11-18)

Part 1 is complete: `target_python_policy` admits 3.12-3.15 with default 3.15;
`config/release_targets.toml` holds only the two 3.15 Tier 1 cells; oracles and
the regrtest host resolve the exact pinned release. Admission landed on
2026-11-17 with the default held at 3.12; the default moved to 3.15 when HF-901
closed. Verification settled four points:

1. **PEP 667 frame-locals writers are a closed-world fact (HF-901).** From 3.13 a
   callback may rebind a running function's fast locals through a `frame.f_locals`
   proxy. The namespace graph now records every `f_locals` read (an attribute
   read, or the literal name passed to a call) and names the program's first one,
   or a native extension, as `PythonNamespacePremise.frame_locals_writer`. With
   none, a 3.13+ target keeps a function's locals in SSA across callbacks as 3.12
   does; with one, it keeps the conservative 3.13 rule, and the build's binding
   diagnostics name the writer. Before the fact, target 3.15 retired 11.0x the
   instructions of 3.12 on bench_attr_access and 7.6x on bench_struct.
2. **Older frontend, newer target.** The development image stays CPython 3.12
   (owner correction above). `_parse_source_for_target` parses with the host
   grammar and names the target's frontend only for source the host cannot parse.
   Evidence: over 9,320 files of CPython 3.12.16 and 3.15.2 Lib and Molt's own
   tree, every file both parsers accept has the same normalized AST, and no file
   that 3.12 accepts is refused by 3.15. Qualification cells still compile under
   the exact target release (molt_diff's managed frontend).
3. **`project.requires-python` constrains the target.** The default target
   serves when the project admits it, else the newest admitted target it admits.
4. **Absent 3.15 modules.** The 3.15 union adds 42 module paths Molt lacks.
   No stub stands in; `molt.stdlib_intrinsic_policy.UNAVAILABLE_STDLIB_MODULES`
   declares them, the union gate refuses a stale entry, and Part 2 retires each
   entry with its source family. Importing one raises `ModuleNotFoundError`.

Proof-plan cells keep the development interpreter (3.12) as their harness. Every
differential lane names its target explicitly; `r6.target-version-parity.py315`
compiles and compares on the pinned 3.15 release, and the lanes that name 3.12
stay Tier 2 cells.
