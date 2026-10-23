# Host capacity and scheduling

Status: in progress, 2026-10-17. Owner: build, test and landing speed.
Owns HF-27, HF-68, HF-170 and HF-225 with HF-334.

Every machine that runs Molt tooling sizes its own work from what it has.
No tool fixes a worker count.

## Problem

Measured on macOS arm64 (18 CPUs, 128 GiB), 2026-10-17:

- Parallelism is a set of fixed constants that disagree. The proof executor
  runs `executor_max_workers = 4` commands; landing pytest runs on
  `local_pytest_workers = 4`; `tools/resource_pressure.py` assumes 4 Cargo jobs
  and 2 compile slots of 12 GB each; the hash-seed proof runs 3 builds at once;
  `tools/runtime_test_gate_plan.py` defaults to `min(8, cpu_count)` threads.
  About twenty host tools read `os.cpu_count()`, which ignores CPU affinity
  and cgroup quotas. Plan commands that pass `-n auto` get pytest-xdist's
  default, one worker per CPU with no memory bound: on an 8 GB, 8-core M1 that
  is eight workers of up to 1.4 GB each (HF-170) beside their builds.
- There are two schedulers. `tools/proof_executor.py` is a dependency DAG with
  resource classes, cancellation and receipts that record each command's
  duration and peak RSS. Landing gates (`tools/molt_dev.py integrate`) run in
  a second, sequential loop, so their times add up: one landing spent 404 s in
  clippy, 411 s in runtime tests and 190 s in pytest, 1,103 s in all. The
  always gates now run as one executor block (HF-35); the rule, pytest and
  Cargo gates still run in that loop.
- More workers do not shorten a suite whose slowest test is its floor.
  `tests/test_molt_dev.py` and `tests/test_proof_plan.py` take 67 s at
  `-n 4`, 65 s at `-n 8` and 62 s at `-n 12`; one test,
  `test_every_authority_input_mutation_invalidates_receipt`, takes 40 s.

## Prior art

- **Bazel** keeps one local resource model (`--local_resources`, default
  `HOST_CPUS` and a share of `HOST_RAM`). Each action declares a resource set
  (CPU and memory); the scheduler admits actions while the model has room.
  Test `size` sets a default resource set and timeout.
- **Buck2** schedules a DAG against a host resource model and re-estimates
  from observed action costs.
