# Memory guard custody sampling

Status: Darwin and Linux implemented, 2026-10-18. Owner: integrator
(coordination task `claude-guard-event-tree`). Ledger: HF-358 layer 2.

## Problem

The memory guard polls every 50-100 ms. Each poll reads the whole host process
table, then keeps the few rows of its custody tree:

- macOS: one `KERN_PROC_ALL` sysctl (about 1,360 rows of 648 bytes, 880 KB) and a
  Python unpack of every row: about 6 ms of parent CPU per poll, 0.35 s per
  2-second guarded child (HF-358, measured 2026-10-18).
- Linux: a scan of every `/proc/<pid>`.
- Windows already polls only its job's members (`windows_job_members`); the
  host snapshot remains for the exit census and cleanup.

The cost scales with the host, not with the tree, and thousands of guarded
test commands pay it.

## What a poll needs

`ProcessTreeTracker.update(samples)` admits a process when its parent is a
live tracked pid, and keeps a tracked pid after it is reparented. Group
membership admits only at the exit census (`admit_reserved_group_members`),
which reads the host table. The run loop also gives the same samples to
`backend_daemon_suite_custody.transferable_groups`, which reads the lease's
owner and guardian pids, and those can sit outside the tree. So a poll needs
the rows of one interest set, not of the host:

- the pids the last poll watched, with their resident size;
- the children of each of them, walked until no new pid appears;
- the suite lease's owner and guardian, read once at run start;
- the guard's own lineage to launchd, read once at run start.

Every poll also judges protection (RSS accounting filters protected pids),
and protection walks each tree member's ancestry through the guard. With
the lineage, custody rows give the host table's verdicts; without it, an
unowned member's group was judged protected (measured on a live tree).
Members of a watched group share the root's session, so they are tree rows.
A kill still reads the host table for its protection decision (defense in
depth); only the sampling-failure path reuses the last poll's rows, by design.
That read is observation, so enforcement no longer waits for it; see
[Enforcement independent of observation](#enforcement-independent-of-observation-hf-417).

## How leading tools do it

- systemd and Bazel's Linux sandbox: cgroups (exact membership, kernel memory
  accounting); `PR_SET_CHILD_SUBREAPER` so orphans stay in the tree.
- Windows job objects (`JobObjectBasicProcessIdList`, extended accounting):
  cargo-nextest and Chromium assign children to a job and ask the job.
- macOS has neither; `proc_listpids(PROC_PPID_ONLY | PROC_PGRP_ONLY)` and
  `KERN_PROC_PGRP` filter in the kernel, and kqueue `EVFILT_PROC`
  (`NOTE_FORK`, `NOTE_EXIT`) reports changes without polling.
- psutil walks the full table for `children()`; it is the cost we have now.

## Design

One sampler, `process_model.sample_processes_custody(interest)`, returns the
rows of a `CustodyInterest` (watched `tree_pids`, declared `exact_pids`) only.
The guard's run loop uses it whenever its sampler is the host sampler. Each platform implements it with
kernel filters, chosen from typed platform facts, never by trying and falling
back:

1. Darwin (implemented): `KERN_PROC_PID` for each watched and declared pid,
   `proc_listpids(PROC_PPID_ONLY)` for each watched pid and each new child;
   a child is kept only while its row still names that parent. Resident
   size binds on read as before.
2. Windows: done before this design; the job's member list gives exact
   membership (`sampling_source = "windows_job_members"`).
3. Linux (implemented): `/proc/<pid>/stat` for each watched and declared pid,
   and `/proc/<pid>/task/<tid>/children` for every thread of each watched pid
   (a child belongs to the thread that forked it), walked the same way. A
   kernel without `CONFIG_PROC_CHILDREN` reads every `/proc/<pid>`; that is a
   capability checked once, never a retry.
4. Later, events: kqueue `EVFILT_PROC` on Darwin and pidfd on Linux, so a poll
   with no fork or exit reads only resident sizes.

Full-table `sample_processes()` stays for the callers that need the host view
(custody cleanup, diagnostics, preflight, the exit census), and stays the
oracle in tests. `process_model` owns both entry points and their single
`is_windows_process_model()` predicate. Custody policy, the guard, sentinel and
orphan reaper consume that authority directly; custody and sentinel sampler
wrappers are removed. Windows host and custody snapshots use the same bounded
query, including at timeout. Job-member polling remains a separate kernel
operation. An explicitly supplied sampler stays in use at timeout; cleanup
captures the native host sampler before any caller substitution. Tests replace
the actual authority, preserving private OS views and session-sentinel isolation.

## Costs

Per poll: O(watched pids + their children + declared pids) small syscalls,
each returning a few rows, instead of one host-sized copy and parse. Memory:
the interest set and its rows. No new process or thread.

## Verification

- Live differential (`test_actual_darwin_custody_poll_tracks_the_tree_the_host_table_does`):
  a real child with two grandchildren gives the same tracked set from custody
  rows as from the host table, and `child_pids` matches the table's parents.
- Fake authority (`test_darwin_custody_poll_reads_the_tree_not_the_host_table`):
  a fork since the last poll, a reparented tracked pid, a reused pid, a zombie
  and a lease owner; the host table is never read.
