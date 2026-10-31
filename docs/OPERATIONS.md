# Operations Guide

This document consolidates remote access, logging, progress reporting, and
multi-agent workflow rules.

## Farm proofs at an exact commit

`tools/cross_hosts.toml` owns host addresses, custody layout, bootstrap Python,
detached launchers and admitted proof commands. The Stage 1 runner uses that same
inventory and SSH staging owner as `cross_run.py`. From a clean, committed
controller checkout and its managed Python image:

```sh
python -B tools/trunk_loop.py run --host bat00 --commit 06d37eafd2 \
  --cell portability.cargo-custody.windows --cell portability.queue.windows
```

The command prints a durable `run.json` recovery path before allocating remote
resources. `--no-wait` returns after detached launch; resume collection with
`python -B tools/trunk_loop.py collect --record /absolute/path/to/run.json`.
Collection's `--no-wait` reports unfinished work with exit 3. Exit 0 means all
selected cells succeeded, 1 means collected red/infrastructure evidence, and 2
means the controller refused or could not finish an operation. A launch PID is
not completion proof. Preserve the printed record after an SSH/controller error.

The canonical host checkout donates Git objects; the runner never moves its
branch. Separate nonce-owned worktrees bind the committed controller and tested
source. Pinned host archives are downloaded on the controller into SHA-256/size
cache namespaces, transferred and imported by `molt.tool_releases` with the same
pin checks and failure-atomic publication as online provisioning. Locked Python
environment publication remains owned by `molt.checkout_provisioning` and uv.
The entire selected toolchain closure is admitted before any test starts.
Prerequisite failure returns infrastructure evidence without launching tests.
The inventory selects short, disjoint Cargo-output and metadata fixture roots
for Windows portability tests, matching the explicit roles supplied by hosted
CI. Each run admits its exclusive child directories through the existing queue
output-layout authority and records their physical identities in provisioning
evidence; operator shell exports are unnecessary.

Windows launch uses CIM `Win32_Process.Create`; POSIX launch creates a new
session. A host-wide OS file lock excludes competing trunk workers. The worker
uses the existing detached-execution waiter for proof-driver lifetime and
managed-state leases; an unresolved child retains custody without a terminal
completion record. The worker
runs the proof plan's arguments, deadlines and workers unchanged, and stops host
work on the executor's global custody failure. Independent partition failures
retain evidence and allow the next selected cell. No system settings or existing
host processes are changed.

Evidence is collected under
`<artifact-root>/evidence/trunk-loop/<full-commit>/<host>/<cell>/<run-id>/`.
It includes `cmd.json`, `rc`, `run.log`, the canonical `receipt.json`, provisioning
records and pytest JUnit when finalized. A diagnostic `durations-<pid>.jsonl`
stream retains completed setup/call/teardown timings even if the deadline stops
pytest before it can finalize JUnit. The timing stream is not a failure parser.
Remote staging and worktrees remain indexed by the recovery record for the
integrator's reconciliation/retirement; failed or unknown closure never permits
deletion. This stage does not schedule continuous runs, classify regressions or
write the findings ledger. See [the owning design](design/trunk_loop.md).

## Version Policy
The [versioned Python contract](spec/areas/compat/contracts/verified_subset_contract.md)
owns supported versions and Python 3.15 release obligations. Target-version
differences belong in their owning specifications and tests; host behavior does
not choose guest semantics.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see [docs/spec/STATUS.md](docs/spec/STATUS.md)).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **Dirty-tree hand merge coverage**: before pushing a replay from a dirty source checkout through a clean side worktree, run `python tools/dirty_tree_landing_audit.py --source-root <dirty-checkout> --landed-root <side-worktree> --base-ref <old-main-sha> --head-ref HEAD`. The audit fails if an owned dirty source path is missing from the landed range, while still allowing conflict-resolved content to differ from the dirty checkout. Use repeated `--owned <path-or-glob>` filters when preserving unrelated WIP, and `--json-output <path>` to leave a capsule artifact.
- **Windows Codex stall timing**: when Codex appears stalled around a long Molt proof lane, run `uv run --python 3.12 python tools/agent_coordination.py codex-stall -- <proof-command>`. The diagnostic mirrors child stdout/stderr live but writes only timing metadata to `logs/agents/codex_stall/*.json`: first-output gaps, per-stream idle spans, byte/chunk counts, elapsed time, and return code. It does not read, delete, repair, or rewrite Codex state, and it launches the proof command through `tools/memory_guard.py` by default; pass `--no-memory-guard` only when the direct child is already intentionally guarded or the command is a non-proof probe.
- **Windows Codex crash capsule**: when Desktop reports a crash dialog, write a bounded recovery capsule instead of editing Codex state from the repo lane: `uv run --python 3.12 python tools/agent_coordination.py codex-crash --crash-text "<dialog text>"`. The capsule lands under `logs/agents/codex_crash/*.json`, stores the crash-text hash plus parsed code/signal/warning fields, classifies known signatures such as `3221225786` (`STATUS_CONTROL_C_EXIT`), `codex_core::responses_retry` stream disconnects, plugin `interface.defaultPrompt` warnings, and rollout-state repair symptoms, and counts plugin-manifest pressure without storing raw plugin JSON. It does not read, delete, repair, or rewrite Codex SQLite, plugin caches, rollout summaries, or repo state.
- **Windows process-table sampling budget**: process-sentinel cleanup uses a parent-enforced Windows snapshot helper with `MOLT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC` defaulting to 5 seconds. If sampling exceeds the budget, the helper is terminated and the snapshot fails closed to an empty process table so repo-scoped kill decisions are not made from partial host-control-plane evidence. Hot memory-guard RSS polling keeps the in-process sampler plus the same fail-closed deadline to avoid spawning a Python helper every poll. Set the variable to a positive float to tune the budget; set `0`/`off` only for an intentional local investigation where an unbounded process-table read is acceptable.
- **Windows Job sampler authority**: once a guarded child is assigned to its kill-on-close Job, steady-state RSS enforcement queries only kernel-owned Job members. Per-member handle-backed RSS and creation identity preserve distinct per-process and aggregate-tree limits; image names are refreshed only when membership changes. Full host snapshots are reserved for explicit lineage/repro diagnostics or the fail-closed no-Job fallback. Summary telemetry records sampler source, wall/CPU cost, row counts, maxima, and total observer duty so guard overhead remains measurable.
- **Process census detail cost**: host-wide identity snapshots retain parent, group, birth and status for escape detection and protected ancestry. Darwin and Windows bind separately queried resident size only when accounting or reporting visits a process, through the same read-once mechanism as native argv. Each sample freezes its first RSS value; an exited or replaced instance binds zero. Darwin costs one host-table sysctl plus task-info and birth-validation reads for sized processes; Windows removes memory queries from its snapshot helper and opens one birth-checked handle per sized process. Linux keeps RSS from the existing `/proc/<pid>/stat` identity read; the other POSIX `ps` sampler keeps the RSS column already returned in each row. Neither adds a detail read for RSS.
- **Process-classification reuse**: guards and sentinels share a bounded cache of host-control-plane command classification keyed by the complete command and classification policy. Only lexical work is reused: PID identity, ancestry, ownership, process-group protection, and pre-termination revalidation remain live checks. Cache hits do not authorize process cleanup or relax sampling deadlines.
- **WASM validation and linking**: raw and linked builds require the attested `wasm-tools` release; linked builds also require `wasm-ld`. A missing validator fails before Rust backend or runtime compilation and reports the explicit provisioning command. Its prerequisite is captured once per build; actual validation still verifies the executable. These are compiler-side requirements and add no checks to emitted programs. Use `--require-linked` when a linked artifact is required.
- **Node PATH drift (macOS shells)**: some shells resolve `/usr/local/bin/node` (for example Node 14) before modern Homebrew/fnm nodes. Set `MOLT_NODE_BIN=/opt/homebrew/bin/node` (or another Node >= 18 path) for deterministic wasm lanes.
- **Node wasm instability/noise**: use deterministic Node wasm flags (`--no-warnings --no-wasm-tier-up --no-wasm-dynamic-tiering --wasm-num-compilation-tasks=1`) to avoid warning-noise diffs and post-run Zone OOM incidents on large linked modules. The shared guard never projects its RSS budget onto `RLIMIT_AS` or `RLIMIT_DATA`, so V8's sparse address reservations coexist with the same recursive RSS/tree/global authority used by native runners; no Node-only child-limit bypass is required.