- **[Ninja](https://ninja-build.org/manual.html)** bounds heavy actions with
  pools as well as the global job ceiling; neither permits the other to be
  exceeded. Its generator moves policy decisions ahead of incremental execution.
- **GNU make and Cargo** share CPU across nested tools through the jobserver:
  one token pool, inherited by every child build.
- **[pytest-xdist](https://pytest-xdist.readthedocs.io/en/stable/distribution.html)**
  exposes `pytest_xdist_auto_num_workers` to size `-n auto` from project policy,
  and `--dist worksteal` rebalances
  uneven tests.
- **Rust's `available_parallelism`** honors CPU affinity and Linux cgroup
  quotas; **Python 3.13's `os.process_cpu_count`** honors affinity only.

An independent audit (2026-10-17) found more:

- `molt.dx` read the sysctl `vm.page_inactive_count`, which macOS 26 does not
  have, so every consumer of its probe sized from total memory and ignored
  load; a mock hid it. The guard's Mach `host_statistics64` probe worked.
- No code read a cgroup CPU quota. In a 4-CPU container on a 64-CPU host,
  Python set `RAYON_NUM_THREADS` to 32, the backend daemon opened 4 slots and
  xdist started 64 workers.
- Declared per-unit memory exceeds the measured peaks 4 to 7 times (rustc
  2.35 GB measured against 12 GB declared), and no scheduler reads the peak
  RSS every receipt records.
- `tools/compile_governor.py` has no production caller.

## Design

1. **One capacity authority.** `molt.host_capacity` samples CPUs (affinity,
   then the Linux cgroup CPU quota) and memory (total and live available,
   bounded by the cgroup limit, through the probes `molt.dx` already has). A
   consumer asks for a worker count for a declared unit,
   `workers(cpu=1, memory_bytes=..., headroom_bytes=...)`, never for a fixed
   number. The memory term uses live available memory, so a host already
   loaded by other builds or agents gets fewer workers.
2. **Declared units, not counts.** Configuration names what one unit of work
   needs. `local_pytest_workers` becomes a per-worker memory budget; Cargo jobs
   keep their per-job budget; the proof executor's global ceiling comes from
   the authority. `max_parallel` stays only where commands share mutable
   state (one Cargo target), which is a correctness bound, not capacity.
3. **pytest uses `-n auto` everywhere.** Molt's pytest plugin implements
   `pytest_xdist_auto_num_workers` from the authority, so checked-in commands
   stay identical on every machine and each machine picks its own count.
4. **One scheduler.** Landing gates become typed commands (id, argv, resource
   class, timeout) that the proof executor runs, so the pytest run and the
   Cargo gates overlap, a failure cancels the rest, and every landing leaves a
   receipt with each gate's duration and peak RSS.
5. **Longest first.** The executor orders ready commands by their last
   recorded duration, and a test that alone bounds its suite is split so
   workers can share it. Inside one pytest run, a test marked
   `@pytest.mark.long_pole` (measured 10 s or more in a warm `-n 4` run)
   starts before every unmarked test, and xdist's load scheduler sends at
   most two tests at a time (`molt.pytest_capacity_plugin`). This is
   Graham's longest-processing-time rule: a long test that starts last
   otherwise bounds the run, and xdist's default first step of a quarter of
   each worker's share would hand every pole to one worker. Unmarked tests
   keep their collection order, so a worker still sets up each module's
   fixtures once; an explicit `--maxschedchunk` still wins and a serial run
   keeps its order. The marks are declared, not recorded per run: a CI
   runner, a new lane and the proof queue start with empty pytest caches,
   and each must get the same order. On macOS arm64 at `-n 4`, two warm
   runs of the whole CLI suite (8,404 tests) each way, beside other agents'
   load: the busiest worker ran 11.5% and 13.6% more test time than the mean
   worker before, and 1.4% after; wall time over a quarter of the summed test
   time fell from 1.22 and 1.25 to 1.11 and 1.16 (577 s to 467 s wall in the
   first pair). A replay of xdist's scheduler over the measured durations,
   with the final marks, gives the busiest worker 474 s against 529 s in the
   first warm run and 763 s against 934 s in a new lane's cold first run.
6. **One host-wide token pool for CPU, per-peer slices for memory.**
   Concurrent tools on one host (landings, other agents' builds, nested
   Cargo) take CPU tokens from one GNU make jobserver, so idle peers return
   capacity; Cargo, rustc and Ninja are already clients, and the backend can
   join through the `jobserver` crate already in `Cargo.lock`. Memory cannot be
   a token (a parent holding memory its child waits for deadlocks), so a
   parent divides its memory slice among the units it admits and exports it;
   a child sizes itself against the smaller of its slice and live memory.

### Guard delegation

An inherited tree limit is an allocation ceiling, not a new machine. Keep the
host/cgroup total and live availability distinct from the parent's delegated
tree limit in one typed capacity snapshot. Scheduling uses the bounded view;
guard policy uses the raw host facts for the one host reserve and global
margin, then caps its tree by the inherited ceiling. A serial nested guard
does not allocate a new peer share merely by adding another process wrapper.
Applying the root reserve and tree fraction at every nesting level previously
reduced a 9.54 GiB admitted proof tree to a roughly 0.69 GiB optimizer process
limit on a 12 GiB VM, rejecting actual Binaryen work at about 0.8 GiB.

The invariant follows the [Linux cgroup v2 delegation model](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html#model-of-delegation):
descendants remain inside their ancestor's restriction. Our inference for
serial tooling is that an unchanged allocation remains unchanged across
wrappers. Explicit smaller allocations and live host pressure may tighten it;
nesting depth alone may not. This does not implement the separate peer
allocation and host-wide jobserver work above.

One capacity sample replaces the guard's separate total/available probes.
The snapshot adds a small fixed-size Python value in development tooling; it
removes a repeated host probe and preserves the delegated-limit fact. Guard
exports carry the admitted backend process cap separately from raw availability:
the backend intersects that cap with any explicit availability minus reserve,
and never deducts a reserve from the already admitted cap. Loss of host
telemetry preserves an inherited tree allocation within the hard guard ceiling
and retains the conservative fallback process ceiling; zero available bytes
remain observed pressure. The guest runtime and its memory policy are
unaffected. Known-host nesting, pressure, explicit environment selection, RSS
accounting and scheduler consumers have focused Python coverage. The corrected
Linux/aarch64 Python 3.12 pipeline passes all nine host-boundary/final-TIR/finder
cases and the separate GPU retained-bytes and Falcon package-entry cases with
accepted source custody. The largest observed process tree is 4.14 GiB. These
are scoped correctness and resource observations; the Falcon fixture uses a
stub model. The owning Rust budget/cache controls, other matrix cells and
comparative performance acceptance remain separate obligations.

### Executable-image and lifetime memory

POSIX exit `ru_maxrss` is a process-lifetime high-water mark. On Linux it
survives exec, so a small child can carry a loaded parent's pre-exec image
peak. It is neither current RSS nor an allocation measurement of the new
executable. See [getrusage](https://man7.org/linux/man-pages/man2/getrusage.2.html)
and [Linux exec accounting](https://github.com/torvalds/linux/blob/master/fs/exec.c).
The guard retains this conservative short-lived-spike enforcement and labels
it `process_rusage`; it does not subtract a parent's peak or raise a cap after
failure. Sampled tree peaks and lifetime process peaks remain distinct.

Small-cap protocol and metadata tests run their exact pytest node, with its
real fixtures, in a fresh observer through the existing guarded subprocess
authority. This costs one development-only interpreter and narrow collection
per affected case; it removes dependence on unrelated tests' retained heaps
without changing the subject's limits. The observer must execute exactly one
passing call; a skip cannot stand in for its proof. An inflated-parent control
requires both the raw lifetime trip and the isolated subject's successful exit.
Benchmark executable-image attribution and scheduler cost qualification remain
HF-334/V1-07 obligations; lifetime RSS alone cannot satisfy those claims. The
guest binary pays none of this test-observer cost.

## Storage

Capacity includes where state lives. The frontend's source record store
(`toolchain_source_records/v<schema>/records.sqlite`) sits under the artifact
root, which on the primary Mac is an external USB volume: 285 MB holding
50,315 records of 5 compiler generations, interleaved. A keyed lookup costs
2.26 ms cold and 0.034 ms warm, and discovery for `import scipy.ndimage` reads
1,086 records, so a cold page cache adds seconds to every cold build and far
more under suite load (that test took 43 s alone and 295 s in its suite).

Small, hot metadata belongs on the fastest local volume with room for it, and
bulk artifacts (Cargo targets, linked outputs) on the large one. The canonical
source-record authority instead uses one shared four-kind
SQLite table whose row recency and global byte/age budget own retention; compiler
generation remains read provenance. Short metadata snapshots release the WAL
before eviction work; see
[the record contract](../spec/areas/compiler/SIMPLE_IR_JSON_SCHEMA.md). The
previous per-generation-file proposal is retired: retained scan-key replays
under HF-285 do not establish a persistent-storage bottleneck. Frontend
operations and live observations own connections; a weak identity registry
releases abandoned roots rather than retaining a connection per historical
root. Volume choice still requires measurement per machine.

[SQLite WAL checkpointing](https://sqlite.org/wal.html#checkpointing) requires
readers to release their snapshots before the WAL can reset. Maintenance uses
short page snapshots and the existing inventory count as a finite observation
budget; synchronous NORMAL and automatic checkpointing remain enabled.

## Migration

1. The capacity authority. Done: `molt.host_capacity` owns the CPU count
   (affinity, then the tightest cgroup quota, as Rust does) and the one memory
   probe (Linux meminfo bounded by the cgroup, the Mach VM counters on macOS,
   `GlobalMemoryStatusEx` on Windows); the probes in `molt.dx` and the guard
   are deleted, and every sizing read of `os.cpu_count()` uses it. The guard
   measures the unit: each sample sums the RSS under each xdist worker (found
   from the current-test records the workers publish), and every receipt
   carries `worker_peaks`, the largest worker subtree and the tests with the
   largest peaks. `molt.pytest_capacity_plugin` resolves `-n auto`; every
   plan command that runs xdist declares `pytest_worker_memory_gb` (its argv
   projects it as `--molt-worker-memory-gb`), its measured worker peak times
   1.25 (the guard samples about every 2 s and can miss a short peak), rounded
   up to a quarter GiB. Measured units on macOS
   arm64 run from 0.22 GiB to 4.94 GiB, so no single default fits. The proof
   executor divides the memory beyond the reserve by the maximum simultaneous
   commands allowed by the selected resource classes and the executor ceiling,
   then exports each share as the slice its `-n auto` sizes within. A serialized
   Cargo-only cohort receives one share regardless of its command count. This
   static upper bound takes linear work in the selected commands and resource
   policies; dependency ordering can leave some shares idle. This bound is our
   inference from the already enforced pool/global limits, not a claim that
   Ninja implements these memory slices. It does not claim
   elastic reallocation or a host-wide jobserver. A slice already holds the
   reserve, so nothing inside subtracts it again. One typed memory snapshot owns
   both the live readings and reserve decision.
   Landing cohorts use `-n auto` with the largest envelope owned by their
   selected test paths or nodes. One prefix index derives those envelopes from
   the existing command argv; directories include descendant envelopes, and
   uncovered targets retain the shared ad hoc default. Selectors keep their
   file envelope until a narrower measurement exists. Index construction and
   lookup take work proportional to the declared and requested path/node
   components, with no filesystem scan or pytest collection. The plugin and
   projection share the default in the lightweight capacity authority; parsing
   policy does not import pytest. A bounded macOS arm64 observation over 161
   commands and 827 index nodes measures a 3.85 ms median construction and
   0.30 ms warmed selection for 33 gates; the first cold selection takes
   15.63 ms including pattern compilation. Python-traced peak index allocation
   is 186,330 bytes. This is accepted development-side metadata cost for
   preserving declared allocations, not an emitted-program cost or a general
   wall-clock performance claim. CI no longer exports a runner-wide worker
   count. On the 7 GB macOS runner the
   suites with units above 1.1 GiB now run one worker; their CI budgets at
   one worker are unverified.
2. Split the tests that alone bound a suite's wall time.
3. Landing gates through the proof executor, with declared timeouts and
   resource classes, longest first; the first step of the
   [landing queue](landing_queue.md). The always gates run as one block
   in the `repository-policy` class, bounded by `executor_max_workers` and
   one GiB per gate from this authority, and leave a receipt. Done
   (2026-10-22): the read-only checks run first, so a failing check stops
   every build before it starts. The Cargo and pytest gates then share one
   executor block: every pytest session is admitted in `python-tests`, at
   most two at once, each sizing `-n auto` within the exported memory slice;
   other Cargo gates run in `compiler-build-resource`; and every gate that
   may build with Cargo, a pytest session included, waits for the previous
   one in declared order, because they share one target. A pytest session
   that does not build overlaps the Cargo gates instead of waiting for them.
   Declared timeouts and longest-first order remain.
4. The host-wide jobserver.
5. Qualify the canonical shared source-record lifetime and retention costs;
   measure volume placement before changing it.
6. The same authority on each machine of the Tailscale farm, so a remote cell
   sizes itself on its host.

Target semantics are out of scope: a compiled program's `os.cpu_count()`
reports the machine it runs on, as CPython does.

### Fresh worker processes

Compiler, source-audit, environment-audit and differential worker pools share
`molt.compiler_process.worker_context`. It explicitly selects `spawn` before
starting work. No pool forks the calling compiler's heap or locks, and every
new pool receives the current parent environment. This follows
[CPython's process-context guidance](https://docs.python.org/3.12/library/multiprocessing.html#contexts-and-start-methods)
while deliberately retaining spawn on POSIX: a reused forkserver keeps the
server's original environment, which is stale when an embedded compiler changes
cache, source, session or toolchain selections between invocations. Rebuilding
that ambient-state protocol would add another authority. Spawn starts each
interpreter independently; its startup cost must be measured against the
inherited-heap amplification it removes. The policy adds no guest work.
Differential stop
state comes from the selected context, and orphan detection observes the
creating parent's sentinel rather than the server's OS PID. Explicit fork
ownership tests retain their separate semantics.

The compiler's fast exit joins the already-loaded spawn resource tracker after
thread and atexit callbacks have joined workers and released queues. This
executes the ownership work in
[CPython's tracker destructor](https://github.com/python/cpython/blob/v3.12.15/Lib/multiprocessing/resource_tracker.py#L73-L112),
which heap-free exit otherwise skips;
[multiprocessing's atexit callback](https://github.com/python/cpython/blob/v3.12.15/Lib/multiprocessing/util.py#L312-L354)
joins registered workers but does not join this helper. The canonical spawn
lane borrows only the parent's tracker descriptor in workers; CPython's own
stop operation leaves that borrowed owner intact. This is not a custody claim
for arbitrary raw-fork callers, whose tracker state differs across CPython
versions. Forkserver is not a compiler producer, and the descriptor sharer is
a daemon thread with different finalization semantics; neither gains another
cleanup path here.

Commands without a tracker perform one loaded-module lookup, without importing
multiprocessing or starting a process. Commands with an owned tracker close
its existing pipe and join it once; this preserves fast heap reclamation and
eliminates reliance on a guard terminating the surviving helper. The kernel
wait oracle checks retirement before root exit, while real spawned workers
check that their borrowed descriptor and parent helper remain usable. These
are compiler/development costs only; emitted binaries have no Python tracker.
