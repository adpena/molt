# Observed inputs and content identity

Status: design approved by the owner, 2026-10-30. Owner: one agent owns ledger family F
("Observed inputs and content identity") and every consumer migration below. This document
replaces `foundation/74_build_work_deduplication.md`. It is the only authority for the rules in
"Invariants". A consumer document states what its own step reads and links here; it does not
restate these rules.

## Problem

Every reuse, selection and freshness decision in Molt asks one question: what did this step read,
and is that content unchanged? Molt answers it in many partial copies:

- **Content identity.** `molt.file_hashing` applies the racy-git rule and has 55 consumers. 29
  other modules key their own hashes by `st_mtime_ns`, `st_ino` or `st_dev` (for example
  `tools/release/archive.py`, `molt.python_file_node_custody`, `tools/disk_guard.py`,
  `molt.cli.native_link_tool_identity`, `tools/proof_queue_pkg/command_identity.py`,
  `molt.toolchain_identity`, `molt.toolchain_facts`, `molt.build_record`).
- **Input sets.** Hand-written lists guess what a step reads: proof-plan authority inputs, the
  Python capture authority, perf-methodology globs (HF-601, HF-591), guessed Cargo crate roots, and
  a static import graph for test selection (HF-605).
- **Durable identity.** Records key files by `st_dev`, which changed from 16777240 to 16777246 at a
  reboot on 2026-10-30. Every guarded command then refused (HF-604).

What the copies cost, measured on 2026-10-30
(`logs/agents/lane-gate-cost-research/artifacts/report.md`):

- **Gate time.** 36 heavy landing blocks took 34,438 s. The 32 failed blocks took 31,054 s (90%).
- **Selection.** Each hub module selects 983 of 983 test files. The harness-aware closure selects
  340 for the same batch.
- **Reuse.** Only 13-17% of test-seconds are reusable, because no verdict is keyed by what the
  test read.
- **Merge hazard.** Two lanes that each add an authority input merge as text but leave a stale count
  in `PROOF_PLAN.generated.md` (440 against 442). That stale count failed three landing batches on
  2026-10-30.

## Prior art

- **Go** (`cmd/go/internal/cache`, `go help test`). An action ID hashes every input. The test cache
  logs the files and environment variables a test reads, then re-hashes them before it reuses a
  result. File hashes are cached by stat key.
- **Bazel, Buck2, Remote Execution API v2.** An action is a command digest, an input-root digest
  and a platform. Results live in an action cache over a content store. Tests are actions
  (`--cache_test_results`); `external` tests are never reused.
- **Build Systems à la Carte** (Mokhov, Mitchell, Peyton Jones, 2018); Shake; Salsa. Dependencies
  are recorded when they are read (dynamic dependencies). Traces are verified against current
  content. Early cutoff stops at an unchanged output.
