# State machines: catalog, invariants and transition tables

Status: living document, first audit 2026-11-06. Owner: integrator.
Owner rule (2026-10-29): every lifecycle is an explicit typed state machine with
an exhaustive transition table; untyped state strings are poison.

## Scope and method

This file catalogs Molt's lifecycles in one place. It audits in depth the
machines that persist state and run with retries or concurrency: farm runs,
landing requests, landing attempts, gate retries, post-submit verdicts and
artifact publication. For each, it lists states, invariants and the transition
table, then the invalid states or transitions the code can reach.

A finding counts as **confirmed** only with evidence: a reproduction against the
code before the fix, or records on disk that show the state. Each fix carries a
regression test. Machines marked **inventory** have their states listed and an
owner named; nobody has audited their reachable invalid states yet.

## Catalog

| Machine | Kind | Authority | Retries / concurrency | Audit |
| --- | --- | --- | --- | --- |
| Farm run (`run.json`) | typed (`RunState`, `RUN_TRANSITIONS`) since this audit; was untyped strings | `tools/trunk_loop.py` | remote worker, detached launch, collect retries | deep |
| Farm run / landing verdict | typed (`TerminalStatus`) since this audit | `tools/trunk_loop.py`, `tools/trunk_landing.py` | written by the remote worker, read by collect | deep |
| Landing request | implicit (request file, then one result) | `tools/molt_dev_queue.py` | runner lock; bisection; withdraw | deep |
| Landing request result | typed (`LandingStatus`) | `tools/molt_dev_queue.py` | as above | deep |
| `land --fast` attempt | implicit loop, `MAX_ATTEMPTS = 3` | `tools/molt_dev_land_fast.py` | upstream races; receipt reuse | deep |
| Gate block retry | implicit (first run, solo rerun of one failure) | `tools/molt_dev_gates.py` | memory-pressure stops; proof executor | deep |
| Post-submit verdict | typed (`PostsubmitStatus`) | `tools/molt_dev_queue.py` | runner lock | deep |
| Artifact publication journal | typed (`JournalState`, `JournalEvent`, `JOURNAL_TRANSITIONS`) | `src/molt/artifact_publication.py` | crash recovery | deep (exemplar) |
| Scratch finalization | typed (`ScratchFinalizationPhase`) | `src/molt/temporary_artifacts.py` | retention sweeps | inventory; HF-694 owns the blocked legacy archive |
| Disk reclaim | typed (`ReclaimPhase`) | `tools/disk_guard.py` | guarded sweeps | inventory |
| Guard infrastructure | typed (`GuardInfrastructurePhase`) | `tools/memory_guard_core/process_custody.py` | process custody | inventory |
| Checkout namespace | typed (`NamespaceState`) | `src/molt/checkout_state.py` | parallel worktrees | inventory |
| Artifact binding | typed (`ArtifactBindingState`) | `src/molt/dx.py` | build reuse | inventory |
| Checkout input | typed `Literal` (`InputState`) | `src/molt/checkout_provisioning.py` | image provisioning | inventory |
| Backend daemon suite custody | typed (`SuiteCustodyState`, `ScratchOutcome`, `SealFailure`) since 2026-11-07 | `src/molt/backend_daemon_suite_custody.py` | daemon clients, drain retries | deep |
| Source extension candidate | untyped (`"prepared"`) | `src/molt/cli/source_extension_candidate_transaction.py` | build transaction | inventory |
| Source package seal | untyped (`"committed"`) | `src/molt/cli/source_package_seal.py` | build transaction | inventory |
| Runtime lifecycles: generator and coroutine frames, async tasks, import locks, runtime initialization | runtime types | `docs/design/foundation/26_real-async-generators.md`, `28_asyncio-frontier-runtime.md`, `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md` | free-threaded design | inventory |

## Farm run (`tools/trunk_loop.py`)

A run executes selected proof cells, or landing gates, on a farm host. `start`
writes `run.json` before any remote work; `collect` reads the worker's
`terminal.json` and retains its evidence.

### States

| State | Meaning |
| --- | --- |
| `PREPARING` | record written; remote stage, checkouts and seeds being prepared |
| `LAUNCHED` | the launcher acknowledged a worker (`launcher_pid`, `worker` probe) |
| `COLLECTED` | terminal receipt digest-checked; evidence extracted locally (terminal) |
| `VANISHED` | the worker is gone and published no terminal receipt (terminal) |
| `ABANDONED` | start failed before any launch was attempted (terminal) |

### Transition table (`RUN_TRANSITIONS`)

| From \ To | LAUNCHED | COLLECTED | VANISHED | ABANDONED |
| --- | --- | --- | --- | --- |
| PREPARING | launch acknowledged | lost acknowledgement, terminal found | - | preparation failed |
| LAUNCHED | - | terminal found | worker gone, no terminal | - |
| COLLECTED, VANISHED, ABANDONED | - | - | - | - |

### Invariants

1. A record moves only through `transition()`, which refuses any edge not in
   the table and any unknown state string.