- Run loop (`test_darwin_guard_polls_read_the_tree_not_the_host_table`): the
  receipt names `darwin_custody_rows` and reads under 16 rows per poll; it fails
  when the custody path is disabled.

## Measured (macOS arm64, guarded `/bin/sleep 2`, 3 runs each)

| | main | Darwin custody |
|---|---|---|
| sampler CPU | 0.336-0.357 s, 54-56 polls (6.3 ms/poll) | 0.063-0.077 s, 67-68 polls (1.0 ms/poll) |
| rows per poll (max) | 1,333-1,337 | 8 (the tree and the guard's 7-process lineage) |
| guard observer CPU | 0.44-0.47 s | 0.14-0.16 s |
| guard process user+sys | 0.64-0.67 s | 0.34-0.36 s |

Linux (Lima aarch64 VM, 145 processes, guarded `/bin/sleep 2`, 3 runs each):
sampler CPU 0.235-0.254 s over 69-70 polls before, 0.037-0.041 s over 76-78
polls after; rows per poll 145 before, 6 after; observer CPU 0.29-0.31 s
before, 0.08-0.10 s after.


## Operator cancellation and nested-generation closure

The low-level guard publishes a structured terminal result after cleanup.
Subprocess-compatible callers propagate its actual `guard_signal`, including
when `check=False`: SIGINT raises `KeyboardInterrupt`; other operator signals
raise `SystemExit(128 + signal)`. The exception retains `guarded_result`, including
any secondary closure failure. A child's ordinary exit 130 or 143 does not imply
operator cancellation. Direct serial measurement consumers use the same signal
boundary; explicit suite and proof-owner stop protocols retain their own authority.

A stopped inline guard may have published another child generation during signal
grace. Closing its original process group or old marker does not close that later
child. Final POSIX scratch closure reads active and retired marker generations for
the captured producer PID/birth pairs after the producers stop, and follows exact
recorded child identities whose custody is closed. A live child, unpublished launch,
unreadable record or unavailable producer birth holds closure. This observation
never signals a replacement or infers ownership from a current numeric PID.
Windows continues to use its exact Job completion authority.

`ProcessTreeTracker.closure_identities()` includes current captured identities and
identities retired specifically when a poll proves PID reuse. Multiple births of
the same PID remain distinct. Retired identities never rejoin live admission or
signaling membership. Explicit receiver transfer removes that instance from this
tracker's custody without retaining it as a retired lifetime; the receiver owns
its closure. This keeps an earlier poll from erasing the only link to a former
publisher's late child generation.

The high-level signal check is constant work. Tracking reuse adds one set insertion
only when an observed lifetime retires, with storage bounded by retired lifetimes
within one tracker. Final closure performs at most two directory listings per
traversed generation depth. PID names are filtered before sorting or reading, so
cost is O(directory entries + matching records log matching records) per depth,
and only matching-family payloads are read. No global process scanner, extra
process, signal or per-poll directory read is added. These are developer-process
custody costs; compiled user binaries do not carry them.


## Enforcement independent of observation (HF-417)

Status: POSIX implemented and measured on macOS arm64, 2026-10-21; Linux and
Windows unverified. Ledger: HF-417.

### Problem

The run loop enforced the deadline, memory trips and an owner's cancellation
on the thread that also samples, exports daemon custody and writes evidence.
Any of those can block while the host stalls. On 2026-10-21 a guard with a
300 s deadline (`uv run python3 tools/audit_op_kinds.py --check`) signalled
its child at 2,270 s: its startup report is dated 38 minutes after the
spawn, so it waited inside the running summary's host-wide repro read (every
process's command line) before its loop began. Four landing tests waited out
the same 13:16:49Z-13:54:39Z window.

### How leading supervisors do it

- Bazel's process wrapper runs the child in its own process group, arms a
  timer, and on expiry sends SIGTERM to the group, then SIGKILL after
  `--kill_delay`; nothing else runs on that path.
- GNU `timeout` signals the command from its alarm handler, and `-k` sends
  KILL after a delay.
- Go's `exec.CommandContext` cancels from a goroutine that waits only on the
  context, apart from the goroutines that copy output (`Cmd.Cancel`,
  `Cmd.WaitDelay`).
- systemd enforces `RuntimeMaxSec` and stop timeouts from timers on its event
  loop and kills by cgroup, which needs no process scan.
- CPython's `Popen.send_signal` polls before it signals to narrow PID reuse
  (bpo-38630); Linux `pidfd_send_signal` closes it. Here the reap lock does.

### Design

`OwnedTreeEnforcer` (`tools/memory_guard_core/process_custody.py`) is a
thread that reads no process table and writes no file. It signals through
`owned_root_group_signal` only, the one target that needs no observation to
identify:

- POSIX: the child's own process group. The child leads a new session from
  launch, so only its descendants can join the group, and while its
  `ChildExecutionClock` holds it unreaped, its PID, and so the group it leads,
  cannot name another process. Each signal is sent under the lock the reap
  takes. SIGTERM goes first; SIGKILL follows the grace unless the root has
  been reaped by then, when observation must finish what remains.
- Windows: the guard's Job (unverified here).
- Otherwise (no reserved wait, or a root that does not lead its group): none,
  and the run loop alone enforces, as before.

The enforcer is a backstop. A deadline falls due `ENFORCEMENT_SLACK_SEC`
(2 s) after it passes; a memory trip or a cancellation falls due 2 s after it
arises. The run loop calls `begin(reason)` just before it signals, which
defers to a strike already made or pushes the due time another 2 s out, and
`delivered(reason)` after. A healthy run loop delivered a timeout 0.03-0.58 s
after its deadline at host load 12-120, so the enforcer stays idle and the
loop keeps its evidence, such as the Cargo interruption inventory it takes
before it signals. A loop that resumes after a strike does not signal again
from stale rows; it waits for the reap, and the closure after the loop adds
the observed exit to the enforcer's report.

An owner's cancellation callable may do I/O, so a `StopRequestWatch` polls
it on a thread of its own and latches the first request for the loop and the
enforcer; a second watch hands an operator's signal to the enforcer. The
kill's host-table read now comes before `begin`, so a stall there defers to
the enforcer instead of signalling from stale rows.

Under a suite daemon lease, the export before a signal waits for the
adoption lock without a ceiling (it decides whether a daemon the suite
adopted survives). On the memory-trip, operator-signal, cancellation,
deadline and post-loop paths the enforcer is requested before that export,
and it stays armed until the post-loop path is done, so the root's group is
signalled within the slack however long the export waits. The trade:
descendants in other process groups, which are the daemons that export
decides about, run until the export returns. The
running summary publishes its startup report on the guard thread and writes
its host-wide repro block from a daemon thread (`RunningSummaryPublisher`),
retired before the guard returns so it never replaces the terminal summary.

Residual: memory trips still need a tree sample to be detected, an operator
signal still waits for the guard thread to run Python, and a member that
ignores SIGTERM after the root's reap waits for the closure's observation.

### Costs

One thread per guarded command that has a deadline, a memory trip or a
cancellation callable, plus one for the callable and one for a running
summary; each waits on an event. Interleaved runs of guarded `/bin/sleep 2`
(12 each, load 12-16, 2026-10-21): wall per sample 2.33 ms before, 2.11 ms
after (median); observer CPU 0.163 s and 0.144 s; guard process CPU 0.183 s
and 0.164 s. With a running summary, guard overhead outside the child fell
from 0.092 s to 0.028 s (median), because the host-wide repro read left the
guard thread.

### Verification

`tests/test_memory_guard_tool.py` stalls each read that stood on the
enforcement path and requires the child's own group to be signalled while it
is stalled: the guard thread's samples (deadline, cancellation), the kill's
host-table read (memory trip) and the running summary's repro read
(deadline). Two tests in `tests/test_child_clock_custody.py` bind the signal
to the PID reservation: it signals a live root's group (TERM, then KILL for a
root that ignores TERM) and nothing once the root is reaped.


