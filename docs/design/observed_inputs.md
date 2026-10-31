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
last 24 hours first, changed test files next, then direct readers of changed paths. The dependency
graph supplies the first hop, without closing importers transitively. A test's first hop is every
edge of its own module: the module body, its functions and its literal data reads. Path rules
retain their declared commands and scopes. A rule that runs the same or a wider scope of a test
absorbs that graph request; a narrower rule scope (`-k`) does not, so the direct reader also runs
whole. Direct inputs and changed tests run their slow probes too; the ordinary cells exclude slow probes,
which remain owned by the existing `-m slow` postsubmit cell. Failure history orders execution;
it never substitutes for a passing result. Pytest's parallel scheduler may finish files out of order.

Postsubmit runs the full transitive selection on the newest main head when no landing is pending,
using the same proof executor and receipts. A new request cancels postsubmit. Only a passing
postsubmit advances `refs/molt/last-green` by compare-and-swap; new lanes use it unless their base
is explicit, and say when they fall back to `origin/main`. A failure retains last green, names
every commit since it as a suspect and prints the exact revert command for the integrator. The
runner never reverts. The integrator restores green within about an hour. If transitive misses
exceed about one landing in five, retain the cases for an owner decision about widening presubmit.
Observed-input selection and verdict reuse remain deferred stages 4-6, not implemented by this slice.

## Stages

Order is by wall time saved per unit of work. Each stage lands as one complete migration and deletes
the copy it replaces.

| Stage | Change | Measured or expected effect |
| --- | --- | --- |
| 0 | A cancelled gate is recorded as `cancelled`. The runner decides by `failure_trigger` first and bisects every candidate failure (HF-605). | -7,428 s per day (8 identical-tree reruns) |
| 1 | Fast presubmit: changed tests and first-hop readers, recent postsubmit failures first, with `--maxfail=1`. Rule-owned commands retain their scope; expensive real-process probes run in their slow cell except at a direct changed input. | Historical replay caught 24 of 31 failed blocks with a median 212 files; live before/after wall time remains required |
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