2. Terminal states never change. `collect` answers a `COLLECTED` run from its
   record and its digest-checked `results.zip`, and never contacts the remote.
3. `ABANDONED` means no launch command ran; the record holds the error.
4. A failure at or after the launch command keeps `PREPARING` and records
   `launch_error`: the worker may exist, and `collect` can still find its
   terminal receipt (`PREPARING -> COLLECTED`).
5. `TerminalStatus` (`success`, `failure`, `infrastructure`) is the one
   vocabulary of `terminal.json` and the remote landing `receipt.json`.

### Findings

- **SM-1A (confirmed, fixed): `COLLECTED -> VANISHED`.** `collect` re-observed
  the remote for every record. For a collected run whose stage was gone, the
  probe found no worker and no terminal, so `worker_vanished` overwrote the
  terminal record. Evidence: a reproduction on the base code (2026-11-06) ended
  with `state: vanished` after a re-collect of a `collected` record. The
  integrator had just retired the stages of 13 collected molt-gate runs by hand,
  so any re-collect of those records would have reached this state.
  Test: `test_collect_answers_a_collected_run_from_its_record_without_the_remote`.
- **SM-1B (confirmed, fixed): `PREPARING` had no failure exit.** Any failure
  between writing the record and the launch acknowledgement left `PREPARING`
  forever, with no reason. Evidence: five such records on the integrator Mac
  (four macmini runs on 2026-11-05, one molt-gate run on 2026-11-06), each with
  no `launcher_pid` and no error. Test:
  `test_start_closes_a_failed_preparation_and_keeps_a_failed_launch_collectable`.
- **SM-1C (confirmed, fixed): untyped status vocabulary.** `terminal.json` and
  `receipt.json` statuses were bare strings compared in ten places across two
  files. They are `TerminalStatus` now; an unknown status fails loud.
- **SM-3 (confirmed, fixed 2026-11-07): terminal runs kept their remote
  allocations.** No run retired its stage or worktrees; on 2026-11-06
  molt-gate held 26 worktrees and 13 stages of collected runs (~7.5 GB), and
  bat00's C: drive filled with Molt data. Retirement is now a typed field of a
  terminal record (`RemoteRetirement`: `RETIRED`, `DEFERRED` while the worker
  runs, `FAILED`), never a new `RunState`. `collect` retires after the
  `COLLECTED` record is durable; `start()` and `trunk_loop retire --host`
  sweep unretired terminal runs. Allocations come only from a record whose host
  matches the inventory; a worktree is removed only when `git worktree list`
  registers that exact path at the run's commit; the stage goes through the
  transport's owner-checked cleanup. Tests:
  `test_retirement_removes_only_the_runs_registered_worktrees_then_its_stage`,
  `test_retirement_defers_while_the_worker_runs_and_refuses_live_runs`,
  `test_retirement_refuses_a_record_whose_host_differs_from_the_inventory`,
  `test_sweep_retires_only_terminal_unretired_runs_of_its_host`. Windows proof
  roots (`<proof_*_root>/v2/<nonce>`) are not yet retired (open).

## Landing request (`tools/molt_dev_queue.py`)

`land` enqueues a request file; the runner tests batches under the runner lock,
bisects failures and records one result per request.

### States

| State | Representation |
| --- | --- |
| `PENDING` | `requests/<id>.json` exists and `results/<id>.json` does not |
| `CLOSED(status)` | `results/<id>.json` exists, with one `LandingStatus` |

`LandingStatus`: `LANDED`, `LANDED_RETIREMENT_FAILED`, `FAILED`, `CONFLICT`,
`WITHDRAWN`.

### Transition table

| From | Event | To |
| --- | --- | --- |
| (none) | `enqueue` | `PENDING` |
| `PENDING` | batch lands | `CLOSED(LANDED)` or `CLOSED(LANDED_RETIREMENT_FAILED)` |
| `PENDING` | candidate failure after bisection | `CLOSED(FAILED)` |
| `PENDING` | merge conflict | `CLOSED(CONFLICT)` |
| `PENDING` | `queue withdraw` (runner lock held) | `CLOSED(WITHDRAWN)` |
| `CLOSED(*)` | any | refused |

### Invariants

1. A request closes exactly once: `record_result` publishes its result with
   `durable_publish_exclusive`, so a second close raises and never replaces a
   result.
2. A result wins over a request file: `pending()` skips a request that has a
   result, so no batch tests a closed request.
3. The runner and `queue withdraw` finish any interrupted close
   (`finish_interrupted_closes`) while they hold the runner lock.

### Findings

- **SM-2 (confirmed, fixed): a request could close twice.** `record_result`
  wrote the result, then removed the request. A runner stopped between the two
  left a request both pending and closed; the next batch tested it again and
  `os.replace` rewrote its result. Evidence: a reproduction on the base code
  (2026-11-06) showed the closed request in `pending()` and its `failed` result
  replaced by `landed`. Test:
  `test_queue_request_closes_exactly_once_across_an_interrupted_close`.

## `land --fast` attempt (`tools/molt_dev_land_fast.py`)