- **Git** (`Documentation/technical/racy-git.txt`) and **Watchman** (`content.sha1hex`). A digest is
  cached by stat key. A file changed within the timestamp tick is rehashed.
  [Linux multigrain timestamps](https://docs.kernel.org/filesystems/multigrain-ts.html) separate
  clock precision from timestamp generation, and
  [Windows file times](https://learn.microsoft.com/en-us/windows/win32/sysinfo/file-times) can
  stay unchanged until a writing handle closes, so resolution alone never proves a stat key.
- **Google TAP** (Memon et al., ICSE-SEIP 2017), **Chromium CQ**, **LLVM**. Presubmit runs only the
  tests a change can reach. Postsubmit runs everything on trunk, finds the culprit by bisection, and
  routes the failure. Chromium reruns a failed suite "without patch" to tell a real failure from a
  flake.
- **MLIR.** One infrastructure serves many dialects. Here, one substrate serves every consumer.

## Invariants

1. **One content identity.** `molt.file_hashing` is the only code that maps a file to a content
   digest. Its cache key is the durable file identity (volume UUID and file ID, HF-604) plus size,
   mtime and ctime; ctime catches a same-size rewrite that restores mtime. It applies the
   racy-git rule. Windows file times can stay unchanged until a writing handle closes, so
   Windows recomputes the digest instead of trusting the stat key. Every other stat-keyed
   hash is a defect.
2. **Inputs are observed, not guessed.** A step's input set is what it read: files and directory
   listings from the Python audit hook, dep-info for Rust, the module graph for the frontend, and
   tool identity for probes. Environment inputs are the typed variables from the environment
   registry. A hand-written input list may exist only as an oracle that checks the observed set.
   The observed set is then the authority, and the list is deleted.
3. **Two-level keys.** Level 1 hashes the declared inputs: action kind, argv, toolchain identity,
   proof cell and typed environment. Level 2 re-checks each observed input's current digest. A
   result is reused only when both levels match. Action kinds form a closed, generated type.
4. **Work is never redone, and failure keeps progress.** A step whose two-level key matches is
   skipped. Each sub-step records its result when that sub-step succeeds, not at the end of the
   build.
5. **Only hermetic passes are reused.** An action that reads the network, the wall clock, a farm
   host or an unrecorded input is `external` and always runs. A recorded failure orders work
   (failure first) and is never a verdict.
6. **Every cache is attested.** Each lookup records key, hit or miss, the first changed input and
   wall time. Hit rate is a gated metric.
7. **One store per host.** Records are content-addressed. Retention is by last use under a liveness
   lock.

## Consumers

Each consumer reads the substrate. None keeps a private hash, input list or selection rule.

| Consumer | Document | What it owns | What it gives up |
| --- | --- | --- | --- |
| Cargo freshness | `cargo_input_authority.md` | Rust dep-info and build-script inputs | Guessed crate roots, private file hashing |
| Frontend early cutoff | `build_early_cutoff.md`, `frontend_semantic_fingerprint.md` | Module-graph reads and lowering producers | Private record keys |
| Toolchain probes | `toolchain_fact_cache.md` | Probe argv and tool identity | Private stat keys |
| Presubmit selection and verdict reuse | `landing_queue.md` | What a landing runs, bisection, fail-fast | Static-graph selection, per-gate verdicts |
| Postsubmit | `trunk_loop.md` | Full runs on the farm, culprit finding | Its own input recording |
| Proof authority inputs | `tools/proof_plan.toml` | Gate argv and cells | Hand-written input lists (HF-601, HF-591) |
| Durable state identity | HF-604 | Volume UUID and file ID | `st_dev` keys |

The current landing policy uses a typed `PRESUBMIT` / `POSTSUBMIT` selection mode. Presubmit runs
static gates, then graph-selected pytest with `--maxfail=1`: tests that failed postsubmit in the
last 24 hours first, changed test files next, then the direct readers that import or run a changed
source (data readers wait for postsubmit; [landing design](landing_queue.md), section 9). The dependency
graph supplies the first hop, without closing importers transitively. A test's first hop is every
edge of its own module: the module body, its functions and its literal data reads. Path rules
retain their declared commands and scopes. A rule that runs the same or a wider scope of a test
absorbs that graph request; a narrower rule scope (`-k`) does not, so the direct reader also runs
whole. Changed tests and tests that import or run a changed source run their slow probes too; a
reader reached only through its harness or data runs with `--skip-slow`, and the ordinary cells
exclude slow probes, which remain owned by the existing `-m slow` postsubmit cell
([landing design](landing_queue.md), section 9, part 10). Failure history orders execution;
it never substitutes for a passing result. Pytest's parallel scheduler may finish files out of order.

Postsubmit runs the full transitive selection on the newest main head when no landing is pending,
using the same proof executor and receipts. A new request cancels postsubmit. A postsubmit whose
runnable commands all pass advances `refs/molt/last-green` by compare-and-swap. Last green is the delta
base for the next postsubmit, not a branch point: new lanes start at the fetched `origin/main` (or an explicit
`--base`) and report how far main is past last green (HF-767). A failure retains last green, names
every commit since it as a suspect and prints the exact revert command for the integrator. The
runner never reverts. The integrator restores green within about an hour. If transitive misses
exceed about one landing in five, retain the cases for an owner decision about widening presubmit.
Presubmit guest-input edges of a qualified whole-file command can now be refined to pytest
node IDs by postsubmit observations (`tools/test_dependency_graph.py`, HF-633). The command's
`pytest_observed_inputs = true` opts in; initially only the import-collection owner does so.
The controller issues a context through
the existing build-request key: host Python sources, configuration, command declarations,
interpreter and launch environment must match. Runtime, stdlib and differential guest sources
remain individual inputs. Host Python and configuration bind the context; other data changes
retain their static requests. Execution-control IDs use the existing build-request session-noise
authority (`CODEX_TOOL_CALL_ID`, like terminal-session IDs), without a selector-private whitelist.
Changes to host code, tests or their harness
retain the file-level obligation. Postsubmit still uses the full static transitive closure and
loads `tests.observed_inputs` explicitly; ordinary pytest startup gains no observer.
The opt-in also supplies that plugin as a dependency of its owning test file, so plugin and
recorder changes select the actual CLI consumer through the static postsubmit closure.

The recorder retains every collected parameter before marker/deselection filtering, collection
reads, file opens and directory listings, canonical `input_report` cached-use events, and shared-fixture reads for
all later consumers. Unobserved subprocess/network activity, failed/skipped cases, incomplete
inventories and malformed/missing records retain the existing static obligation. Observations
qualify absence only after successful collection and, for a node, the ordered successful
setup, call and teardown reports plus a normal protocol return. Interrupts and missing phases
publish incomplete observations even when pytest produces no failure report. Observed reads
also add data readers that static inference missed. A recent failure retains the whole file,
absorbing any narrower node requests. Every node keeps its existing proof-cell envelope.
Each inventory and node references an immutable guest-source snapshot in the same fact cache.
A changed recorded input or directory retains its reader even when it predates the current diff:
an old dependency set cannot hide a newly conditional read. Partial postsubmit preserves older
node records whose reads and current collection inputs are unchanged; missing snapshot facts
stay unknown. Completeness proves absence only: a known positive read is retained even for an
opaque node that the static graph missed.

Records live beside the existing dependency-fact cache. Atomic publication never changes a
verdict; the existing cache-recency and build-record retention authorities govern their use and
retirement. A cold or incompatible context therefore runs the static readers until postsubmit
has captured it. No passing verdict is reused. General action-store migration and verdict reuse
remain deferred stages 4-6; this refinement does not qualify unobserved target cells.

The compiled runtime-WASM signature oracle resolves the checkout's dev/full Cargo
plan through `resolve_runtime_wasm_pair_identity`, the pair producer's input-identity
authority. It reads only `<family digest>.source-build.expected.json`, with the
receipt role and path owned by `RuntimeWasmExpectedPair.path`, and admits the
selection against those exact shared/reloc identities. Other candidates' records
cannot establish this checkout's compiled truth. Missing, substituted or damaged
candidate evidence fails as `candidate-runtime-wasm-pair-unavailable`; it never
skips. The local `runtime-wasm-signatures` rule puts the existing atomic pair
producer before the oracle in the shared-target Cargo dependency chain for both
presubmit and postsubmit. The WASM CI command depends on that same producer;
Python-only surface cells no longer claim the compiled obligation. Final preflight
uses the same identity resolver and receipt path, including the source-build role.

Before execution, `GateConfig.prepare` resolves each selected command to the closed
`Runnable | Deferred` outcome. A deferred command names its missing declared
toolchain, pinned asset or foreign portability execution cell and the owning
proof cell id, validated against the matrix and its command producers. Diagnostic
prose renders that id; missing tools also render their policy's `setup_value`
and `setup_evidence`. Foreign cells enter this same outcome;
the dependency graph no longer silently drops those commands. Toolchain admission
retains typed absent-image evidence from its existing probes and provider projection.
A present image with the wrong version, a raised probe, failed provisioning or
any command that runs and fails still fails the landing. An undeclared tool is
never grounds for deferral. No error-message parsing grants a deferral.

Pinned assets are declared in `proof_plan.toml`; their pin documents alone own
content identity. `proof_plan.provision_pinned_assets`, called at the same
pre-execution preparation boundary as toolchain provisioning, uses immutable
Git objects in the repository's primary checkout to provision one verified
generation per asset name and pinned commit in DX's `pinned-assets` storage
class, under `<artifact root>/molt-state/pinned-assets`, outside all checkouts.
Provisioning holds the canonical shared-cache lock for that key. Every lane
and sandbox reaches the same generation through a directory symlink at the
declared path. Lane payload discovery recognizes links to the declared store
key as regenerable projections, splitting ignored parents to preserve unknown
siblings. Verified regenerable assets never become lane evidence. A missing verified donor defers; an
older verified in-checkout generation migrates into the store and becomes a link;
changed or unpinned entries block migration and remain in their owner's custody. An
existing destination with changed content fails. `docs/spec/tinygrad_pin.md`
owns the tinygrad commit, rather than a version-only provisioning check.
Execution preparation owns the single full content scan; payload discovery
does not repeat it for a store link. A corrupt external tree still fails admission.

Runs retain `prerequisites.json` in the executor receipt's `.d` directory, with runnable and
deferred commands, atomically published through `molt.file_publication` even
when nothing deferred. Its location is independent of the optional per-run
logs; inability to retain selection evidence fails preparation before execution.
Postsubmit's result receipt also lists the deferred commands,
and its green summary explicitly calls them unproven here. Last green therefore
attests the runnable host selection, not full farm or release qualification.
This follows [Bazel's incompatible-target selection](https://bazel.build/versions/9.2.0/extending/platforms)
and [Buck2's target compatibility](https://buck2.build/docs/concepts/configurations/):
declared platform incompatibility can skip work, without calling it passed.

## Stages

Order is by wall time saved per unit of work. Each stage lands as one complete migration and deletes
the copy it replaces.

| Stage | Change | Measured or expected effect |
| --- | --- | --- |
| 0 | A cancelled gate is recorded as `cancelled`. The runner decides by `failure_trigger` first and bisects every candidate failure (HF-605). | -7,428 s per day (8 identical-tree reruns) |
| 1 | Fast presubmit: changed tests and first-hop readers, recent postsubmit failures first among those readers, with `--maxfail=1`. Failure history adds no unrelated tests. Rule-owned commands retain their scope; `slow` probes run in presubmit only for a changed test or one that imports or runs the change. | Historical replay before the 2026-11-05 selection correction caught 24 of 31 failed blocks with a median 212 files; live before/after wall time remains required |
| 2 | One CPU budget per host that knows performance cores. Receipts record `-n` and CPU time. | Not yet measured |
| 3 | A typed layer table: `src/molt` never imports `tools`. Break the 38-file import cycle. Shrink the harness kernel. | 983 to 340 files per batch |
| 4 | Deferred by the owner: `molt.file_hashing` becomes the only content identity, keyed by durable file identity; delete private copies. | One hash per file per host |
| 5 | Deferred by the owner: observed-input records (audit-hook reads, dep-info) in one host action store, with two-level lookup. | 13-17% of test-seconds today; more after stage 3 |
| 6 | Deferred by the owner: derive selection, proof authority and perf-methodology inputs from records; retire the hand lists and merge-sensitive counts. | Removes HF-601 and HF-591 drift and the projection merge hazard |
| 7 | Queue postsubmit runs the full selection on newest main after pending landings, retaining last green and red-head suspects for integrator-directed restoration. | Main health is restored without complete presubmit runs; cross-host farm qualification remains open |

## Costs to measure before stage 5

- **Audit hook.** Cost per `open` in a test process, and the size of the record each test writes.
- **Stat cache.** One `stat` per observed input per lookup. Report the p50 and p99 input counts per
  test file.
- **Store.** Bytes per record, records per day, and retention reclaim.

## Not decided

- **Digest choice.** The current digest stays until a measurement shows hashing cost on a hot path.
- **Remote action cache across farm hosts.** This waits until local reuse is measured.