## Differential Suite (Operational Controls)
- **Memory profiling**: native differential build/run RSS metrics are opt-in with `MOLT_DIFF_MEASURE_RSS=1`, using `/usr/bin/time` on Linux/macOS. A positive `MOLT_DIFF_MAX_RSS_KB` threshold also requests profiling when that switch is unset; an explicit profiling-off setting with an active threshold raises a configuration error. The mandatory sampled process-tree memory guard remains active for native and WASM runs; its samples are separate evidence from these time metrics. Measured proof-queue execution remains unqualified until its time image is explicitly captured and admitted; correctness lanes need no time utility.
- **Build profile default**: diff harness defaults to `--build-profile dev` (override with `--build-profile release` or `MOLT_DIFF_BUILD_PROFILE=release` for release validation).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) records jobs, limits, and RSS aggregates.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `MOLT_DIFF_FAILURES` or `--failures-output`).
- **First failure is terminal**: each selected backend attempt retains its first outcome and diagnostics. OOM, dyld, daemon/cache and batch failures do not trigger an implicit rerun, setting mutation, cold target or batch-to-CLI fallback. Operator re-execution is a separate run with its own recorded selection. Keep earlier run evidence; the failure queue and raw `MOLT_DIFF_RESULTS_JSONL` records remain the result authorities.
- **Memory caps**: the direct-child `RLIMIT_RSS` backstop defaults to the same adaptive per-process budget as the RSS guard where the host OS enforces it; fast-start recursive RSS polling and POSIX child `rusage` checks remain authoritative across platforms. The guard never conflates that committed-memory policy with virtual-address limits. Override with `MOLT_DIFF_CHILD_RLIMIT_GB`, or disable with `MOLT_DIFF_CHILD_RLIMIT_GB=0` only for an explicit local investigation.
- **In-harness memory guard**: always active for differential runs; no environment variable disables it, so conformance work cannot accidentally bypass custody. It is layered on top of OS rlimits and delegates subprocess execution plus suite-wide process-group custody to `tools/harness_memory_guard.py`. It enforces per-process, per-test-tree, and global active diff RSS ceilings with `MOLT_DIFF_MAX_PROCESS_RSS_GB`, `MOLT_DIFF_MAX_TOTAL_RSS_GB`, `MOLT_DIFF_MAX_GLOBAL_RSS_GB`, and `MOLT_DIFF_MEMORY_GUARD_POLL_SEC`; unset values use the same shared fallback names as the other harnesses (`MOLT_MAX_PROCESS_RSS_GB`, `MOLT_MAX_TOTAL_RSS_GB`, `MOLT_MAX_GLOBAL_RSS_GB`, `MOLT_MEMORY_GUARD_POLL_SEC`). The inherited OS limit is controlled separately by `MOLT_DIFF_CHILD_RLIMIT_GB` or shared `MOLT_CHILD_RLIMIT_GB` and defaults to the live adaptive per-process budget, clamped by the active tree/global budgets. Default caps are adaptive from live available memory (Linux `MemAvailable`; macOS free/inactive/speculative/purgeable pages, the `vm_stat` rows, read from one `host_statistics64` call without a subprocess) and physical RAM, preserving a small reserve for the OS and other workloads while scaling up on large-memory hosts and keeping the reserve floor small enough for hosted runners. Adaptive caps are refreshed in the shared subprocess guard and the suite-level repo sentinel; the already-observed guarded process-tree RSS is accounted back into the live available-memory budget so Molt can use its allocation without self-tightening while still scaling down when unrelated environment pressure appears. Once a child or grandchild is observed, the shared guard remembers its PID and process group so reparented or new-session descendants remain part of per-tree and global RSS accounting. Guard trips terminate tracked process groups, write `MOLT_DIFF_ROOT/memory_guard/tripped.json`, append bounded event/sample telemetry under `MOLT_DIFF_ROOT/memory_guard/`, and force a nonzero summary with `<memory_guard>` in `failed_files`. The trip marker preserves every victim group and sampled process creation identity. Native, CPython, WASM, LLVM, and Luau use one outcome merger: a completed command receives suite RSS attribution only when its captured root or an identity admitted by its existing child-tree/job tracker matches a recorded victim, and the child did not complete successfully. Malformed or unwritable trip evidence is a typed `rss_trip_evidence` infrastructure failure retained by the suite sentinel; it stops admission and increments `guard_infrastructure_failures`, without adding an OOM. The marker uses the canonical atomic byte publisher; only an absent stage is ignored during cleanup, and any other cleanup error remains visible in sentinel details alongside the original publication error. Artifact retirement preserves the original infrastructure phase while appending diagnostics. Diff auto-parallelism uses a separate adaptive scheduler budget (`MOLT_DIFF_MEM_PER_JOB_GB` override) instead of treating the worst-case process-tree kill ceiling as one job, so large-memory workstations can scale job count while cumulative RSS remains governed by the shared suite sentinel.
- **Memory-guard live stream**: `python3 tools/memory_guard_stream.py --diff-root "$MOLT_DIFF_ROOT"` tails the guard's compact NDJSON stream without running its own process sampler. For zero extra viewer process, set `MOLT_DIFF_MEMORY_GUARD_STREAM=stderr` (or `stdout`, `json-stderr`, `json-stdout`) before launching `tests/molt_diff.py`. Telemetry is bounded by default (`MOLT_DIFF_MEMORY_GUARD_MAX_EVENT_MB=1`, `MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB=2`, one rotated sibling per stream) and sample writes are rate-limited with `MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC` (default 1s). Set `MOLT_DIFF_MEMORY_GUARD_WRITE_SAMPLES=0` to keep only trip/event evidence plus the live stream.
- **Standalone guarded commands**: use `python3 tools/memory_guard.py --max-rss-gb <gb> --max-total-rss-gb <gb> --stream stderr -- <command...>` for arbitrary local commands. The guard wraps the direct child in an OS resource-limit launcher by default (`--child-rlimit-gb`) using the adaptive per-process budget as an `RLIMIT_RSS` backstop where supported. It does not apply `RLIMIT_AS` or `RLIMIT_DATA`; sparse virtual reservations from Node/WASM, sanitizers, linkers, and compilers are not committed-memory pressure. RSS remains bounded by recursive polling. The first sample after launch is both the orphan-cleanup baseline and the first enforced, recorded sample, so `--stream` and `--samples-jsonl` always carry at least one sample; between samples the guard waits on the owned child handle, so it observes exit at once rather than after a poll interval. Recursive polling is stateful for accounting: descendants observed before reparenting or session changes continue to be tracked for per-tree/global RSS. Before RSS aggregation or teardown, the low-level guard excludes the current ancestor chain plus Claude, Codex/app-server/renderer/node-repl, and other protected host/control-plane process groups. Teardown stays deliberately narrower: the guard terminates the private root process group created for the direct child, then terminates exact tracked escaped PIDs by PID, never protected ancestor/control-plane groups or child-reported process groups that might also contain Claude, Codex, or other host processes; the raw process-group terminator re-samples protected PGIDs immediately before SIGTERM as the final actuator-side refusal. RSS trips, timeouts, signal exits, and orphan cleanup emit incident-only repro context with command, cwd, limits, selected safe env knobs, pytest identity, optional file-backed pytest current-node snapshot, guard process IDs, sampled parent lineage, and bounded host-control-plane PGID/process samples. `--stream` emits live samples without creating artifacts; `--samples-jsonl <path>` writes bounded samples and rotates once by default (`--samples-max-mb`, default 2 MB).
- **Guarded Cargo interruption custody**: incremental policy comes from the shared Cargo environment authority. Recovery requires a freshly interrupted Rustc directly parented by the observed Cargo process, exact process births and native command arguments, absence of those owners, and both native Cargo target/profile locks. Standalone Rustc/rustdoc and unproven wrapper ancestry do not authorize recovery. The observed `-C incremental` path identifies a profile cache root, not an individual compilation unit. Unknown ownership, active locks, redirected paths or incomplete closure defer recovery without moving caches. Completed compiler caches are retained when a later program or build script fails. Summary JSON records observations and planned/moved paths; provisional and final receipts preserve partial outcomes. The guard retains earlier quarantine evidence and does not prune it automatically. Explicit artifact cleanup is a separate deliberate action after needed receipts have been archived. Ordinary compiler errors do not trigger quarantine, and the guard never deletes the whole target directory as a failure side effect.
- **Native compiler scratch**: one-shot native batches allocate under the active guard's scratch lease, so guard closure also covers abrupt exit. Suite daemons select a separate identity-bound scratch directory through `TMPDIR`/`TMP`/`TEMP` at spawn; per-request environments do not transport the requester's scratch root. The suite EOF guardian reclaims that directory only after verified daemon drain. Failed closure or lost allocation identity preserves it with a blocked cleanup result. `MOLT_DEBUG_ARTIFACT_DIR` remains the explicit replay-artifact destination.
- **Shared harness guard**: benchmark, profiling, `molt build` Cargo backend/runtime builds, extension C compile/link, post-link optimizers, `molt bench`, `molt profile`, `molt test`, `molt validate`, `molt harness` quality layers, direct CLI run/compare/parity child execution, conformance, regrtest, compliance, CLI tests, native compile/run tests, dev-test runner subprocesses, artifact cleanup helpers, surface-probe tests, property, mutation, runtime-compat, Rust-backend, formal-methods, compile-progress, codegen-quality analysis, wasm-link external tools, wasm test helpers, cross-platform smoke, and continuous-test harnesses use the default-on shared guard in `tools/harness_memory_guard.py`. Direct pytest entrypoints are also guarded before collection by root `sitecustomize.py`, the packaged `molt.pytest_memory_guard_bootstrap` entry point, and the repo-configured `molt.pytest_memory_guard_config_plugin` fallback; one custody check over the effective arguments (repo addopts, `-o` overrides, `PYTEST_ADDOPTS`, argv) refuses `@` argument files, an `addopts` override, another pytest configuration file, `--noconftest`, unsafe `--confcutdir`, a `--basetemp` in the checkout, and disabling or not loading the custody plugins, before tests run. Direct `tests/**.py` scripts and `python -m tests.*` module launches re-exec through the same guard via path-local `tests/*/sitecustomize.py` routers plus project `src/sitecustomize.py` for `uv run`/editable project interpreters, without adding harness files inside differential corpus directories. Re-execed pytest and parent-launched test commands install canonical `MOLT_PYTEST_CURRENT_TEST_FILE` custody under `tmp/pytest-memory-guard/`; serial pytest writes the bounded active-node JSON there, while xdist workers write bounded per-worker sidecars under the aggregate file's `.d/` sibling so workers cannot overwrite each other. Parent-side guard repro payloads reject noncanonical current-test paths, include all bounded worker records, and mark the record whose pid lineage matches the violating process when live samples can prove it. `tests/conftest.py` owns the final live-ancestor assertion plus env/plugin validation, path/session/sentinel setup after custody is already established. `tools/check_subprocess_guard_coverage.py` is part of the lint and smoke-validation surfaces and fails if new raw subprocess calls appear outside documented guard internals, bounded metadata probes, or interactive Popen paths with explicit process custody. The shared limits are `*_MAX_PROCESS_RSS_GB`, `*_MAX_TOTAL_RSS_GB`, `*_MAX_GLOBAL_RSS_GB`, `*_CHILD_RLIMIT_GB`, and `*_MEMORY_GUARD_POLL_SEC`; unset family-specific values fall back to `MOLT_MAX_PROCESS_RSS_GB`, `MOLT_MAX_TOTAL_RSS_GB`, `MOLT_MAX_GLOBAL_RSS_GB`, `MOLT_CHILD_RLIMIT_GB`, and `MOLT_MEMORY_GUARD_POLL_SEC` with adaptive live-memory defaults, a 100 ms steady poll, a 20 ms fast-start poll window for allocator spikes, and POSIX post-exit `rusage` classification for short-lived allocation spikes. That `process_rusage` value is lifetime high-water RSS, preserved across exec on Linux; it can include the inherited pre-exec image and must not be reported as the new executable’s allocation or sampled tree peak. Small-cap protocol tests use a fresh guarded observer without weakening this enforcement; [the capacity contract](design/host_capacity.md#executable-image-and-lifetime-memory) owns attribution and its remaining acceptance obligations. Guarded environments also install canonical repo-local artifact/cache roots and set `MOLT_SESSION_ID=guard-<pid>` when the caller has not supplied one, preserving caller-provided session ids for multi-agent isolation. Adaptive RSS defaults refresh during the run, so large-memory workstations scale up automatically and active suites scale down under competing memory pressure while preserving the OS/workload reserve; implausibly large explicit RSS overrides are still capped by that live adaptive budget instead of bypassing the reserve. In Codex-interactive shells (`CODEX_SHELL=1` or a Codex origin marker), unset dynamic shared-guard defaults are capped at 18 GB per process, 24 GB per process tree, and 36 GB global so backend proof work trips Molt-owned groups before starving the Codex control plane; explicit RSS overrides remain operator authority and are still subject to the shared hard/live clamps. Test-family adapters and `tools/guarded_exec.py` also honor `*_TIMEOUT_SEC` or `MOLT_TEST_PROCESS_TIMEOUT_SEC` before their bounded defaults; streamed guarded commands emit keepalive lines every `*_KEEPALIVE_SEC` or `MOLT_SUBPROCESS_KEEPALIVE_SEC` seconds by default so long silent CI work stays observable; set the timeout env to `0` only for an intentionally unbounded local investigation. Harness-launched process trees default the direct-child `RLIMIT_RSS` backstop to the live per-process RSS budget, capped by the tree/global budgets, alongside recursive process-tree RSS polling. Explicit child-rlimit settings adjust that RSS backstop up to the hard child-rlimit cap; they never constrain sparse virtual address space through `RLIMIT_AS` or `RLIMIT_DATA`. The shared low-level guard uses the same persistent lineage tracker as differential runs, so observed recursive descendants stay covered across ppid, process group, and session changes for accounting, while protected ancestor plus Claude/Codex/app-server/renderer/node-repl control-plane groups are excluded before RSS aggregation and teardown; teardown remains scoped to the guarded root process group plus exact tracked escaped PIDs. Automatic repo sentinels scope violation/drain kill sets to the guarded current process tree, while `tools/process_sentinel.py` cleanup remains a repo-scoped operator action. Sentinels reuse the low-level guard's host-control-plane token authority, exclude ancestor plus Claude/Codex app/control-plane groups before kill decisions, and record skipped protected groups once per sentinel as `repo_process_guard_protected_host_group` with pids, command, guard start, observation time, and action; a current-tree sentinel records only the groups of its own tree, the only groups it could have signalled. RSS trips, timeouts, signal exits, orphan cleanup, and termination-settle failures append operator-facing diagnostics with incident time, elapsed duration, cleanup scope, reason, bounded repro context, and the next corrective action; repo-sentinel trip and drain JSONL events also carry `process_samples`, `external_parent_pids`, resolved limits, kill scope, killer/victim attribution, SIGTERM/SIGKILL metadata, and the same bounded repro context. An operator signal (SIGINT, SIGTERM, SIGHUP) to a POSIX guard gives the guarded tree SIGTERM and up to `OPERATOR_SIGNAL_GRACE_S` (10 s) to write its own receipts before SIGKILL; resource trips, timeouts and an owner's cancellation kill at once. The post-timeout/process-trip settle wait is bounded by `MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC` so CI reports the stuck process state instead of waiting forever after SIGTERM/SIGKILL. A guarded subprocess takes no process census before launch: it owns nothing yet, so it could signal nothing. Stale orphaned Molt groups from earlier sessions are an operator action (`molt clean --kill-processes`, or `tools/process_sentinel.py --stale-orphan-sec/--stale-pytest-sec`). Every shared guarded subprocess also starts a short-lived current-tree sentinel when no suite-level sentinel is already active, so benchmark/conformance/regrtest-adjacent helper tools inherit cumulative Molt-process RSS protection by default; that sentinel does not drain, so it takes no baseline census, and it scans only after its first poll interval. After the child exits, one process-table observation serves tracked-orphan cleanup, repo-scoped orphan cleanup and the scratch-closure proof until the guard signals a process; any signal takes a fresh observation. Standalone guarded subprocesses clean their own tracked descendants at parent exit; when a suite-level sentinel is active, per-command orphan cleanup is deferred to that sentinel so intentional warm daemons remain available during the run and are drained at scope exit. Long-running suites should use `guarded_harness_scope` or `repo_process_sentinel` so one sentinel spans the whole run. The sentinel snapshots pre-existing groups on entry and drains only newly spawned Molt groups on exit, so warm daemons remain available during the run but cannot accumulate after successful benchmarks, conformance, regrtest, or pytest sessions. Benchmark timing reads the guard's child pre-`exec` elapsed time, so Python guard startup is not charged to Molt/CPython/WASM runtime samples.
- **Current-tree guard ownership**: repo-sentinel current-tree ownership survives reparenting only while live parent lineage or repo/Molt command identity still proves the process group belongs to the guarded launch. Numeric process-group reuse alone is not ownership proof; a later host process that inherits an old PGID must be excluded from drain and violation kill sets, and stale-preflight cleanup must ignore orphaned host processes that lack Molt/repo command identity. The standalone `tools/process_sentinel.py` terminator also re-samples ancestor plus Claude/Codex/app-server/renderer/node-repl protected PGIDs before signaling, so direct cleanup commands share the same final kill refusal.
- **Process cleanup custody**: cleanup authority is Molt-owned process identity, not repo path, process name, stale PID, parent shell, or Codex/Claude ancestry. Only live-proved Molt build/test/bench workers, backend daemons, runtime children, and guard-owned process groups may be signaled. Codex, Claude, app-server, renderer, node-repl, MCP/plugin helpers, shell hosts, Git pollers, ancestors, and other host-control-plane processes are never cleanup targets; ambiguous ownership must skip and preserve evidence.
- **Descendant birth admission**: tracker-free watches and sentinel ownership use the same live birth-fenced descendant authority as tracked custody. Every inferred edge requires exact positive integer creation timestamps with parent birth no later than child birth; unknown births and stale parent PIDs cannot extend ownership. Directly admitted roots retain their existing meaning. The current-guard exception to host protection requires that same verified ancestry, while uncertain host ancestry and snapshot-helper descendants remain conservatively protected. PPID-only descendant walks serve diagnostics and never grant cleanup custody.
- **Guard-marker custody**: `tools/memory_guard_core/active_custody.py` owns the
  schema-v2 guard/direct-child birth identities, publication, reconciliation and
  retirement for the producer, disk guard, pytest bootstrap, proof-queue
  diagnostics and final runtime-WASM preflight. `active/` holds unresolved
  custody, including interrupted historical runs; inspection costs O(active
  entries), even when their processes have exited. Observation readers stream
  one full payload at a time and retain only compact blocker diagnostics;
  boolean protection checks stop at the first blocker. Diagnostics are sorted
  after projection. Process-specific lookup filters names before sorting its
  matches. Enumeration and read failures remain protective. Reconciliation
  deliberately captures all generations before its native process snapshot,
  retaining their full payloads until classification and publication. These
  development checks add no work to emitted programs. A guard that
  finishes moves its own marker to `retired/` once its scratch is reclaimed,
  retained or blocked; `retired/` keeps the newest 256 records. A marker that
  stays in `active/` names a live run, a run that raised, a run whose closure or
  scratch is unresolved, or a guard that died. The producer publishes
  `spawn_pending` before it creates a child, records the handle-bound child
  birth, and records `child_launch_state: failed` when the launch raises (no
  child exists). An exiting guard sweeps `active/` when it holds 16 more records
  than the previous sweep left (`sweep.json`); the gate reads at most 1,025
  directory entries and never sweeps above 1,024 records, so it never pays for
  history. `python tools/memory_guard_custody.py` does the same full pass on
  demand: dry run by default, `--apply` to write. One native snapshot decides
  each record. Native Linux tables can contain processes with a zero boot-tick
  birth. Keep those rows and their observed zero; they must not abort the
  complete census. Zero cannot establish the positive birth comparison required
  by the marker protocol, so a same-pid observation remains protective. An
  operator-only release preserves that zero in its attested receipt. Marker
  identities still require positive births. A pid absent from the snapshot is
  dead, whatever its recorded birth. A pid that now names another birth was
  reused, and a reused child leader proves its recorded group closed (a kernel never gives a new process a
  pid that still names a live group). A record whose guard and child are dead
  or reused, with no recorded child-group member left, becomes
  `custody_reconciled` and leaves `active/`. Its leased or indeterminate scratch
  then takes the scratch authority's failure path (see `docs/agent/PROOF_QUEUE.md`).
  `completed` and `finalizer_completed` are terminal without births, because the
  guard reaped its child before it published them, but only when the guard
  proved descendant closure. The guard publishes that verdict as
  `descendants_closed`; a record written before the field existed proves it only
  when `orphaned_process_groups` is empty. An unclosed completion stays
  unresolved custody, so it protects disk reclamation, until one snapshot shows
  its child's group and every recorded orphaned group empty; its receipt lists
  those groups in `empty_process_groups`. Live evidence (a matching birth, or a
  live member of the child's or an orphaned group) always protects, except that a recorded orphaned
  group is closed when the process that now holds its pid is younger than the time since the record's
  last update less 300 s: a kernel never reuses a pid that still names a live group. Inconclusive evidence (a present
  pid without two comparable births, or a launch that never published its
  outcome) and unreadable records stay protective, and the report names each
  one with the exact command `memory_guard_custody.py --active-dir <dir>
  --release <marker> --apply`. Use it only after you confirm that no process of
  that run is alive; it records an `operator_attested` receipt and still refuses
  live evidence. `tools/disk_guard.py` reads files only; when an active record
  blocks a candidate, its result names the records and this command. Marker
  generations are read before the snapshot and compared under the producer's
  lock before any write. Empty, failed or substituted snapshots abort without
  writes. A parent's watched PIDs and file timestamps cannot retire another
  marker. Reconciliation never signals processes or grants process-cleanup
  authority. Host-control-plane exclusions remain unchanged. A host whose
  `active/` still holds history from before retirement existed migrates it once
  with `python tools/memory_guard_custody.py --apply`.
- **Guarded-command hotspot profile**: shared guarded subprocesses write structured `guarded_command_profile` JSONL events only for incidents by default, keeping routine successful test/build runs clean. Incident events include prefix, session id, command, cwd, return code, elapsed time, memory-guard limits, peak RSS when sampled, violation/timeout/signal/orphan status, GitHub Actions context when present, and a bounded repro payload with pytest/Codex/Molt env hints, parent-process lineage, and host-control-plane process topology. Set `MOLT_GUARD_PROFILE=all` or provide `MOLT_GUARD_PROFILE_LOG=<path>` for an intentional all-command capture; set `MOLT_GUARD_PROFILE=0` to disable profile writes entirely. The log rotates to `commands.jsonl.1` at 16 MB by default (`MOLT_GUARD_PROFILE_MAX_MB`, set `<=0` only for an intentional unbounded local capture). `tools/guarded_exec.py` also prints the elapsed time and profile path for each wrapped CI/dev command. Use `MOLT_GUARD_PROFILE_LOG=<path>` to move this log to another canonical artifact path for a specific run, and use `python3 tools/profile_hotspots.py --limit 20` to summarize slowest events and grouped command families when all-command profiling is enabled.
  Profile events also retain nullable `child_elapsed_s`, `cleanup_elapsed_s`, and `sampling_telemetry` from the guard. Cleanup is the guard-minus-child residual, not a separate scratch-deletion timer; scratch finalization has its own phase fields. Sampling overlaps the child lifetime and includes tracker/incremental observations, so it is not additive elapsed time or pure native enumeration. CPU measurements use process time and can include other threads. Missing measurements remain `null`; measured zero remains zero. These fields are diagnostics, not custody identities.
- **Repo-sentinel incident schema**: shared harness repo-sentinel JSONL events distinguish termination from observation. Claimed kills carry `claim_status=claimed`, `termination.attempted=true`, `killed_at`, and `killer_*`; PGIDs already claimed by another guard carry `claim_status=already_claimed`, `termination.attempted=false`, `observed_at`, and `observer_*` so diagnostics do not falsely attribute a kill to the wrong guard.
- **Output startup and binary-size audit**: use `python3 tools/output_startup_size_audit.py --targets all --build-profiles dev,release --backends all --samples 5` when startup or output-size regressions are suspected. The audit builds the same hello-world probe through the normal CLI for native, linked-WASM, Luau, and MLIR targets, records artifact bytes for each target/profile/backend row, measures same-path and fresh-path startup wherever a canonical runner exists, and writes a JSON artifact to `bench/results/output_startup_size_audit_<timestamp>.json`. Native fresh-path copies expose dyld/code-signature fixed costs; linked-WASM rows run under deterministic Node flags when Node is available; Luau/MLIR rows keep size custody and record an explicit skipped-runner reason when local startup is not canonical. Pass `--max-artifact-mb` and `--max-fresh-start-ms` only when intentionally enforcing a budget; otherwise the audit is observational so current regressions can be measured before setting the ratchet.
- **Adaptive resource pressure plan**: `tools/resource_pressure.py` is the shared policy layer above the memory guard. CI (`tools/ci_resource_env.py`), compile-slot custody (`tools/compile_governor.py`), and differential auto-parallelism (`tests/molt_diff.py`) all consume the same `molt.resource_pressure.v2` JSON-shaped plan so selected Cargo jobs, compile slots, active-process caps, diff jobs, memory source, reserve, pressure level, and rationale are explainable from one contract. CI Cargo concurrency is calibrated from the exact measured process-tree peak, receipt identity, and explicit safety headroom in `config/ci_resource_policy.toml`; an explicit environment override remains visible as a distinct authority rather than masquerading as calibrated evidence. Existing family-specific env overrides still win, but unset defaults are derived from live adaptive memory and CPU capacity instead of fixed host assumptions. Use `python3 tools/ci_resource_env.py --dry-run --json` to print the current hosted-runner/dev-runner policy without mutating `$GITHUB_ENV`.
- **Runtime resource tracker**: manifest/env limits (`MOLT_RESOURCE_MAX_MEMORY`, `MOLT_RESOURCE_MAX_DURATION_MS`, `MOLT_RESOURCE_MAX_ALLOCATIONS`, `MOLT_RESOURCE_MAX_RECURSION_DEPTH`) install a runtime-wide tracker for the current thread and future worker threads. Core object allocations, object-owned Vec backing stores (lists/tuples/dicts/sets/bytearrays, builders, dataclass fields, memoryview shape/stride metadata, map/zip iterator vectors), scope arenas, JSON parse temp arenas, and WASM scratch buffers reserve budget before touching the allocator and roll reservations back on allocator failure. Vec growth routes through tracked reserve hooks; teardown uses best-effort tracker release paths so Rust TLS destructor order cannot abort process cleanup. WASM split-runtime hosts must provide real `molt_resource_on_allocate` and `molt_resource_on_free` exports; missing resource hooks fail export validation instead of installing no-op host fallbacks.
- **Runtime child resource envelope**: runtime process-spawn intrinsics propagate the active `MOLT_RESOURCE_MAX_*` limits into children and clamp explicit child env values so nested Molt children may tighten but not widen the parent envelope. On Unix, runtime child spawns also install a pre-`exec` memory rlimit from the tighter of `MOLT_RESOURCE_MAX_MEMORY` or `MOLT_CHILD_RLIMIT_GB` unless the shared child rlimit is explicitly set to `0` for an intentional unbounded investigation. Runtime byte streams enforce queued-byte backpressure (`MOLT_STREAM_MAX_QUEUED_BYTES`, or `MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES` for process pipes) so subprocess stdout/stderr readers block instead of accumulating unbounded pipe buffers in the parent.
- **Out-of-band process sentinel**: use `python3 tools/process_sentinel.py --once --kill-all` to stop live-proved repo-scoped Molt build/test/bench process groups from prior sessions, or `python3 tools/process_sentinel.py --kill-all --until-clean-sec 30 --max-runtime-sec 120` to drain delayed stale Molt launches until the repo has stayed quiet. The clean-window mode performs a final scan before success and processes any newly observed delayed Molt launch instead of returning on stale quiet-state. For conservative hygiene, use `python3 tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`, which kills only matched Molt-owned groups whose external parent is init/launchd and whose `ps etimes` age exceeds the selected threshold. It streams to stderr/stdout only, writes no artifacts by default, enforces per-process/per-group/global ceilings, reports action, reason, kill scope, killer/victim attribution, SIGTERM/SIGKILL metadata, `killed_at`/`observed_at`, elapsed sentinel time, process age when available, pids, external parents, sampled process rows, bounded repro context, and next action, and uses the same adaptive live-memory budget unless overridden with `--max-global-rss-gb`. Use `--json` when a parent process needs NDJSON incident payloads instead of human text.
- **Cache retention**: after a source-development proof payload closes, its existing guarded controller applies the shared `MOLT_CACHE_MAX_GB` (30 GiB) and `MOLT_CACHE_MAX_AGE_DAYS` (30 days) budgets in process, before closing source custody. It creates no maintenance queue row, helper process or additional supervisor build. The canonical prune lock is nonblocking and throttles attempts to one per five minutes per cache root, including failed or interrupted sweeps. Cleanup shares the enclosing guard's remaining deadline and memory budget; ordinary prune errors are diagnostic and preserve the payload return code, while timeout, memory and custody failures remain infrastructure failures. Set `MOLT_CACHE_PRUNE=0` to disable automatic retention. Successful source CLI builds dispatch a due daily cache sweep through the existing retained maintenance supervisor; its `logs/cache_retention/automatic/<epoch>/run.log` and `rc` under the selected cache own completion and errors. The command performs no cache inventory. Installed CLI builds apply their packaged daily policy directly. Ordinary installed CLI builds do not load queue machinery; `python -m molt.cache_eviction` and `tools/molt_cache_prune.py` provide the same explicit policy without automatic throttling. Shared native archives and constituent objects are independent immutable entries protected by persistent producer/reader/evictor lock stripes; frontend analysis/lowering JSON leaves are evicted individually. Successful admitted reuse refreshes the existing eviction owner at most once per five minutes: entry-directory timestamps for shared archives, constituent objects and WASM bundles, or the optional flat artifact/JSON timestamp. Required build outputs and immutable entry contents are not touched. Entry readers use their existing publication stripe; lockless optional readers attempt the existing prune lock without blocking after copying and validating their value. This is approximate recency, not a lifetime pin: a busy sweep or a failed optional timestamp update may omit a refresh. Eviction rechecks the inventoried identity and timestamp under its existing lock; refreshed or replaced entries count as busy and survive that sweep. Active entries, unknown mutable namespaces, publication stages and durable control/home/bin state remain protected. The `repository-scan-records` namespace belongs to [`tools/scan_records.py`](../tools/scan_records.py), which bounds it itself: repository gates replay unchanged per-file results from it, records unused for seven days expire, and each scanner keeps its four newest code generations. Legacy flat native archives, compilation scratch and two-hex object shards are retained during mixed-version operation and reported separately: they require separately verified terminal custody, not age-based deletion. A budget applies to positively classified evictable logical entry bytes and can remain exceeded while owners are active; protected namespaces are not a hard whole-root quota. Receipts report actual inventory and sweep elapsed time; dry runs neither create nor modify cache state.

  Recency updates pin the admitted file or directory generation, reject links and
  reparse points, and change only its modification timestamp. POSIX uses a
  no-follow descriptor and `os.utime`; Windows uses an attribute-only no-follow
  handle and [SetFileTime](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfiletime).
  [CPython 3.12](https://github.com/python/cpython/blob/3.12/Modules/posixmodule.c)
  exposes neither descriptor nor no-follow `utime` on Windows. Windows generation
  comparison uses native birth time: CPython pathname stat and handle stat can
  expose different clocks as legacy `ctime`. POSIX retains its change-time check.
  This is platform selection before I/O, with no failed-call retry or
  link-following fallback.
  A cold refresh adds one open, descriptor identity check and close; the existing
  five-minute throttle keeps hot hits free of those operations.
- **Ignored artifact cleanup**: use `molt clean`, `python3 tools/artifact_cleanup.py`, or `python3 tools/dev.py clean-artifacts` for a dry-run of canonical ignored build-output cleanup and cache pruning, including friend-suite checkout caches under `bench/friends/repos/`. Add `--apply` to delete and `--kill-processes` to run the process sentinel before deletion; that sentinel pass may drain only live-proved Molt-owned workers and is never Codex, Claude, app-server, renderer, node-repl, shell, Git, MCP/plugin, or host-control-plane cleanup. In JSON mode, cleanup runs the sentinel with `--json` and embeds parsed `sentinel_events` in the cleanup report; raw sentinel stdout/stderr are included only with `--verbose`. All three entrypoints route through `tools/artifact_cleanup.py`, and both the optional sentinel pass and the `git clean -X` pass run through the default-on `MOLT_DEV_CLEANUP` memory guard. Evidence and unknown checkout caches are preserved: `logs/`, `tmp/`, benchmark results, harness reports, and old provider caches cannot be removed by generic cleanup or an extra path. The cleanup pathspec is allowlisted, so tracked files, dirty partner work, `.venv/`, `.omx/`, `third_party/`, fuzz corpora, and test corpora are not default cleanup targets, and `--extra-path` refuses those paths and their ancestors. Checkout cache roots (`.molt_cache/`, `.molt_cache-*/`, `runtime/molt-backend/.molt_cache/`) hold live locks and, in a plain clone, the default Molt home, so `git clean` never touches them: after `git clean`, cleanup runs `tools/molt_cache_prune.py` on each root with a zero budget, which evicts cache entries and keeps held locks, the Molt home and its bin directory; Cargo incremental quarantine receipts under `target/.molt_state/quarantine/cargo_incremental/` are target artifacts and are intentionally removed by explicit apply-mode cleanup.
- **Wrapper policy**: diff runs disable `RUSTC_WRAPPER`/`sccache` by default for portability. Opt in with `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` on hosts where wrapper caches are known-good.
- **Pass-log pruning**: when `--log-dir` is enabled, per-test logs for passing tests are pruned by default to reduce clutter. Set `MOLT_DIFF_LOG_PASSES=1` to keep pass logs.
- **Backend daemon policy**: diff runs use `MOLT_DIFF_BACKEND_DAEMON` when set; otherwise they use the compiler's `MOLT_BACKEND_DAEMON` policy: enabled by default on POSIX, including macOS, and unavailable on Windows.
- **Backend daemon identity custody**: backend daemon sidecars under the resolved build-state root's `backend_daemon/` directory (see [Build Throughput](#build-throughput-multi-agent)) are JSON identity records (`*.identity.json`) with pid, socket path, project root, cargo profile, config digest, backend binary, and command snapshot. `src/molt/backend_daemon_custody.py` is the shared authority for parsing these sidecars, matching process-table commands, accepting socket-health probes, and issuing verified termination/escalation. A PID or explicit socket env alone never authorizes `SIGTERM`/`SIGKILL`: CLI restart/stale-cleanup paths, `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`, and `tools/bench_individual.py --isolate-daemon` verify the recorded identity first, then revalidate before escalation. Native/WASM benchmark pruning canonicalizes `MOLT_SESSION_ID` before cleanup and terminates only current-session identity records, so concurrent warm daemon/cache state survives. `tools/compile_progress.py` only cleans marker-scoped compiler children and never sweeps backend daemons. Legacy raw `*.pid` files are cleanup debris only and are removed without signaling. An identity marked `retiring` belongs to a daemon draining after a `retire` request: no pruner signals it, and the orphan sweep leaves its socket path to the successor daemon. `tools/verify_native_binary_valid.sh` builds daemon-off and no longer performs blanket daemon `pkill`.
- **dyld preflight**: `MOLT_DIFF_DYLD_PREFLIGHT=1|0` selects the native binary preflight (default on macOS). A detected defect fails the selected attempt without changing daemon, target, state or cache selection.
- **Explicit cache controls**: `MOLT_DIFF_FORCE_NO_CACHE=1|0` selects `--no-cache`; caching stays enabled by default. `MOLT_DIFF_FORCE_REBUILD=1` explicitly requests rebuilds, default off. Failures preserve these settings and the selected roots.
- **Batch compile server selection**: `MOLT_DIFF_BATCH_COMPILE_SERVER=1` selects persistent batch builds, default off. Startup, transport and build failures are terminal for that selected attempt; the harness does not resubmit, enter cooldown or fall back to the CLI. `MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC` bounds a request within its admitted item budget.
- **Shared target pinning (macOS)**: explicitly set both `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to the same shared path for diff runs so workers do not drift onto ad-hoc/default targets and duplicate rebuilds.
- **Interrupted-run cleanup**: before a new long sweep, clear stale Molt-owned harness workers through the custody-aware sentinel (`tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`). Do not use raw PID, name, process-group, parent-chain, `pkill`, `killall`, `taskkill`, or `Stop-Process` cleanup for diff recovery; ambiguous ownership must skip and leave evidence. Keep one supervising diff run per shared target.
- **Sentinel report-only kill switch**: set `MOLT_PROCESS_SENTINEL_REPORT_ONLY=1` to make `tools/process_sentinel.py` report every verified would-be termination without sending signals. This does not relax host-control-plane or identity checks; it is an operator safety valve for recovery sessions where evidence should be recorded before any Molt-owned process is drained.

Automatic LLVM discovery excludes entries whose filesystem provenance cannot
be established, including inaccessible PATH or sibling directories. It shares
[CPython-style executable search](https://docs.python.org/3.12/library/shutil.html#shutil.which)
through the existing toolchain authority, preserves lexical driver names, and
continues to later admissible candidates. Explicit tool and SDK selections retain
path-specific configuration errors; unknown provenance never qualifies as native.
The existing identity-keyed cache retains both admitted and rejected observations.

## Debug Commands

Use the canonical `molt debug` surface for retained debug artifacts and
machine-readable summaries.

Currently wired commands:

- `molt debug repro <source.py> [--compare]`
- `molt debug ir <source.py> --stage pre-midend|post-midend|all`
- `molt debug verify`
- `molt debug trace <source.py> [--family callargs|call_bind_ic|function_bind_meta|backend_timing|compile_func] [--assert-no-pending-on-success]`
- `molt debug reduce <source.py|manifest.json> --oracle-json <oracle> [--eval-command <cmd>] [--eval-timeout <sec>]`
- `molt debug bisect <source.py|manifest.json> --passes <a,b,c> --oracle-json <oracle> --eval-command <cmd>`
- `molt debug diff <summary.json> [--failure-queue <failures.txt>]`
- `molt debug perf <profile.json|profile.log>...`

Current contract:

- manifests are written under `tmp/debug/` by default;
- retained outputs use `logs/debug/` when `--out` is provided;
- `--format json` emits a stable summary payload suitable for automation;
- unsupported or invalid requests must fail via explicit structured payloads,
  not silent no-ops.

Version and platform rules:

- treat Python version as an explicit dimension (`py312`/`py313`/`py314`) when
  behavior differs;
- treat host/process features as capability-gated, not assumed;
- do not silently inherit POSIX-only behavior for process control, timeouts, or
  profiling support.

Current status note:

- `molt debug trace` now wraps the currently wired call-bind/backend trace families
  (`callargs`, `call_bind_ic`, `function_bind_meta`, `backend_timing`, `compile_func`) and records the effective
  low-level trace env knobs in the manifest;
- `backend_timing` enables the existing backend compile-time timing surface
  (`MOLT_BACKEND_TIMING=1`) through the same canonical trace command;
- `compile_func` enables the per-function backend compile trace surface
  (`MOLT_TRACE_COMPILE_FUNC=1`) through the same canonical trace command;
- `--assert-no-pending-on-success` enables the central
  `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1` trap in the clean runtime call core;
- wider runtime assertion rollout beyond that central call core is still under
  active build-out;
- `molt debug reduce` and `molt debug bisect` now accept canonical oracles plus
  evaluator commands, explicit evaluator timeouts, and manifest-backed
  reduction/bisection evidence;
- contested runtime work, especially around call-bind ownership, must not be
  forced through while partner changes are active.

## Backend cache sync receipts

`molt.cli.artifact_sync` owns the persisted source-to-output receipt. Version 2
binds the contract-namespaced source key and cache tier to output size and SHA-256;
size and modification time alone never authorize cache reuse or daemon output
suppression. Old version-1 receipts miss and regenerate through the normal build.
Native, WASM and textual backend artifacts use this same receipt authority.

Backend admission supplies its `StableRegularFileIdentity` to receipt matching.
Receipt admission checks its path and compares it with one fresh shared streaming
content capture; a detached metadata token alone cannot authorize the old digest.
Artifact-sync receipt and frontend payload reads capture bytes and identity in
one owned read. Warm payload hits revalidate content and reuse only JSON parsing;
restoring size and mtime cannot preserve a substituted payload hit. This costs
one receipt read/hash per lookup and one artifact hash when validating a supplied
artifact identity, while eliminating the cold capture-then-read duplicate.
Publication invalidates the process payload cache rather than binding caller
data to a possibly replaced path. Other metadata-only polling has the limits
described in the toolchain direct-file custody contract.

Other content-cache hits follow the same distinction. Native custody rereads
the archive and extracted members, reusing only archive parsing when its digest
matches. WASM fact and linking caches hash the selected member before reuse;
cached structural admission hashes the pair, and final codegen binding checks
hash the pair and pinned receipt. Mapped symbol parsing hashes its mapped view
without copying the entire file. Observed copies and generation staging hash
their existing copy stream. These are content-admission costs, not a speedup
claim or an implicit change to metadata-only tree polling.

Warm-output reuse and daemon suppression share native module-chunk closure
admission and return the same generation-bound result. A self-consistent receipt
does not override conflicting bytes produced for the same key: publication and
materialization preserve the conflicting inputs and report the mismatch.
Mutable output and shared-stdlib link snapshots own independent copies, never
hard links to writable cache sources. Immutable publication captures its returned
identity after private-link cleanup, which itself changes inode mutation metadata.
Rust archive production validates the actual emitted container and every member's
target before atomic commit, through the same parser used for cache admission.
An upstream writer's automatic COFF-to-GNU/GNU64 promotion is an error, not an
implicit target-format conversion; failed admission preserves the prior output.

The hash count is distinct from bytes read: semantic/format validators and symbol
tools may inspect bytes in addition to the one identity hash. Native archive
shape admission visits bounded headers through the shared archive parser without
hashing or reading opaque payloads; callers requesting semantic archive identity
still hash every content member. Read-byte and hash-call regressions pin that
distinction rather than treating hash-helper invocation counts as I/O profiles.

## Linear Workspace Hygiene
- Refresh the repo-backed local Linear artifacts from current TODO contracts with `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .` to inspect drift.
- Apply the refreshed local seed/manifests/index with `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root . --apply`.
- `refresh-local-artifacts` is codebase-canonical by default: it harvests structured `TODO(...)` contracts from `src/`, `runtime/`, `tools/`, `tests/`, `formal/`, and `demo/`, materializes the full deduped leaf inventory into `ops/linear/seed_backlog.json`, then reduces that inventory into grouped/category manifests under `ops/linear/manifests/*.json` and updates counts in `ops/linear/manifests/index.json`.
- Use `--source-mode all` only for audits that explicitly want stale doc signal mixed in; the default `codebase` mode is the source of truth for prioritization and live sync.
- The grouped manifests are the live-sync source of truth. They preserve strong urgency via worst-leaf Linear priority plus explicit impact/pressure rollups in the issue title/description/metadata, while keeping non-code signal secondary.
- The local artifact contract is now two-layer:
  - `ops/linear/seed_backlog.json`: full normalized leaf inventory for auditability.
  - `ops/linear/manifests/*.json`: grouped/category issues sized for the live Linear workspace.

## On-Call Runbook (Stdlib Intrinsics Gates)
Use this section when `python3 tools/check_stdlib_intrinsics.py` fails in CI or
on-call triage.

For agent triage, add `--json-out <local-report.json>`. The
`molt.stdlib-intrinsics-audit.v4` report contains `ok`, `analysis_complete`, and
one ordered `diagnostics` list matching the console. Policy failures report all
evaluated gates in one scan; source or manifest evaluation failures replace any
stale report with `analysis_complete: false` and no fabricated coverage. Always
check the exit status and these fields before consuming module counts.
`facades` records static forwarding evidence, not runtime conformance. Classifying
a forwarding module as intrinsic support requires all its resolved implementation
owners to have that backing. Mixed or unproved owners and unanchored cycles can
remain `python-compiled`; this classification does not decide runtime admission.
Pure imports can forward a provider's target-dependent `__all__` without executing
exports during classification. Actual import custody is checked separately.
`--update-doc` publishes the generated public audit only when every gate passes.

### Standard First Commands
Run these first to establish current state:
```bash
python3 tools/gen_stdlib_stubs.py --check
python3 tools/check_stdlib_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
cat tools/stdlib_intrinsics_ratchet.json
python3 tools/check_stdlib_intrinsics.py --update-doc
```

### WASM Runner Quick Triage
Use these when wasm parity lanes fail before module semantics are reached:
```bash
which -a node
node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -e "const m=require('node:wasi'); console.log(typeof m.WASI)"
which wasmtime
wasmtime --version
```

### WASM Runtime Artifact Failure Strings
Use these when `molt build --target wasm` fails with runtime-artifact validation errors.

`Runtime wasm build produced invalid artifact`
```bash
# Preserve and validate the exact failed artifact; the producer fails closed and
# never retries under a different Cargo profile.
RUNTIME="$CARGO_TARGET_DIR/wasm32-wasip1/wasm-release/molt_runtime.wasm"
ls -l "$RUNTIME"
xxd -l 16 "$RUNTIME"
wasm-tools validate "$RUNTIME"

# Confirm the canonical profile contract before investigating compiler/toolchain
# drift. `wasm-release` is the only shipping WASM codegen authority.
uv run --python 3.12 pytest -q \
  tests/cli/test_backend_manifest_contract.py::test_shipping_profiles_use_measured_target_codegen_policy \
  tests/cli/test_backend_manifest_contract.py::test_runtime_wasm_shipping_has_no_fallback_compiler_authority \
  tests/cli/test_cli_wasm_artifact_validation.py::test_ensure_runtime_wasm_fails_closed_on_invalid_cargo_artifact \
  tests/cli/test_runtime_wasm_single_compile.py::test_app_path_freestanding_also_requires_atomic_pair
```

### Failure String -> Copy-Paste Response
`stdlib intrinsics lint failed: stdlib top-level coverage gate violated`
```bash
python3 tools/gen_stdlib_stubs.py --write
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed: top-level module/package duplicate mapping`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py" and len(rel.parts) == 2:
        pkgs[rel.parts[0]] = str(rel)
    elif len(rel.parts) == 1 and p.suffix == ".py":
        mods[p.stem] = str(rel)
for name in sorted(set(mods) & set(pkgs)):
    print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib package kind gate violated`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required_packages = set(base["STDLIB_PACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required_packages):
    mod = root / f"{name}.py"
    pkg = root / name / "__init__.py"
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: stdlib submodule coverage gate violated`
```bash
python3 tools/gen_stdlib_stubs.py --write
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed: submodule/package duplicate mapping`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py":
        if len(rel.parts) > 1:
            pkgs[".".join(rel.parts[:-1])] = str(rel)
    else:
        mods[".".join((*rel.parts[:-1], p.stem))] = str(rel)
for name in sorted((set(mods) & set(pkgs))):
    if "." in name:
        print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib subpackage kind gate violated`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required = set(base["STDLIB_PY_SUBPACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required):
    parts = name.split(".")
    mod = root.joinpath(*parts[:-1], f"{parts[-1]}.py")
    pkg = root.joinpath(*parts, "__init__.py")
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: unknown intrinsic names`
```bash
python3 tools/gen_intrinsics.py --write
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed: unknown strict-import modules requested`
```bash
python3 tools/check_stdlib_intrinsics.py --allowlist-modules builtins,sys,types,importlib,importlib.machinery,importlib.util
```

`stdlib intrinsics lint failed: strict-import roots must be implemented`
```bash
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
rg -n "TODO\\(stdlib[^,]*,.*status:(missing|partial|planned|divergent)" src/molt/stdlib
```

`stdlib intrinsics lint failed: bootstrap strict roots are incomplete`
```bash
ls src/molt/stdlib/builtins.py src/molt/stdlib/sys.py src/molt/stdlib/types.py
ls src/molt/stdlib/importlib/__init__.py src/molt/stdlib/importlib/machinery.py src/molt/stdlib/importlib/util.py
```

`stdlib intrinsics lint failed: bootstrap modules must be implemented`
```bash
python3 tools/check_stdlib_intrinsics.py
```

The source audit inventories candidate imports; it does not require optional
imports to succeed or classify exception handlers as forbidden. Required symbols
and module custody are checked by compilation/link admission. Use API differential
tests for exception propagation and target behavior; see
[the source authority](internals/source_scan_authority.md#what-source-inventory-establishes).

`stdlib intrinsics lint failed: required intrinsics nothing reads (delete each requirement; an intrinsic must be read to count)`
```bash
python3 tools/check_stdlib_intrinsics.py --json-out /tmp/audit.json
# unread_intrinsic_bindings / discarded_intrinsic_requirements name each site.
# Delete the requirement; never call an intrinsic only to make a module count.
```

`stdlib intrinsics lint failed: intrinsic-partial ratchet gate violated` (or `stub ratchet gate violated`)
```bash
python3 tools/check_stdlib_intrinsics.py
cat tools/stdlib_intrinsics_ratchet.json
# Lower modules first, then tighten max_intrinsic_partial or max_stub.
```

`stdlib intrinsics lint failed: full-coverage attestation references unknown modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
print("covered_count", len(covered))
for name in sorted(covered):
    print(name)
PY
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract has non-attested modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("contract_without_attestation", sorted(contract - covered))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract missing modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("attested_without_contract", sorted(covered - contract))
PY
```

`stdlib intrinsics lint failed: full-coverage modules must remain intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
statuses = {entry["module"]: entry["status"] for entry in payload["modules"]}
for name in payload.get("fully_covered_modules", []):
    if statuses.get(name) != "intrinsic-backed":
        print(name, statuses.get(name))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract references unknown intrinsics`
```bash
python3 tools/gen_intrinsics.py --write
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract violated`
```bash
python3 tools/check_stdlib_intrinsics.py --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
print(payload.get("full_coverage_required_intrinsics", {}))
PY
```

`stdlib intrinsics lint failed: ...` (dynamic message, e.g. baseline invalid/missing)
```bash
python3 tools/gen_stdlib_module_union.py --write
python3 tools/gen_stdlib_stubs.py --write
python3 tools/check_stdlib_intrinsics.py
```

`stdlib intrinsics lint failed:` (file-level scan errors block)
```bash
python3 tools/check_stdlib_intrinsics.py
uv run --python 3.12 ruff format src/molt/stdlib tools/check_stdlib_intrinsics.py
```

### Differential Expected-Failure Policy (Too Dynamic)
Use this for intentionally unsupported dynamism (for example `exec`/`eval`)
that is documented by vision/break-policy constraints.

```bash
# 1) Declare the policy on the differential source itself:
#    # MOLT_META: verified_subset_scope=dynamic_execution_policy \
#    #   expect_fail=molt expect_fail_reason=too_dynamic_policy
#
# 2) Run the differential lane as normal. The shared typed policy converts a
#    Molt failure to XFAIL only when CPython passes and the source marker applies.
uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic/exec_locals_scope.py

# 3) XPASS is a failure; remove the source-local expected-failure pair when Molt
#    gains support. Dynamic-policy guards discover the physical suites from the
#    verified-subset policy, so there is no second path registry to update.
```

## Disk pressure and storage placement

Observe free space on the filesystems actually receiving output. Use the
existing DX resolution and `molt.disk_capacity` admission for host builds;
explicit output and cache overrides retain their own filesystem. For Linux
VM work, inspect both guest free space and the host filesystem holding the
instance disk. A roomy guest can still exhaust its host, and external host
Cargo/cache routing does not relocate a VM disk.

Reclaim through the existing owners:

- `tools/disk_guard.py --root <artifact-root> --ensure-free --dry-run --json`
  plans eligible build-state reclamation. Review its scope before applying.
  A successful plan or collector run does not admit the next build: remeasure
  the actual output filesystem through the capacity authority.
- `tools/molt_cache_prune.py --cache-dir <cache-root> --max-gb <budget> --dry-run`
  plans cache retention. Protected legacy publishers, held readers and
  installation state remain protected; an empty plan is not permission for
  a broad filesystem deletion. Reconcile and retire obsolete producers with
  their storage and lifetime consumers; protection is not an indefinite
  compatibility lane. Keep one current protocol and resolve live ownership.
  Completed `toolchain_symbol_facts/vN/<digest>.json` records use that same
  budget and identity-fenced recency; unknown schemas and stages are excluded.
  Shared `runtime-wasm-generations-v2/<family>` entries use persistent external
  stripes for publication, admission, binding and eviction. The family directory
  records successful ledger renewal; immutable members are never touched for
  recency. Even a zero budget preserves the existing 24-hour reader grace.
  Selected target/runtime installation stores keep their own lifetime. See
  [the storage policy](design/foundation/56_dx_buildspeed_tooling.md#28-one-product-store-per-host-hf-390).
- `tools/linux_vm.py` owns VM definitions, idle-work checks, service startup
  and host-file release. Lima's explicit `LIMA_HOME` owns disk placement.
  Follow [the storage relocation protocol](design/linux_vm_apparatus.md#storage-placement-and-capacity)
  to preserve stopped guests and their identities before retiring originals.

Record the measured growth location, its producer and retention owner, and
the actual free-space change. Recurring pressure requires correcting that
writer's placement or retention policy. A one-time cleanup, a removed snapshot,
an external-volume preference or successful dry-run alone proves no durable fix.
On APFS, inspect snapshots when deleted or discarded blocks do not become free;
report retained snapshot blocks separately from new file growth. Discarding a
restore point requires the applicable user authorization.
Keep private operation evidence outside tracked source. `AGENTS.md` owns this
standing policy; `CLAUDE.md` imports it.

## Storage authority and auxiliary retention (HF-582)

The closed `molt.custody_layout.StorageClass` and exhaustive `storage_rule`
own defaults, producer and lifetime. DX resolves `StorageLocation` with its
selected volume and path, including the existing unbound SDK user-state
selection. Use `molt.dx.storage_path`; a private root is not a new storage class. The
[storage design](design/storage_authority.md) records placement, the measured
toolchain exception and upstream retention protocols.

`python tools/molt_dev.py storage inventory --repo /checkout` shows the
selected paths and policies. `storage trim --repo /checkout --dry-run` reports
last-use retirement; `--apply` runs it. The existing daily checkout collector
also runs auxiliary retention under its single admission lock. Current/recent
controllers and pinned toolchains stay; browser/Edgebox input trees use their
lock-file producer and command lifetime locks; an exclusive lifetime lock is required
to retire any older proven generation. Provider caches retain their own
protocols: uv uses `uv cache prune`, and an unsatisfied byte budget is reported.
Compile-cache selection honors the family artifact binding even when
`MOLT_EXT_ROOT` is unset. Toolchain providers publish last use only after exact
receipt/hash validation, with external locks covering publication and readers.

Controller source/environments, the landing sandbox, lane payloads and gate
receipts now project onto the artifact root. Gate subprocesses receive the
canonical run context, including provider caches and scratch temp roots.
The runner admits output capacity before materializing that captured context;
it does not reselect the prepared image or create outputs on refusal.
The shared sccache server keeps stable provider temp beside its cache. When
that path exceeds the startup socket limit, DX records an artifact-root
assignment for a new cache under a cache-adjacent lock; every later client
uses the same assignment. Existing unassigned cache/temp payload or a live
server blocks that assignment for explicit operator reconciliation. Projection
and inventory still compute an unfittable path. Under `MOLT_USE_SCCACHE=auto`,
startup socket overflow declines the generated cache with one warning, leaving
no `SCCACHE_DIR`, wrapper or endpoint default. Explicit enablement or an explicit
unfittable `SCCACHE_DIR` raises `SccacheSocketPathError` before launch. Select a
shorter artifact root or an explicit short `SCCACHE_DIR` to enable caching.
Coordination records and recovery indices stay in the checkout. Evidence never
expires into deletion: compaction verifies an archive, publishes its external
manifest and source index, then removes only represented raw payloads.

Existing state moves only through a separately reviewed operation:

```sh
python tools/molt_dev.py storage migrate --repo /primary/checkout --dry-run \
  --legacy-temp-root /original/host/temp
```

Review every `keep`/`live-lock` row before approving `--apply`. Unknown preimages,
active/referenced lanes, locked landings and recent use remain. Named replaced
repository layouts use the same last-use collector in daily GC; historical user
cache and host-temp inspection require explicit operator scope. Legacy temp
idle for six hours is archived with an index before raw retirement, including
unregistered signal; names never grant disposable-data custody. Valid integrator
`MOVED.txt` transfers report `already-moved`. No test applies live migration.
Unknown ignored lane signal is harvested into the lane payload class before
handoff/retirement. Verified archives and source indices are recorded in common
Git storage under `molt/lane-payload-index`, which survives slot reuse. Stale
provisioned inputs and live/malformed lock custody still require reconciliation.
Gate environments replace ambient TMPDIR/TMP/TEMP with canonical scratch;
pytest also updates Python's cached tempfile view during early bootstrap.

The `external-provider` class reports Cargo home, rustup home and npm cache
sizes and provider retention. It preserves their configured locations and
compiler-input custody; Molt never migrates or prunes them. Cargo's global-cache
GC is disabled offline; npm cache verification is provider/user invoked and has
no automatic age bound. See the primary sources in the storage design.

Tests exercise temporary roots and check actual Python writes against typed
claims; exhaustive typing rejects classes without placement/retention policy.

## Build Throughput (Multi-Agent)
- **Concurrent backend daemon**: one daemon serves every build that shares its configuration digest. The digest and the socket path bind only process-scoped facts (project, build-state root, Cargo profile, compiler binary and fingerprint, frontend tooling fingerprint, features, target, suite lease, session), so every program and codegen setting of a build shares one warm daemon. Each connection gets its own thread, at most one compile per core runs, and the rest wait for a slot. Concurrent requests for one artifact compile it once (single flight); in-memory cache and single-flight keys bind the request's configuration namespace, complete typed native/WASM target and admitted semantic settings ([identity and transitions](design/backend_compile_daemon.md#cache-identity-and-job-transitions)). A request's backend settings (the catalog in `src/molt/backend_environment.json`) live in a per-request scope on the compiling thread (`molt_ir::backend_environment`, the `Session` model of rustc and Bazel's multiplex workers); the daemon never writes them into its process environment, worker threads and native batch workers receive the scope explicitly, and a read outside a request scope panics. Compiler crates read the catalog only through that typed accessor; each crate's `clippy.toml` disallows `std::env::var` so a new direct read fails lint. A daemon retires on a `retire` request, or by itself after `MOLT_BACKEND_DAEMON_IDLE_TIMEOUT` seconds (default 1800, the sccache and Gradle model) with no connection: it unlinks its socket, so the next build starts a fresh daemon, then finishes the accepted connections and exits. It unlinks only the socket it bound, so a successor on the same path keeps its own. The daemon alone decides when it retires; the launcher never stops a daemon whose socket is gone, and starts its successor. `tests/molt_diff.py` retires an idle daemon whose RSS exceeds `MOLT_DIFF_DAEMON_MAX_RSS_KB` and never stops a busy or draining one.
- **Stable backend cache keys**: compiler IR and backend cache payload identities do not depend on the host hash seed. Wrapper build reuse keeps its broader environment contract, which includes `PYTHONHASHSEED` for external build inputs.
- **Host hash seed**: the compiler keeps the launching interpreter's hash-seed policy and does not restart Python. Set ordinary `PYTHONHASHSEED` only for host-seed experiments; installed `-I` isolation keeps CPython's normal semantics.
- **Cache invalidation scope**: object-cache keys use IR payload plus runtime/backend/tooling content and source-tree metadata fingerprints. Unrelated stdlib file edits do not invalidate every cached object unless they affect generated IR for that build, but compiler/runtime/tooling source metadata changes do invalidate caches to prevent stale outputs in long-lived processes. Shared exact-key artifacts, including module/function `.o`/`.wasm` cache entries, `shared-stdlib/<key>/stdlib.a` sidecars and content-addressed `native-stdlib-archives-v1/<key>/stdlib.a` generations, are non-destructive during normal build/link/probe paths: mismatches skip reuse and rebuild/publish under the entry lock, while deletion belongs to retention cleanup (automatic development proof-queue maintenance, `molt clean`, `tools/throughput_env.sh --apply`, or explicit prune policy).
- **Required native archive custody**: `MOLT_STDLIB_OBJ` is the build-owned archive used by the linker. The optional `MOLT_STDLIB_CACHE_ENTRY` is hydrated before cache admission or published after cold compilation; both archive and sidecars transfer while cache custody is held. Daemon requests carry both paths in their own request scope, so concurrent requests never see each other's paths. Linking never reopens an optional cache generation. A build that misses the shared entry takes the entry's producer stripe and checks again before it compiles, so concurrent builds compile one stdlib archive and hydrate it. Required output is outside the selected cache root or under its durable canonical `home` subtree; canonical home physical targets remain protected even when a later environment selects another home.
- **Persisted publication**: source-hash, artifact-sync, import-scan, module-analysis, graph, diagnostics, emitted IR, snapshot, split-runtime, validation, package/archive, vendor file, linker-support, and other persisted JSON/text/byte/file cache or sidecar writers publish through unique same-directory temp files plus atomic replace. Vendored directory tree replacement prepares a hidden temp tree and preserves the previous tree for restore-on-failure; do not describe it as a universal kernel-level atomic directory exchange until every supported OS has an exchange primitive wired. Module-analysis cache identity includes the import scan mode (`full` vs `module_init`) in both the filename key and payload schema, so different graph semantics never alias the same artifact.
- **Lock-check cache**: deterministic lock checks are cached in `lock_checks/` under the project's Cargo target to avoid repeated `uv lock --check` and `cargo metadata --locked` on every build; lock-check JSON publishes through the same unique atomic temp-sibling writer as the other persisted cache state.
- **Concurrent rebuild suppression**: backend/runtime Cargo rebuilds acquire file locks under the resolved build-state root's `build_locks/` directory. A shared Cargo target resolves to shared control state regardless of per-run receipt or disposable output placement. Distinct session targets have distinct target-addressed control roots; an explicit `MOLT_BUILD_STATE_DIR` remains authoritative. Ordinary DX environments default `CARGO_INCREMENTAL=1`; deterministic proof/CI environments may select `0` explicitly. Configured sccache wrappers require `0` through the shared Cargo environment policy. Interrupted Cargo work quarantines only incremental payload under its target rather than deleting shared artifacts. Exact-key backend cache publication uses session-independent locks under the resolved cache root's `locks/compile.<key>.lock`, so separate target/session roots still serialize publication to a shared cache entry.
- **Project Cargo target**: [`molt.dx.project_cargo_target_dir`](../src/molt/dx.py) is the one rule for the Cargo target a Molt command builds a project into. An explicit `CARGO_TARGET_DIR` wins (a relative value is relative to the working directory, as Cargo reads it). Otherwise explicit artifact configuration (`MOLT_EXT_ROOT` or a durable family binding), `MOLT_PREFER_EXTERNAL_ARTIFACTS` or `MOLT_REQUIRE_EXTERNAL_ARTIFACTS` selects `<artifact root>/target/checkouts/<checkout-component>`, as `molt dx run` does; an unconfigured project keeps `<project>/target`. A pinned `MOLT_SESSION_ID` adds `sessions/<component>` below the selected checkout target; a generated one never does. The runtime and backend builds, build control, the backend daemon and its suite lease, the compile governor, the lock-check cache, the WASM host and the MLIR backend all read it.
- **Checkout state lifecycle**: under a shared artifact root a checkout keeps state roots named by `molt.dx.checkout_component`: its Cargo target (`target/checkouts/<component>`), its published Python generations (`build-environments/development/<component>/<recipe_sha>`), its pytest cache (`tmp/pytest-cache/<component>`) and its Hypothesis home (`tmp/hypothesis/<component>`); each target owns a build-control root (`tmp/build-control/<address>`). [`molt.checkout_state`](../src/molt/checkout_state.py) owns their layout, owner records (`.molt-state-owner.json`), last-use stamps (`.molt-last-use`) and retirement policy ([design 56, section 2.6](design/foundation/56_dx_buildspeed_tooling.md#26-checkout-state-retention-hf-313)). Python-generation owner/use metadata lives outside the immutable image under the existing state-lock metadata authority; other roots retain their in-root metadata. Legacy `uv-project-envs` roots remain recognized for custody and retirement, not new managed selection. A pinned session's target (`<target>/sessions/<session>`) is a root of the same policy. `RunContext.canonical_env` marks selected roots and a pinned session's target; a CLI build marks the checkout target, session target and build control it selects as used. Python readers hold generation use leases through observed process closure, while retirement requires the exclusive state lock. Selecting a new generation does not revoke an older live reader. [Checkout provisioning](ops/INTEGRATION.md#checkout-provisioning) owns publication, selection and the distinct verifier/payload cache policies. The disk guard retires a root unused for `MOLT_CHECKOUT_STATE_MAX_AGE_DAYS` (default 30) whatever its owner, a session target unused for `MOLT_SESSION_TARGET_IDLE_HOURS` (default 6), a root whose recorded owner is gone once unused for two hours (build control right after its target or session), and, under disk pressure, roots unused for two hours oldest first. A checkout target's retirement takes its session targets along. Inside the roots that stay, the same pass retires the per-target layout the host's product store replaced (`.molt-native-generations`, `.molt-backend-generations`, runtime `*.generation.json` receipts, `backend_probe_validations/`) once idle for two hours and under the root's exclusive retirement lock ([design 56, section 2.8](design/foundation/56_dx_buildspeed_tooling.md#28-one-product-store-per-host-hf-390)). It runs at most once a day from `RunContext.canonical_env` and on demand with `tools/disk_guard.py --gc`. `tools/molt_dev.py integrate --cleanup-worktree` and `lane retire` retire the worktree's daemons, remove it, then reclaim its roots and its targets' build control at once; a failed removal keeps them. A held lock (Cargo's per-profile `.cargo-lock`, build control's build, diff and daemon-suite owner locks, including those of a session inside a target), `CARGO_TARGET_DIR`, the current `MOLT_SESSION_ID`'s target and the running interpreter's own environment always keep a root; guard markers elsewhere in the artifact root do not. A plain clone keeps its target in the checkout and its scratch in the host temporary folder, so it has no such roots.
- **Command resource lifetime**: command-aware state projection requires the actual argv. It retains declared `VIRTUAL_ENV` and existing bound Python images enclosing possible native launch resources, including unactivated executable paths and finite PATH/cwd/application search coordinates. A candidate may retain an unselected image until child closure; this conservative shared lease establishes no interpreter identity or recipe admission. Optional absent, unowned or inaccessible coordinates confer no materialization or new launch refusal. Environment and suite owners use the explicit environment-only projection; low-level caller-owned process launches require their caller's lifetime owner. These rules do not infer arbitrary loader, shebang or unmanaged alias targets.
- **Completed Cargo incremental retirement**: plan with `tools/disk_guard.py
  --root <artifact-root> --retire-cargo-incremental <path>
  --guard-receipt <retired-guard.json>`; after independent review, repeat with
  `--inventory-sha256 <reviewed-digest> --apply`. This explicit operation never
  enters automatic GC. It accepts only a bound checkout session's
  `.molt-toolchains/<digest>/[<target>/]<profile>/incremental`, a successful
  descendant-closed Cargo guard binding both roots, and an unchanged inventory.
  Checkout-state custody excludes managed readers. The intermediate profile's
  existing `.cargo-build-lock` and output profile's existing `.cargo-lock` stay
  held through canonical durable removal ([Cargo layout](https://doc.rust-lang.org/beta/nightly-rustc/src/cargo/compiler/layout.rs.html)).
  Missing locks, indirect paths, unknown file kinds and changed custody refuse.
  Deps, products, fingerprints, seals, source, receipts and external hardlinks
  remain; externally linked storage contributes zero estimated reclaimed bytes.
  Raw Rustc writers bypassing Cargo locks and concurrent raw Cargo clean are
  unsupported. Each inventory visits at most 100,000 entries within ten seconds;
  payload bytes are not read, locks are nonblocking, and no process is launched.
  Apply inventories twice; failed durable removal leaves named residue for
  recovery review. Build admission must remeasure actual free space afterward.

- **Python runtime refresh**: ordinary checkout provisioning reads the published selection metadata; command admission checks live inputs. After a base interpreter changes, or after the runtime-closure schema changes (selection then fails with "run canonical ensure"), clear `UV_PROJECT_ENVIRONMENT` and run `PYTHONPATH=src <slot image>/bin/python -B -m molt.checkout_provisioning ensure --checkout <checkout> uv-environment --refresh-runtime`. The existing slot image only hosts the provisioning code (it carries `packaging`; a bare base interpreter does not); a new image is published in about 12-17 s. This explicitly recaptures the runtime through the existing image publisher and updates the default selection under its provisioning lock. Previous generations keep their lifetime protection and are never repaired in place. The flag applies only to `ensure` with `uv-environment`; ordinary selection keeps its cheap metadata path.


- **Build-state authority**: [`molt.build_state_layout.build_state_root`](../src/molt/build_state_layout.py) is the shared projection for CLI builds, daemon custody, governors, and differential harness locks; `project_build_state_root` applies it to the project's Cargo target. Explicit `MOLT_BUILD_STATE_DIR` selects the state root (relative paths are project-relative). Otherwise, with explicit `MOLT_EXT_ROOT` or a durable family binding, state lives at `<artifact root>/tmp/build-control/<target-address>/`; the address comes from the canonical Cargo-target identity, not the receipt or guest directory. Only environments without an artifact root use `<CARGO_TARGET_DIR>/.molt_state/`. Select disposable payload locations separately; never hand-compute the address. CI failure-artifact uploads resolve this same authority under the admitted harness environment.
- **Benchmark build-cache policy**: `tools/bench.py` and `tools/bench_wasm.py`
  use cache-enabled Molt builds by default, preserving non-destructive
  frontend/backend/runtime cache entries across concurrent benchmark agents.
  Pass `--no-molt-build-cache` only for deliberate no-cache/cold-rebuild
  investigations.
- **sccache auto-enable**: the CLI enables `sccache` automatically when available (`MOLT_USE_SCCACHE=auto`); set `MOLT_USE_SCCACHE=0` to disable. Cargo builds now auto-retry once without `RUSTC_WRAPPER` when a wrapper-level `sccache` failure is detected.
- **sccache server temp dir**: the shared sccache server keeps the temporary directory of the client that starts it, and sccache has no setting of its own for it. Every Cargo child whose wrapper is sccache, and every direct CLI sccache client, therefore runs with `TMPDIR`, `TMP` and `TEMP` set to `$SCCACHE_DIR-tmp` (`$MOLT_EXT_ROOT/.sccache-tmp`), never a run's scratch directory (`src/molt/cargo_execution_policy.py`).
- **sccache server endpoint**: sccache's default endpoint, TCP `127.0.0.1:4226`, is shared by every sccache on the host, including a VM's forwarded guest port (Molt's own VMs forward none: [Linux VMs](#linux-vms-lima)); a foreign server there runs Molt's compiles in its own environment (2026-10-16: a Lima Linux guest's server made every macOS `rustc -vV` probe fail with "Compiler not supported"). Molt pins each cache's clients to a private server: `SCCACHE_SERVER_UDS=$SCCACHE_DIR-server.sock`, or `SCCACHE_SERVER_PORT` derived from the cache path where a socket path would exceed 103 bytes or on Windows. An endpoint the caller sets is kept, and `molt dx` exports the same endpoint, so `sccache --stop-server` and `-s` in those shells reach Molt's server.
- **Dev profile routing**: `molt ... --profile dev` maps to Cargo profile `dev-fast` by default. Override with `MOLT_DEV_CARGO_PROFILE`; use `MOLT_RELEASE_CARGO_PROFILE` for release lane overrides.
- **WASM iteration policy**: without explicit iteration or Cargo incrementality controls, source-checkout runtime builds retain the nonincremental default for both `dev` and `release`. A valid `MOLT_RUNTIME_BUILD_PROFILE` or enabled `MOLT_RUNTIME_WASM_INCREMENTAL` opts into stable family placement; an explicit `CARGO_TARGET_DIR` takes precedence, followed by a deliberately pinned session. Generated session labels do not isolate build state. Proof scratch, `SOURCE_DATE_EPOCH` and `MOLT_RUNTIME_WASM_INCREMENTAL=0` prevent automatic family placement and force incrementality off; `CARGO_INCREMENTAL=0` also forces it off without changing placement. Otherwise the captured Cargo profile and controls, after canonical sccache normalization, determine actual incrementality. Diagnostics report placement, effective incrementality and shared publication separately. Incremental emissions cannot populate the shared cache; bound nonincremental emissions remain eligible regardless of placement, subject to exact artifact admission. [Cargo](https://doc.rust-lang.org/cargo/reference/profiles.html#incremental) owns profile inheritance and package overrides; runtime capture admits those exact controls before compilation.
- **Runtime artifact stability**: runtime Cargo feature sets are stable for the selected stdlib profile and target. User import graphs and required WASM exports must not mutate the runtime Cargo command or fingerprint; they affect user-code lowering, linking/export validation, and user-artifact cache keys only. Shared and relocatable WASM runtimes publish as one manifest-last generation whose location-independent identity covers the exact source closure, immutable provisioned toolchain content, resolved profile/features/flags/link plan, and publication transforms for both members. Ordinary shared-cache hits admit the existing family-addressed WASM generation in place, renew its store use, and pin its receipt before codegen; they do not copy or republish members when another family replaces the host selection. The existing generation-store lock serializes admission renewal and eviction: selected members remain available, and use renews the 24-hour idle window. Codegen binding renews its pinned receipt and members; same-family replacement preserves that window. Missing or changed members fail exact binding and linker admission, including after expiry; this is bounded retention, not an indefinite build lease. Cache admission, explicit relocation, linker admission, and deployment require caller-trusted identities plus both exact hashes; adjacent checksum/receipt files, path freshness, and cross-profile substitution have no authority. Exact uncached identities are compared before and after Cargo, input mutation refuses publication, and a successful Cargo run without the matching `molt_runtime` artifact fails closed. Native static archives use an ordered semantic identity over linkable member names and contents, excluding only derived archive metadata and local Molt rustc CGU discriminators; candidates without that identity are rebuilt rather than hydrated.
- **Native runtime archive aliases**: Cargo still produces scratch
  `libmolt_runtime.a`, but Molt publishes and links profile-qualified aliases
  under the selected target/profile directory:
  `libmolt_runtime.stdlib_micro.a` and `libmolt_runtime.stdlib_full.a`. Cache
  freshness, daemon staleness checks, and native link inputs must use these
  aliases, including target-triple-specific aliases, so `micro` and `full`
  builds cannot overwrite or stale-check each other through the scratch name.
  The `micro` profile includes collection and filesystem/tempfile intrinsics
  because core/default-profile stdlib imports such as `collections.abc`,
  `copyreg`, `runpy`, and `tempfile` must link against a coherent intrinsic
  surface.
- **Backend daemon**: native backend compiles use a persistent daemon by
  default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift cold-start. Tune
  startup with `MOLT_BACKEND_DAEMON_START_TIMEOUT`. The daemon enforces bounded
  request/job/cache state by default; tune with
  `MOLT_BACKEND_DAEMON_REQUEST_LIMIT_BYTES`, `MOLT_BACKEND_DAEMON_MAX_JOBS`,
  and `MOLT_BACKEND_DAEMON_CACHE_MB`.
- **Backend TIR cache memory**: `runtime/molt-backend` keeps TIR artifact bytes
  in an LRU in-memory cache in front of the disk-backed store under
  `MOLT_CACHE`. The default cap is adaptive: an explicit
  `MOLT_BACKEND_TIR_CACHE_MEMORY_MB` wins, then propagated memory-guard
  availability/reserve env (`MOLT_MEMORY_AVAILABLE_GB`,
  `MOLT_MEMORY_RESERVE_GB`, and `MOLT_CLI_*` aliases), otherwise total
  physical memory. The shared compiler resident-cache authority charges retained
  keys, quarantine paths, payloads, Arc headers and entry/link storage. Daemon
  `cache_bytes` and TIR `resident_bytes` report this conservative ownership
  charge, not process RSS; shared allocations are charged per entry. Oversized
  entries are returned without retaining them, and warm hits allocate nothing.
  The full contract is in [the daemon design](design/backend_compile_daemon.md#resident-ownership-and-eviction).
- **Backend TIR cache concurrency**: `molt-passes::tir::cache` is a lock-free
  content-addressed store, the design of Go's build cache
  (`cmd/go/internal/cache`). Each artifact is an authenticated envelope (key,
  payload length, SHA-256) published by a no-clobber hard link, and every read
  verifies it, so opening, reading and writing take no lock and flush nothing;
  crash residue reads as a miss. A different payload for an existing key
  quarantines the key with a `<hash>.poison` sidecar and fails the compile with
  an integrity error. `MOLT_BACKEND_TIR_CACHE_DISK_MB` and
  `MOLT_BACKEND_TIR_CACHE_MAX_ENTRIES` are enforced by a trim that runs at most
  hourly per namespace under a non-blocking `.trim.lock`: it evicts by file
  modification time (refreshed on use at most hourly) and retires sibling
  backend namespaces unused for five days. The previous design serialized every
  insertion through one exclusive namespace lock with four `F_FULLFSYNC`
  flushes (20 ms median each on the build volume), so parallel compiles ran one
  at a time. `CompilationCacheStats.telemetry` counts hits, writes, reuses,
  quarantines, trims and evictions.
- **Backend TIR cache root**: molt resolves the cache root once
  (`molt.default_paths`) and passes it, absolute, to every backend process as
  `MOLT_CACHE` (`molt.backend_environment.backend_process_environment`); the
  JIT runner takes it from its image. The process-scoped `cache` group of
  `src/molt/backend_environment.json` holds the root and the TIR and daemon
  cache limits, so a daemon serves only builds that share them. A backend
  started without `MOLT_CACHE` (a Rust test, a direct run) keeps no
  persistent TIR cache. It no longer falls back to `.molt_cache` in its current
  directory, which for a daemon was the compiler checkout (HF-443). Each TIR
  cache writes `CACHEDIR.TAG` into its own directory when it first creates it
  (a write or a trim), without replacing a tag already there, so backup tools
  and lane retirement treat it as regenerable; the Molt cache root
  is never tagged, since it also holds durable state (`home/`: installed tools
  and `MOLT_BIN`).
- **One-shot backend stdin limit**: non-daemon backend IR reads from stdin are
  bounded by `MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES` and default to the same
  512 MiB ceiling as daemon requests. Msgpack and NDJSON stdin paths stream
  through the limit; JSON and CBOR stdin paths reject oversized input before
  deserialization rather than buffering unbounded request bodies.
- **Backend IR lease custody**: CLI backend dispatch writes request IR as a
  content-addressed MessagePack lease under `tmp/backend-ir-leases/` and passes
  `ir_path` or `--ir-format msgpack --ir-file` to daemon and one-shot backends,
  including factgraph. A one-shot lease ships every function body; a daemon
  lease ships only the bodies the daemon's probe named as missing (HF-516,
  `docs/design/backend_compile_daemon.md`). The writer reuses the sorted
  function-body codec and retains at most one encoded body. It creates each lease exclusively,
  preserves an existing path on collision, removes its own partial writes, and
  removes the lease after the request completes. Both receivers use the same
  typed complete-frame reader and reject nonregular file descriptors before
  reading. File inputs have no additional byte ceiling; the stdin ceiling
  above applies only to stdin. The daemon protocol has only digest-probe and
  MessagePack-file states; inline IR is retired. Its version is shared through
  `src/molt/backend_daemon_protocol_version.txt`. JSON exports and MLIR stdin
  retain their separate JSON contracts. These are compiler-side costs and
  dependencies; emitted programs do not carry the transport codec.
- **Daemon probe path**: every daemon build first sends a probe that names its function digests. A warm artifact hit answers it; a miss answers with the positions the daemon's function store lacks, and the CLI encodes only those bodies. A body evicted after the probe makes the daemon ask again, and the second lease ships every body.
- **Daemon socket placement**: sockets default to a local temp dir (`MOLT_BACKEND_DAEMON_SOCKET_DIR`, or explicit `MOLT_BACKEND_DAEMON_SOCKET`) so shared/external volumes that do not support Unix sockets do not break daemon startup.
- **Daemon lifecycle**: with the ordinary persistent target root, daemon logs
  and identity/fingerprint sidecars live in `backend_daemon/` under the resolved
  build-state root. Isolated Cargo targets resolve to their own control roots;
  receipt directories and disposable guest leaves do not own shared daemon state.
  If an agent sees daemon
  protocol/connectivity errors before a full IR compile request is sent, the
  CLI may restart the daemon once under lock for classified retryable failures.
  After full-request admission, the daemon owns the compile outcome: the client
  fails closed with daemon diagnostics and does not restart or launch a hidden
  one-shot compile that overlaps backend memory. Verified live daemon identities
  also survive short startup readiness misses, because a synchronous daemon may
  be busy compiling and must not be unlinked/rebound by a second daemon.
  Serial pytest assigns a default
  `MOLT_SESSION_ID=pytest-<pid>` when the caller has not supplied one, then the
  session guard drains daemon groups created during that pytest run at session
  finish; xdist workers use worker-scoped session ids and skip the serial
  session sentinel.
- **Test sessions build where a developer run does**: the pytest memory-guard
  handoff, and every pytest process, enters the Molt roots of the run context
  (`molt.dx.MOLT_ROOT_ENV_KEYS` through `RunContext.root_env`): `MOLT_EXT_ROOT`,
  `CARGO_TARGET_DIR`, `MOLT_CACHE` and the differential roots. Explicit values
  stay; tool caches, `TMPDIR` and the uv environment keep the caller's values.
  A generated pytest session id never scopes the Cargo target, so code under
  test never writes `<checkout>/target/sessions/pytest-*`. `tests/conftest.py`
  fails a session that adds entries to the checkout's own `target/` (for a
  plain clone, which builds in its own `target/`, only `sessions/pytest-*`).
- **One-shot backend capture**: one-shot native backend compiles must capture stdout/stderr through temp-file-backed handles instead of pipe-backed `capture_output=True`. Child/grandchild toolchain processes may inherit stdio; temp-file capture avoids silent stalls where the parent waits forever on a still-open pipe after codegen has already finished.
- **Native runtime overlap**: native builds start runtime verification/build overlap after cache/setup and join it only at the true native link boundary. `emit=obj` skips that async runtime work because there is no native link step to hide it behind.
- **Bootstrap command**: `uv run --python 3.12 python -m molt.cli dx env`
  reports the same canonical DX facts on Windows, macOS, and Linux, and
  `uv run --python 3.12 python -m molt.cli dx run -- <command>` runs a command
  under those facts without shell activation. This is the maintainer/agent
  development and proof-lane bootstrap, not a public compile requirement; real
  users may compile in place, use default Molt/Cargo locations, or pass explicit
  target/output flags. The resolver configures:
  - `MOLT_EXT_ROOT=<artifact-root>` (`molt.dx.artifact_root`: explicit environment root, then durable family binding, then requested healthy candidates or checkout custody; the clone itself for an unconfigured plain clone)
  - `MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache`
  - `CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target`
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` so differential runs reuse the same shared Cargo artifacts by default
  - `MOLT_DIFF_ROOT=<scratch>/diff`, `MOLT_DIFF_TMPDIR=<scratch>` and `TMPDIR=<scratch>`, where `<scratch>` is `molt.dx.scratch_root`: `$MOLT_EXT_ROOT/tmp/scratch-<checkout path digest>`, or the same lane-owned child of a per-checkout host-temp folder when the artifact scratch container would fall inside the checkout; control state keeps its existing shared addresses
  - `UV_CACHE_DIR=$MOLT_EXT_ROOT/.uv-cache`
  - `SCCACHE_DIR=$MOLT_EXT_ROOT/.sccache` and `SCCACHE_CACHE_SIZE=<policy default>`
  - `MOLT_USE_SCCACHE=1` and `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`; `CARGO_INCREMENTAL` stays unset, so the Cargo profile builds workspace crates incrementally while sccache caches registry crates
- **Scratch storage and budget**: `MOLT_SCRATCH_STORAGE` selects run scratch
  storage: unset or `disk` uses the default artifact scratch; queued guards
  retain their state-root sibling scratch default. Set
  `disk:<absolute-existing-directory>` to place scratch on a chosen disk,
  such as an existing directory on a local SSD. `memory` selects Linux
  `/dev/shm`; a bare absolute path selects a memory-backed directory you
  mounted, such as a macOS RAM disk. Explicit disk and memory roots override
  state-root sibling placement for data scratch only. Molt never creates
  or mounts a RAM disk. Locks, guard markers, build control, products and
  caches retain their configured placement, so processes with different
  scratch storage still agree on control state. Before a guarded run starts,
  the memory guard requires
  `MOLT_SCRATCH_BUDGET_GB` (default 4) free on the scratch volume and refuses
  the run otherwise, instead of failing mid-run with `ENOSPC`. Set a larger
  budget for full-suite pytest runs.
- **Artifact root policy**: persist maintainer/agent storage selection with
  `python -m molt.checkout_provisioning artifact-root --checkout /checkout --bind /absolute/root`.
  The root must already exist, be writable, lie outside source checkouts and
  pass `MOLT_EXTERNAL_MIN_FREE_GB` (default 20 GiB). The command never creates
  it. Every linked worktree reads the same
  `<git common dir>/molt/artifact-root.json` record without shell startup files.
  Explicit `MOLT_EXT_ROOT` takes precedence; preference flags cannot disable
  the binding. Without a binding, existing plain-clone defaults and candidate
  preferences remain. This is a maintainer storage choice, not a public CLI
  requirement. To inspect the selected path and source (`explicit-env`,
  `family-binding`, `external-candidate`, `plain-clone-default` or verified
  hosted custody), run the same `artifact-root` command without mutation flags;
  add `--json` for structured status. Use `--clear` to deliberately remove a
  binding, including one whose storage is offline. Neither command migrates
  existing artifacts or changes source/toolchain custody. Restart long-lived
  processes after changing the binding: configuration is cached per process.
  A missing, unmounted, replaced or unhealthy bound root stops selection with
  its observed state and repair command; restore/mount/free that storage or
  explicitly rebind/clear it. A remount with a new directory identity requires
  rebind. There is no fallback to the checkout/root volume. The proof supervisor
  now uses `<artifact root>/proof-supervisor/<namespace>` as well; an explicit
  immutable `--cargo-output-root` keeps its existing override, and source/receipt
  overlap protection remains mandatory. See [design 56](design/foundation/56_dx_buildspeed_tooling.md#durable-artifact-roots-hf-521)
  for the authority, upstream research and cost bounds.
- **Cache retention**: the DX resolver emits `MOLT_CACHE_MAX_GB` and
  `MOLT_CACHE_MAX_AGE_DAYS`; the POSIX compatibility wrapper
  `tools/throughput_env.sh --apply` still runs `tools/molt_cache_prune.py`
  using those policy defaults. Override with `MOLT_CACHE_MAX_GB`,
  `MOLT_CACHE_MAX_AGE_DAYS`, or disable via `MOLT_CACHE_PRUNE=0`.
- **Diff run coordination lock**: `tests/molt_diff.py` acquires `diff_run.lock` under the resolved build-state root, so agents sharing a Cargo target queue instead of running overlapping diff sweeps. Tune with `MOLT_DIFF_RUN_LOCK_WAIT_SEC` (default 900) and `MOLT_DIFF_RUN_LOCK_POLL_SEC`.
- **Matrix harness**: use `tools/throughput_matrix.py` for reproducible single-vs-concurrent build throughput checks (profiles + wrapper modes), with optional differential mini-matrix.
  - Example: `uv run --python 3.12 python tools/throughput_matrix.py --concurrency 2 --timeout-sec 75 --shared-target-dir "$MOLT_EXT_ROOT/cargo-target" --run-diff --diff-jobs 2 --diff-timeout-sec 180`
  - Output: `matrix_results.json` under the output root (`$MOLT_EXT_ROOT/...` by default).
  - `matrix_results.json` now includes `gate_status` (thresholds, observed counts, violation details, pass/fail).
  - Use `--fail-on-gate` to return exit code `2` on gate failure.
  - If external root is unavailable, pass `--output-root` explicitly only for an approved emergency override.
  - If rustc or backend cache publication reports hard-link fallback on an
    exFAT artifact root, keep the lane on the DX-selected artifact root and
    diagnose the specific cache authority; do not reroute proof lanes to legacy
    `E:` roots to hide the defect. The DX env should include
    `UV_LINK_MODE=copy` on exFAT roots; if it does not, fix RunContext
    instead of adding a one-off shell export.
- **Compile-progress suite**: use `tools/compile_progress.py` for standardized
  cold/warm + cache-hit/no-cache + daemon-on/off compile tracking.
  - Example: `uv run --python 3.12 python tools/compile_progress.py --clean-state`
  - Include `--diagnostics` to capture per-case phase timing + module-reason
    payloads from compiler builds.
  - Queue lane (daemon warm queue): add
    `--cases dev_queue_daemon_on dev_queue_daemon_off`.
  - Release-iteration lane (`release-fast` Cargo profile override): add
    `--cases release_fast_cold release_fast_warm release_fast_nocache_warm`.
  - Under host contention, prefer:
    `--max-retries 2 --retry-backoff-sec 2 --build-lock-timeout-sec 60`.
  - Timeouts are fail-safe: the harness kills only marker-scoped timed-out
    compiler children (`cargo`/`rustc`/`sccache`) before retrying/continuing;
    backend daemons remain under identity custody so warm state is not destroyed
    by a compile-progress retry.
  - `SIGTERM` exits (`rc=143`/`rc=-15`) are treated as retryable.
  - Snapshots (`compile_progress.json` / `.md`) are updated after each
    completed case so interrupted long runs still preserve progress.
  - Use `--resume` in persistent `tmux`/`mosh` sessions to continue interrupted
    long sweeps (especially release lanes) without rerunning completed cases.
  - Outputs: `compile_progress.json` + `compile_progress.md` + per-case logs.
  - Default output root: `$MOLT_EXT_ROOT/compile_progress_<timestamp>`.
  - If external root is unavailable, pass `--output-root` explicitly only for an approved emergency override.
  - Initiative KPI board lives at `docs/benchmarks/compile_progress.md`.
- **Build diagnostics**: enable compiler phase timing + module-inclusion reasons
  on demand with `--diagnostics`.
  - Optional output file: `--diagnostics-file <path>` (relative paths are
    resolved under the build artifacts directory).
  - Example:
    `uv run --python 3.12 python -m molt.cli build --profile dev --no-cache --diagnostics --diagnostics-file build_diag.json examples/hello.py`
  - Midend diagnostics now carry tier-promotion telemetry (`tier_base_summary`,
    `promoted_functions`, `promotion_source_summary`,
    `promotion_hotspots_top`) to verify PGO-guided promotion decisions.
  - For deterministic control experiments, set
    `MOLT_MIDEND_HOT_TIER_PROMOTION=0` to disable hot-function tier promotion.

## Weekly Stdlib Scoreboard
Update this table at least weekly during lowering burn-down.

Rows before 2026-10-09 counted a module as intrinsic-backed or intrinsic-partial
when it required any intrinsic, read or not (HF-91). From that date the counts
use only intrinsics a module reads, and `python-compiled` and `stub` have their
own columns; `probe-only` and `python-only` no longer exist.

| Date | intrinsic-backed | intrinsic-partial | intrinsic-support | python-compiled | stub | missing top-level | missing submodules | native parity pass % | wasm parity pass % | memory regressions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-02-12 | 177 | 696 | n/a | n/a | n/a | 0 | 0 | TBD | TBD | TBD |

Rules:
- Any PR that worsens this table requires explicit exception sign-off.
- Ratchet updates (`tools/stdlib_intrinsics_ratchet.json`) must only move downward and must ship with real lowering progress in the same change.

### Fast Dev Playbook (Recommended)
1. `tools/throughput_env.sh --apply`
2. `uv run --python 3.12 python -m molt.cli build --profile dev examples/hello.py --cache-report`
3. `UV_NO_SYNC=1 uv run --python 3.12 python -u tests/molt_diff.py --build-profile dev --jobs 2 tests/differential/basic`

Expected behavior in this lane:
- Environment activation prepares artifact directories without stopping or restarting the shared sccache daemon. Wrapper selection does not probe readiness; first actual compiler work starts or contacts the daemon.
- Runtime/backend Cargo crates rebuild once per fingerprint and profile, with parallel agents waiting on shared build-state locks whenever they share `CARGO_TARGET_DIR` or `MOLT_BUILD_STATE_DIR`.
- Differential jobs are clamped by the in-harness memory guard before launch and are killed with telemetry instead of allowing global RSS to run past the configured safety budget.
- SIGINT, SIGTERM or SIGHUP stops the suite at any `--jobs`: it admits no new case, ends the commands its workers are running (each case gets an interrupted receipt), writes its summary and exits with 128 plus the signal, within a few seconds whether the signal reaches the suite or its memory-guard parent.
- Live memory pressure can be watched with `python3 tools/memory_guard_stream.py --diff-root "$MOLT_DIFF_ROOT"` or emitted directly from the harness with `MOLT_DIFF_MEMORY_GUARD_STREAM=stderr`; neither mode creates an additional artifact tree.
- Backend codegen uses the persistent daemon for native builds when available (fewer cold starts across repeated builds).
- Per-script object artifacts are reused via `MOLT_CACHE` keying when IR+fingerprint inputs are unchanged.

When throughput regresses unexpectedly:
1. Check lock contention in `build_locks/` under the [resolved build-state root](#build-throughput-multi-agent).
2. Check `sccache -s` hit rates (in a `molt dx` shell, so it reaches Molt's private server) and wrapper disable/retry logs.
3. Run `tools/throughput_matrix.py` to isolate profile/wrapper/concurrency regressions.

## Remote Access and Persistent Sessions

SSH transports commands and observations; the remote host owns execution,
resource limits and cleanup. A local SSH timeout or disconnected session does
not establish that its remote descendants exited.

Use the existing [proof queue](agent/PROOF_QUEUE.md#detached-long-runs) for
long-running remote builds and proofs, with an explicit timeout, detached
custody and durable terminal evidence. Reconnect to that owner to inspect or
cancel the run. Use the canonical
[detached integration protocol](ops/INTEGRATION.md#the-detached-run-protocol-hazard-11)
for integration. Terminal multiplexers are optional interfaces to those owners;
they do not establish process custody or resource bounds.

Potentially expensive inspections need the same protection. Run process-table
queries, filesystem walks and PowerShell pipelines through remote
`tools/guarded_exec.py` with a finite `--timeout`, a `--cancel-file` and
`--metrics-json`. Inspect its terminal return code and `descendants_closed`
before retiring scratch files or treating the operation as finished. The
Windows guard assigns its child tree to the existing kill-on-close Job; do not
replace it with a bare SSH timeout or a second process supervisor. Keep metadata
queries narrow and retain only the evidence needed for the decision.

An interrupted inspection can consume enough memory to invalidate later proof
results. Preserve its ownership evidence and use the existing guarded cleanup
boundary; shell hosts and ambiguous processes still require explicit operator
authorization. Do not start another inspection of the same family while its
previous execution has unknown closure.

## Linux VMs (Lima)

On a macOS host, two Lima VMs reproduce Linux failures: `molt-linux`
(aarch64) and `molt-x86` (x86_64 through Rosetta, with an x86_64 cross GCC and
Lean). [`tools/linux_vm.toml`](../tools/linux_vm.toml) defines them; edit it,
never `~/.lima/<vm>/lima.yaml`. They mount no host path and forward no guest
port to the host. Design, measurements and costs:
[design/linux_vm_apparatus.md](design/linux_vm_apparatus.md).

```bash
python3 tools/linux_vm.py check                      # drift from the definition: exit 1
python3 tools/linux_vm.py push molt-linux --checkout # this checkout's HEAD into ~/molt-main
python3 tools/linux_vm.py push molt-x86 origin/main --ref main # as refs/heads/host/main
python3 tools/linux_vm.py release                    # free host files a mounting VM pins
python3 tools/linux_vm.py render molt-linux
python3 tools/linux_vm.py apply molt-linux --confirm-restart
```

- **Moving commits in.** `push <vm> [rev]` sends a commit of the checkout you
  run it from into the guest clone (`~/molt-main`) over Lima's SSH, as
  `refs/heads/host/head` (`--ref <name>`: `refs/heads/host/<name>`). It writes
  only that host-owned namespace, so a guest branch is never moved. With
  `--checkout` it checks the exact commit out, detached, but refuses while the
  clone has changes (untracked files too), a Git command holds its index, or
  a guest process has its working directory or an open file in the clone
  (read as root, so the guest needs Lima's passwordless sudo; a listing that
  lacks root's view fails the checkout); without it, it
  prints the checkout command. The check does not see a process that names
  the clone only by path between file accesses (`git -C`, `cargo
  --manifest-path`), or one that starts between the check and the checkout:
  on a shared VM, check by hand. For a writable tree,
  `git -C ~/molt-main archive host/head | tar -x -C /tmp/<x>` in the guest.
  Copy a single file with `limactl copy <file> <vm>:/tmp/`. A guest cannot
  fetch from a host path: nothing is mounted, and the clones' `origin`
  remotes, which still name `~/Molt/molt-src` on the host, no longer work.
- **Pinned host files.** A VM that still mounts host paths (one made by
  hand) pins every host file its guest looked up, even after the host
  deletes it. Apple's virtiofs server closes forgotten files only in batches
  of 64 per VM process, so `drop_caches` alone frees nothing. `release` makes
  the guest forget its unused inodes and then 64 more, which frees every pin
  no guest process holds open; lane retirement runs it for a VM that holds
  files of the retiring checkout, or deleted files its last release did not
  leave behind (those, a guest process holds open). `check` shows what each
  mounting VM holds. `release` never stops a VM. It keeps
  `<mount>/.molt-linux-vm-forget/` (64 empty files) under each mount while a
  VM mounts it; `apply` removes the sets of the mounts it ends, only their
  own empty files. Neither follows a link a guest planted there. A mount
  whose set cannot be written, or is a link, is reported, and the others
  still release.
- **Apply.** `apply` restarts the VM. It starts the VM as a service launch,
  so Lima's hostagent outlives `limactl start` (HF-471), and it succeeds only
  when Lima then lists the VM as running; otherwise, or when that listing
  fails and the hostagent is dead, it restores the replaced config and
  restarts the VM on it. Run it from a plain shell: under a live Molt memory
  guard (any checkout's, also one inside a tool such as `guarded_exec.py`)
  it refuses to restart a running VM, because the guard would close the
  hostagent too. The hostagent inherits no guard token, marker or scratch. It refuses while the guest is busy: a process of
  the guest user, or any process in a login session (`sudo` work too), whose
  executable is not one of the definition's idle executables (read as root,
  so the guest needs Lima's passwordless sudo, and the refusal says so); a
  process under the user's subordinate ids (a container user) or a
  containerd shim (a running container); or `apt`, `dpkg` or
  `cloud-init`. It does not see root work outside a login session (a
  `systemd-run` unit, a cron job) or an idle executable a user runs by hand,
  and a session that starts after its probe is stopped with the VM, so check
  by hand on a shared VM. It keeps the first config it replaced as
  `lima.yaml.molt-previous`.
- **Disk.** VM disk images live in `~/.lima` on the boot volume, so guest
  builds use the Mac's free space.

## Logging and Benchmark Conventions
Persistent sessions are only useful if output is durable and inspectable.

### Logging rules
- Long-running tasks MUST log to disk
- Logs should be line-oriented text
- Include timestamps and phase markers
- Avoid excessive stdout-only output

Recommended structure:
```
logs/
  compiler/
  benchmarks/
  wasm/
  agents/
  cpython_regrtest/
```

### Benchmarking rules
- Benchmarks must be reproducible
- Record:
  - git commit
  - machine info
  - date/time
- For WASM benches, capture the native CPython baseline and compute WASM vs
  CPython ratios only for rows where `molt_wasm_ok` is true and
  `molt_wasm_time_s` is a positive finite duration.
- When validating linked WASM runs, pass `--linked` to `tools/bench_wasm.py` and
  note in logs whether the run used linked or fallback mode.
- Prefer CSV or JSON outputs
- Do not overwrite previous results
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`)
  execute 10 samples, store raw successful sample arrays plus
  mean/median/variance/range stats in JSON output, and reject partial sample
  failure as benchmark evidence; reserve these for release tagging or explicit
  requests.

### Why this matters
- Enables async review
- Enables regression detection
- Enables phone-based inspection

### CPython regrtest harness
The `tools/cpython_regrtest.py` harness writes one run per directory under
`logs/cpython_regrtest/` and emits per-run `summary.md`, `summary.json`, and
`junit.xml`, plus a root `summary.md`. Each run also writes `diff_summary.md`
and `type_semantics_matrix.md` so parity work stays aligned with the stdlib and
type/semantics matrices; `--rust-coverage` adds `rust_coverage/` output
(requires `cargo-llvm-cov`). Use `--clone` and `--uv-prepare` explicitly when
you want networked downloads or dependency installs.
The shim treats `MOLT_COMPAT_ERROR` results as skipped and records the reason
in `junit.xml`; use `tools/cpython_regrtest_skip.txt` for intentional exclusions.
The shim is regrtest's worker. It stops each test at regrtest's `--timeout`
itself (CPython's own workers do this through faulthandler) and appends one row
per test to `item_results.jsonl` (`tools/compat/regrtest_results.py`), which is
the summary's item authority and survives a killed run. `--deadline-s` (needs
`--timeout`) admits only tests whose timeout still fits; the rest report
`DID_NOT_RUN` with reason `deadline`, so the run always writes its summary.
When `--coverage` is enabled, the harness combines host regrtest coverage with
Molt subprocess coverage (use a Python-based `--molt-cmd` to capture it). The
shim forwards interpreter flags from regrtest to the Molt command to preserve
isolation/warnings behavior.
Regrtest runs set `MOLT_MODULE_ROOTS` and `MOLT_REGRTEST_CPYTHON_DIR` to pull
CPython `Lib/test` sources into the static module graph without mutating host
`PYTHONPATH`.
Regrtest runs set `MOLT_CAPABILITY_TIER=full` by default, the host access CPython's
tests assume (`--molt-capability-tier` selects another tier); `--molt-capabilities`
grants more names beyond the tier.

Benchmarks that cannot survive a disconnect are not acceptable.

### Build time

`molt build --verbose` reports frontend lowering-cache hits and the admitted
compiler/runtime product paths. Equal immutable product paths across checkouts,
without a source-build notice, show product reuse. These lines use the existing
admission and cache results; they do not enable build diagnostics. An unchanged
build record finishes before these stages and reports `inputs unchanged`.

Compiler caches use atomic publication without durability barriers when their
contents can be reconstructed: source/import/analysis and lowering records,
toolchain and backend-analysis memos, wrapper build manifests, and both directions
of shared frontend-cache copying. Missing or malformed cache data is a miss;
each consumer still qualifies its source, context, tooling, target or artifact
content before reuse. The in-process JSON memo holds candidates, not attestations
of current cache-file bytes. Its metadata hint avoids rereading large JSON on a
warm lookup; the consumer's semantic identity remains authoritative. Writes
invalidate that memo rather than associating a submitted payload with a later
stat of a possibly replaced path. Artifact sync receipts retain their separate
current-byte reader and durability contract.

The shared `molt.file_publication.PublicationDurability` policy keeps required
outputs durable by default. `RECONSTRUCTIBLE` is an explicit owner choice, with
the same staging, complete-write, close, custody, atomic replacement and cleanup
protocol. It skips file/directory flushes and Windows write-through, without
unlinking an old entry first or copying across filesystems after a failed rename.
An exclusive publication (`atomic_write_bytes(exclusive=True)`) keeps its
no-replace rename under either policy. Memory-guard markers and guard scratch
records use `RECONSTRUCTIBLE` too: they describe processes that an OS crash
kills, and their readers treat a lost, empty or older record as crash residue
(design 56, section 2.7).
This follows [CPython's bytecode cache publication](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap_external.py#L183-L202),
which writes a private file and replaces it atomically without a flush barrier.
It changes compiler/development I/O only; emitted programs do no cache validation.

Exclusive staging grants cleanup custody only after creation succeeds. A
chosen pathname, an unattempted second output, or a failed `x`/`xb` open grants
no deletion authority. Copying, WASM transforms and generations, generated
outputs, native-link sidecars and custody archives, package blobs, LLVM setup
and backend IR leases follow that rule. Acquired partial stages are reclaimed
on later write or validation failure; colliding stages preserve their bytes and
identity. The fast copy path reserves its leaf, then copies with
`molt.file_publication.clone_file`: a copy-on-write clone on APFS (`clonefile`),
btrfs and XFS (`FICLONE`), an in-kernel copy elsewhere, the contract of Rust's
`std::fs::copy` that `molt_artifact_publish::copy_file_atomically` uses. A clone
writes no data, so a durable flush of it is cheap. A clone is a new file, so its
first read, such as the staged copy's digest check, comes from disk. On the
contended USB build volume a 67 MB clone plus that read took 3.5-4.0 s against
5.6-8.5 s for a byte copy; in whole builds the verified-copy sites kept their
wall time (WASM `backend_cache_write` 13.1 s and 12.7 s, medians) while writing
no data. This policy adds no guest execution work and does not establish
protection against pathname replacement after acquisition.

The ratio-direction gate and proof source capture share the lightweight Git
source inventory. Git selects tracked and eligible untracked paths, so ignored
operational logs and nested checkouts never enter the gate's AST scan. Tracked
source remains covered even inside an ignored directory. Missing admitted files
fail closed; the inventory itself makes no claim about current file bytes.

- `MOLT_BUILD_DIAGNOSTICS=1` with `MOLT_BUILD_DIAGNOSTICS_FILE=<path>` writes one
  `molt build`'s phase seconds (`phase_sec`, `ranked_phases`) and its entry as
  JSON. A value ending in `/` names a directory that receives one file per build;
  `python3 tools/build_phase_report.py <dir>` then ranks phases and the slowest
  builds across a whole differential run.
- `MOLT_CLI_PROFILE=<dir>/{pid}.prof` writes a cProfile of each `molt` command;
  `{pid}` keeps parallel builds apart. Read it with `python -m pstats`.
- Import scans, intrinsic source facts, module analyses and binary module
  lowerings share `<schema digest>/records.sqlite`, named by the DDL digest,
  under `<Molt cache>/toolchain_source_records`
  (`molt.cli.source_record_store`). Every project/session uses that store;
  no project copy is hydrated. Each SQLite connection holds one shared
  cache-control lifetime lock; warm record operations add no lock syscall.
  Other recognized namespaces retire only under an exclusive lifetime lock
  through atomic whole-directory retirement. Unknown future version namespaces
  and flat legacy stores remain protected; the latter are reported in
  `legacy_source_record_state`. SQLite/WAL/SHM files never retire independently.
  Source/context admission precedes recency refresh. Due renewals enter a bounded
  queue, saved in one nonwaiting transaction per 4,096 observations, at
  the owning operation boundary or explicit close, into a narrow `recency` table that leaves payload rows untouched.
  GC closes SQLite without writing advisory renewals and defers only the lifetime
  lease until safe normal-context release. Exact compiler generation remains
  part of read admission; retention belongs solely to the configured global
  byte/age cache budget for all four kinds. There is no generation trim or
  in-place migration. Run the existing `molt_cache_prune.py`
  command to inspect or apply it. Its totals combine file bytes and logical
  source payload bytes; separate DB/WAL/SHM measurements report physical storage.
  Exact source-record inventory scans a [SQLite-maintained expression index](https://www.sqlite.org/expridx.html) on
  `length(payload)`, with one compact length entry per record. Inventory remains
  linear in record count; publication, replacement and deletion maintain the
  index transactionally, while recency-only updates leave it untouched. BLOB
  length does not require loading the entire payload; the index avoids sparse
  payload-table page access. [`INDEXED BY`](https://www.sqlite.org/lang_indexedby.html)
  requires this access path; an unavailable index is a cache maintenance error.
  Adding this index changes the DDL digest, so its
  namespace starts empty and repopulates on cache misses. Old and new namespaces
  may coexist until the existing lifetime-protected retirement permits removal;
  there is no in-place index migration or separate application byte counter.
  Each metadata page releases its read snapshot before yielding to row or file
  eviction. The existing inventory count bounds examined observations, including
  repeated rows after recency changes or backward clock movement; an upper key
  and forward cursor constrain selection. Concurrent totals and selection are
  approximate. Deletion requires the exact observed revision and recency, so a
  stale observation cannot delete a replaced or renewed row. Other peer snapshots
  can still defer physical reclamation; the logical byte budget is not a hard
  DB/WAL physical-size limit. Admitted source-record reuse keeps the five-minute
  recency interval and exact generation/revision/use comparison, but skips a busy
  process-local lock or SQLite writer. It does not reopen a closed store solely
  for advisory recency. A flush saves, temporarily disables, then restores
  the existing connection busy timeout under its lock: three constant metadata
  statements replace up to 60 seconds of writer-lock waiting. Ordinary reads,
  publication and initialization retain their policies. This bounds contention
  admission, not I/O latency; checkpoint sync can still block. WAL uses
  synchronous NORMAL and default automatic
  checkpointing, so sync cost remains possible. Row deletion uses bounded
  transactions (256 rows/1 MiB, with one oversized row allowed), and later sweeps
  continue bounded vacuum work even when no new rows are removed. BUSY deferral
  is reported separately from maintenance failure. Dry-run never changes rows,
  schema or checkpoints, though SQLite may manage WAL coordination sidecars.
  Corruption makes this optional cache unavailable without deleting peer-owned
  database files. Lowering schema and digest ownership are defined in
  [the IR contract](spec/areas/compiler/SIMPLE_IR_JSON_SCHEMA.md).
- `MOLT_VERIFY_FUNCTION_DIGESTS=1` re-encodes persisted function digest seeds once
  during compiler cache setup for debugging. Ordinary builds reuse admitted
  unchanged bodies without that walk. It adds no emitted-program work.
- A `molt` process paces the collector for its large heap and ends without
  freeing it (`molt.compiler_process`). A profiler or debugger that runs the
  module, such as `python -m cProfile -o out.prof -m molt.cli build ...`, still
  gets control back and writes its output.
- Both reach builds that a harness starts, such as `tests/molt_diff.py`, so a
  differential run measures exactly the builds it makes.
- Run differential suites with `--jobs` near the core count; builds are mostly
  single-threaded. Set `MOLT_DIFF_RESULTS_JSONL=<path>` for one row per test and
  per backend, which is the per-target evidence (parallel console lines name
  only the file).
- Toolchain provenance is reused, not re-proved: a process captures each root set
  (Rust sysroot, LLVM SDK, C search paths such as the macOS SDK) once, and later
  fences verify it by directory and file stat tokens. File digests persist
  under the Molt cache (`file-digests/v1.sqlite`); the content identity rules
  are in [observed inputs and content identity](design/observed_inputs.md).

### Profiling harness
- Use `tools/profile_hotspots.py` first when the question is dev/CI wall clock:
  it reads the default guarded-command profile log and sorts slow subprocesses
  before you decide which runner or workflow deserves deeper profiling.
- Use `tools/output_startup_size_audit.py` when the question is tiny-binary
  startup or output size across targets/profiles/backends: the normal benchmark
  harness can hide first-path code-signature and loader costs by repeatedly
  launching the same executable, and target-only size claims are incomplete
  without the adjacent native/WASM/Luau/MLIR artifact rows.
- Use `tools/profile.py` for repeatable CPU/alloc profiling runs.
- Default outputs land in `logs/benchmarks/profile_<timestamp>/` with a
  `profile_manifest.json` that records git rev, platform, tool choices, env
  overrides, and per-benchmark artifacts/metrics.
- Prefer `--suite top` for Phase 0 profiling, and add `--profile-compiler` when
  you need compiler-side hotspots.
- Use `--molt-profile` to enable runtime counters (`MOLT_PROFILE=1`) and capture
  allocation/dispatch metrics in the manifest.
- Runtime microbenchmarks that make steady-state allocation, GC, or contention
  claims must bracket each measured phase with the profiling epoch intrinsics.
  The emitted `molt_profile_epoch_json` records process-wide counter deltas and
  signed gauge and source-tagged RSS endpoints without resetting the lifetime
  leak authority; unavailable RSS is explicit and never encoded as zero.
  Impossible monotonic decreases make the whole epoch non-claimable, retain
  `counter_regressions`, and null the affected delta rather than hiding it as
  zero. `tools/profile.py --summary` preserves every label and
  reports cross-run drift. `molt debug perf` also retains every epoch, including
  completed epochs from a log whose final process-exit profile is missing. All
  consumers reject unversioned process payloads rather than guessing a schema.
- Add `--summary` to emit a `profile_summary.json` with top alloc counters,
  allocation-per-call ratios, and hot string allocation sites across benches.
- `molt_profile_cpu_features` lines report detected SIMD features so profile
  manifests can capture which vector paths should be reachable on that host.
- Use `--cpu-tool perf-stat --perf-stat-events ...` on Linux for counter stats
  alongside the time/heap profiling.
- For WASM, use `tools/wasm_profile.py --bench bench_sum` to emit Node
  `.cpuprofile` artifacts in `logs/wasm_profile/<timestamp>/` (pair with
  `--linked` when validating single-module link runs).

## Progress Reporting

### Task scaffolding
Use the helper to create a per-task log + report skeleton:
```bash
tools/new-agent-task.sh <task-name>
```

### Minimal “micro-report” (for frequent updates)
Use this when updating every 10–20 minutes.

```markdown
- [<HH:MM>] <status> — <one-line update>. Output: <path>. Next: <one-line next step>. Resume: <command>.
```

### Full report format
```markdown
## <Task Title>

### Status
<short status line>

### Summary
- ...

### Files Touched
- <path> — <one-line change>

### Tests
- <command> → PASS/FAIL

### Notes
- ...

### Resume
<command>
```

### Coverage reporting helpers
Generate differential coverage summaries and check canonical gap ownership.
The type/stdlib matrices own complete records; the root roadmap must link to
both, and any roadmap duplicate must exactly match its canonical record:
```bash
uv run --python 3.12 python tools/diff_coverage.py
uv run --python 3.12 python tools/check_type_coverage_todos.py
```

## Multi-Agent Workflow
This section standardizes parallel agent work on Molt.

The canonical protocol for proof-lane ownership, task logs, targeted vs broad
validation, and collision handling is
[ops/MULTI_AGENT_COORDINATION.md](ops/MULTI_AGENT_COORDINATION.md). Agents
running long differential, conformance, regrtest, benchmark, or `molt validate`
lanes must follow that protocol first.

### Access and tooling
- Agents may use `gh` (GitHub CLI) and git over SSH.
- Agents are expected to create branches, open PRs, and merge when approved.
- Proactively commit work in logical chunks with clear messages.

### Work partitioning
- Assign each agent a scoped area (runtime/frontend/docs/tests) and avoid
  overlap.
- If cross-cutting changes are required, coordinate early.
- Keep one broad-sweep coordinator per shared target root for full
  differential, CPython regrtest, conformance, or release validation lanes.
  Other agents should run targeted proofs, reduce failure queues, or move to
  non-colliding structural work.

### Communication rules
- Always announce: scope, files touched, and expected tests.
- Keep status updates short and explicit.
- Flag any risky changes early.
- Use `tools/new-agent-task.sh <task-name>` or update an existing
  `logs/agents/<task>/` record before long proof work, including owned files,
  proof lane, artifacts, and next command.
- Use `uv run --python 3.12 python tools/agent_coordination.py env` to capture
  OS/Python/shell facts before choosing platform-specific commands. Then use
  `uv run --python 3.12 python tools/agent_coordination.py scan` to inspect
  active task claims, and `... check` to fail fast on duplicate broad-sweep
  coordinator claims for the same lane and shared target root.

### Quality gates
- Run extensive linting and tests before PRs or merges.
- Prefer `molt setup`, `molt doctor`, and `molt validate` as the canonical
  operator surface. `tools/dev.py` is now a convenience delegate only.
- Use `molt validate --suite smoke` for fast local proof and `molt validate`
  for the heavier full matrix, plus any targeted `cargo` checks required by the
  touched lane.
- Use `molt validate --suite smoke --backend luau` for Luau changes; the lane
  covers generated support-matrix freshness, checked Luau emission,
  Luau runner availability, backend/lowering Rust unit tests, and the targeted
  CPython-vs-Luau smoke under the shared memory guard.
- Executed `molt validate` runs persist their run payload under
  `logs/validate-<suite>-<backend>-<profile>.json` by default. Pass
  `--summary-out` for an explicit custody path; check-only plans only write when
  that option is supplied.
- `tools/dev.py bench` is the canonical convenience entrypoint for a guarded
  local benchmark smoke check. With no arguments it runs the pyproject-owned
  smoke command and writes `bench/results/dev-bench-smoke.json`; pass explicit
  `molt bench` arguments for custom or full benchmark slices.
- `tools/dev.py lint` includes the subprocess guard coverage audit and the
  memory-guard wiring audit; new raw subprocess calls in dev/test/bench
  surfaces must either use `tools/harness_memory_guard.py`/test process-guard
  helpers or document why they are bounded metadata probes or interactive
  guarded Popen paths, and new guard entrypoints must stay visible to the repo
  process sentinel.
  The combined wiring audit shares one fresh file inventory, byte observation
  and lazy AST across its raw-call, guard-wiring and minimal-environment rules.
  The raw-call visitor resolves each encountered AST type's inherited handler
  once per visitor, retaining the existing depth-first order and scope rules.
  Its bounded handler map is cleared on success and failure; neither trees nor
  completed audit verdicts persist across operations. Python profiling
  exposes first-time dispatch through `_resolve_visit_handler`.
- `tools/dev.py gates` persists the pyproject-owned local gate sequence under
  `logs/dev-gates-summary.json` by default. Pass `--summary-out` when a batch
  needs a named custody artifact; failed gates still write the partial command
  list, timings, guard limits, and failure reason before returning nonzero.
- `tools/dev.py test --random-order --random-seed <seed>` is an opt-in DX lane
  for flushing out order dependence. Keep the canonical proof/CI sweeps
  deterministic.
- Do not merge if tests are failing unless explicitly approved.

### Merge discipline
- Merge only after tests pass and conflicts are resolved.
- If two agents touch the same area, rebase and re-validate.