| Step | Next |
| --- | --- |
| fetch, merge main, repair, resolve references | receipt lookup |
| receipt for (tree, base) lists head | publish |
| no receipt | precheck; incomplete -> exit FAIL (nothing published) |
| precheck complete | write receipt, publish |
| publish `LANDED` | exit OK |
| publish lost the race (main moved) | next attempt (at most 3), then exit FAIL |

Invariants: nothing is published without a complete precheck or a receipt for
the exact tree and base; a new main means a new tree, so no receipt is reused
across bases. No reachable invalid transition was found.

## Gate block retry (`tools/molt_dev_gates.py`)

| State | Next |
| --- | --- |
| first run of the heavy block | all pass -> done; one memory-pressure stop or clean-closure failure -> solo rerun; custody fault -> stop |
| solo rerun of that gate (own log directory) | pass -> rest of the block in `heavy-rest`; fail -> candidate failure |

- **SM-4 (confirmed, fixed 2026-11-06 in `ca20a625b2`): the retry reused the
  first run's exclusive log paths** and crashed with `FileExistsError`. Each
  retry block now writes to its own log directory (`heavy-retry`, `heavy-rest`),
  and `GateRuntime.passed_logs` records each passing gate's log. Landings 20 and
  21 show `retry passed: the first failure was load or a flake`.

## Post-submit verdict (`tools/molt_dev_queue.py`)

`PostsubmitStatus`: `GREEN` advances `refs/molt/last-green`; `RED` (a candidate
failure) names suspects and revert commands; `CANCELLED` and `INCONCLUSIVE`
implicate no commit. Matches over the enum are exhaustive. No reachable invalid
transition was found.

## Backend daemon suite custody (`src/molt/backend_daemon_suite_custody.py`)

A suite lease and its adoption index share one lifecycle, parsed into
`SuiteCustodyState` when read; an unknown state refuses the record.

| From | Event | To |
| --- | --- | --- |
| (none) | lease or index created | `ACTIVE` |
| `ACTIVE` | `close()` starts / drain seals the index | `CLOSING` |
| `CLOSING` | retried drain seal | `CLOSING` |
| `CLOSING` | guardian drained / final seal | `CLOSED` |
| `CLOSED` | any write | refused (never reopened) |

Drain results are `ScratchOutcome` (`RECLAIMED`, `ABSENT`, `PRESERVED`,
`BLOCKED`); only `RECLAIMED` and `ABSENT` count as a clean drain.

- **SM-5 (confirmed, fixed 2026-11-07): retry was decided by message text.**
  The drain retried a seal when an error string started with
  `adoption seal: OSError:` or contained `suite adoption transaction lock timed
  out`; rewording either message silently changed the retry policy. Seal
  failures are typed now (`SealFailureKind`: `STORAGE` and `LOCK_TIMEOUT` retry;
  `INVALID` and `RECOVERY` do not); the lock timeout is caught as
  `FileLockTimeout`, whatever its wording. Tests:
  `test_seal_retry_follows_the_typed_failure_kind`,
  `test_seal_classifies_the_lock_timeout_by_type`,
  `test_lease_reader_parses_state_and_refuses_unknown_states`.

## Artifact publication journal (`src/molt/artifact_publication.py`)

The exemplar for the others: typed states and events and one transition map.

| From | COMMIT | ROLLBACK | RECOVER_LEGACY | REFUSE |
| --- | --- | --- | --- | --- |
| OPEN (`prepared`) | COMMITTED | ROLLED_BACK | RECOVERED | REFUSED |
| RECOVERED | COMMITTED | ROLLED_BACK | - | REFUSED |
| COMMITTED, ROLLED_BACK, REFUSED | - | - | - | - |

## Open issues (priority by impact)

| Issue | Findings | Impact | State |
| --- | --- | --- | --- |
| Run lifecycle: typed states, failure exit, immutable terminal states | SM-1A, SM-1B, SM-1C | lost verdicts; stuck records; wrong failure class | fixed (this audit) |
| Landing request closes exactly once | SM-2 | a failed request retested or landed twice; rewritten history | fixed (this audit) |
| Farm allocation retention and orphaned runs | SM-3 | farm disk exhaustion (blocked the LLVM bootstrap on 2026-11-06; filled bat00 C:) | fixed 2026-11-07; Windows proof roots open |
| Untyped residual vocabularies | Fixed: `source_tree_state` is `tools/proof_receipts.py` `SourceTreeState` (CLEAN, DIRTY, UNKNOWN) and the executor's mid-run change is the boolean `source_changed`; the source-extension candidate transaction is `CandidateTransactionState` with `CANDIDATE_TRANSITIONS`, the seal compare-and-swap is `PublicationState` with `PUBLICATION_TRANSITIONS`, and the seal commit record is `SealCommitState`. Each table is checked against an independent statement of its lifecycle | a typo or new value passed unseen; no exhaustive match rejected a stale consumer | fixed |
| Inventory machines not yet audited | see Catalog | unknown | open |
