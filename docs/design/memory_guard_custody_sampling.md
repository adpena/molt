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
A kill still re-reads the host table before it signals (defense in depth);
only the sampling-failure path reuses the last poll's rows, by design.

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