## Atomic summaries and crash-stage custody

Running, terminal and worker-exit summaries use the shared reconstructible
atomic publication authority. Readers see the previous complete JSON or the
next complete JSON. These observational records add no durability barrier.
Scratch comparisons validate identical lexical paths once per comparison;
directory identity reuses the same `lstat` result for link/reparse classification.
Validation is not cached across locks, callbacks or filesystem mutations.

The existing active-marker sweep captures marker and stage generations before
its complete process snapshot. It joins bounded publication destination keys to
reversible marker-lock names in a batched namespace scan; a key collision holds
every plausible marker and lock owner, including during explicit release. An
indirect file, changed generation, busy lock, live or unreadable custody remains
HOLD. A second names-only scan after the snapshot holds late stages. Orphan lock
retirement considers only pre-snapshot lock names whose producer PID is absent;
fresh launch tokens cannot reuse those lock names.
The stage name is routing metadata, never deletion authority. Under the same
marker lock, the captured physical/content identity and committed generation
must still match before a stage can be removed.

Only an exclusive, validated `guard_starting` publication with no launched child
uses the `guard-launch` purpose. The producer cannot spawn until that publication
returns. A process crash during its first partial write is recoverable through
the existing lock and absent producer PID, without parsing a partial body. A
present PID with unavailable birth remains ambiguous, including PID reuse.
Later writes keep the ordinary `write` purpose and require captured complete
custody; a partial update whose committed marker was lost remains HOLD. Legacy
stages without reconstructible provenance also remain HOLD, with an actionable
preservation report. An operator marker release does not erase unresolved stage
custody. No new sidecar, registry, process signal or age-based deletion is used.

A stage alone admits a sweep. The existing sweep receipt separately records
remaining markers and remaining stages; unchanged HOLD residue does not request
a fresh global process snapshot on every guard exit. Discovery is O(active
namespace entries plus actual candidate relations), with two batched listings,
stage-sized key lookup and captured payload storage;
there is no per-stage rescan of locks or retired history. Publication and
reconciliation retain their pre/post-lock physical checks. These are developer
apparatus costs, with no generated-program overhead.
