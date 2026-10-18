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
  clippy, 411 s in runtime tests and 190 s in pytest, 1,103 s in all.
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
   workers can share it.
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
accounting and scheduler consumers have focused Python coverage. Corrected
Linux optimizer execution and resource measurements remain required before
acceptance.

## Storage

Capacity includes where state lives. The frontend's source record store
(`toolchain_source_records/source_records.sqlite`) sits under the artifact
root, which on the primary Mac is an external USB volume: 285 MB holding
50,315 records of 5 compiler generations, interleaved. A keyed lookup costs
2.26 ms cold and 0.034 ms warm, and discovery for `import scipy.ndimage` reads
1,086 records, so a cold page cache adds seconds to every cold build and far
more under suite load (that test took 43 s alone and 295 s in its suite).

Small, hot metadata belongs on the fastest local volume with room for it, and
bulk artifacts (Cargo targets, linked outputs) on the large one. The record
store keeps one file per compiler generation, so a build reads only its own
generation, a trim deletes a file, and a cold process can read its generation
sequentially. Volume choice is measured per machine, not a fixed path.

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
   plan command that runs xdist declares `--molt-worker-memory-gb`, its
   measured worker peak times 1.25 (the guard samples about every 2 s and can
   miss a short peak), rounded up to a quarter GiB. Measured units on macOS
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
   [landing queue](landing_queue.md).
4. The host-wide jobserver.
5. Record stores per compiler generation, on the fastest local volume.
6. The same authority on each machine of the Tailscale farm, so a remote cell
   sizes itself on its host.

Target semantics are out of scope: a compiled program's `os.cpu_count()`
reports the machine it runs on, as CPython does.
