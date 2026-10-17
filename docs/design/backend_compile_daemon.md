# Backend compile daemon

Status: landed design (2026-10-15). The code is the authority:
`runtime/molt-backend/src/backend_process/daemon/` (server), `runtime/molt-ir/src/backend_environment.rs`
(request settings), `src/molt/cli/backend_execution.py` (client), `src/molt/backend_daemon_custody.py`
(identity custody), `tests/molt_diff.py` (pruning).

## Problem

`molt build` sends backend work to a long-lived `molt-backend --daemon` on a Unix socket, so later
builds reuse warm caches. The daemon served one connection at a time. In a 12-job differential run on
2026-10-15, WASM builds spent a mean 111 s in `backend_daemon_compile`, almost all of it waiting in the
accept queue; one isolated build spends 9 s there. The daemon could not simply serve connections in
parallel: each request wrote its settings into the process environment with `std::env::set_var`
(HF-211), and per-build data such as `MOLT_ENTRY_MODULE` and `MOLT_STDLIB_MODULE_SYMBOLS` travelled
the same way. Two concurrent compiles would read each other's values.

## Design

### Request-scoped settings

- `src/molt/backend_environment.json` is the one membership authority. `runtime/molt-ir/build.rs`
  generates a closed `BackendEnv` enum (variants keep the variable's spelling) and a closed
  `BackendEnvGroup`, so a new key or group fails every exhaustive consumer at compile time.
- Compiler crates read settings only through `backend_environment::{var, var_os, is_set}`, which keep
  `std::env`'s contracts. A one-shot backend reads its process environment. The daemon calls
  `serve_requests()` at startup: it strips every request-scoped variable from its process environment,
  and afterwards a read outside a request scope panics.
- Each request's `env` object becomes a typed `BackendEnvironment` (`from_request` rejects unknown
  keys, non-strings and the build-identity group). The dispatcher installs it on the compiling thread
  (`enter`). Work that runs on other threads captures the scope (`carry`) and enters it per item:
  `molt-passes/src/tir/parallel.rs`, `molt-tir/src/passes/pipeline.rs`, the TIR pool in
  `molt-tir/src/tir/pipeline_cache.rs`, and Cranelift's deferred flush. Native batch worker processes
  receive exactly the request's settings (`configure_child`).
- Build identity (`MOLT_BACKEND_COMPILER_FINGERPRINT`) stays process-scoped: the config digest that
  names the daemon's socket already fixes it.
- Each compiler crate's `clippy.toml` disallows `std::env::var` and `std::env::var_os`. A
  process-level read outside the catalog states its reason with `#[expect(clippy::disallowed_methods)]`;
  test builds may read the process environment.

### Concurrency

- The accept loop polls the listener and a `UnixStream::pair` wake socket, and serves each connection
  on its own thread with the 64 MiB stack compiler workers get (`COMPILE_THREAD_STACK_BYTES`); a refused
  thread drops its connection, and accept errors back off instead of spinning. At most one compile per
  core runs. A job takes its compile slot only after the cache probes and the single-flight wait miss,
  so cache hits and followers never hold one.
- The in-memory output cache (`DaemonCache`) holds its LRU lock only to look up or insert bytes, never
  across a compile. Keys carry the request's configuration namespace and whole typed job target.
- Single flight: concurrent requests for one cache key compile once. The first caller leads; followers
  wait, then probe the cache again. A leader that caches nothing (an error, or output above the memory
  budget) releases its followers, and each then compiles for itself.
- The shared native stdlib archive has one producer per cache entry, across processes and threads.
  A build first tries lock-free reuse; on a miss it takes the entry's producer stripe
  (`<cache>/locks/shared-stdlib-producer.<key[..2]>.lock`, taken before the publication stripe) and
  checks again, so one build compiles the stdlib and the rest hydrate its archive. On 2026-10-15 the
  first wave of 10 concurrent native builds each compiled the stdlib for 139-214 s. A build that cannot
  reuse an archive (no entry module, or diagnostics requested) compiles without waiting.
- All compiler parallelism (TIR optimization, the SimpleIR pass pipeline, Cranelift's deferred
  codegen) runs on `compile_thread_pool`: exact-width pools shared by concurrent compiles, at most four
  widths, least recently used first out. A request's `RAYON_NUM_THREADS` sizes its pool, and the daemon
  keeps one thread per planned worker instead of one pool per compile.
- A panicking compile fails only its job: `compile_contained_job` turns the panic into an error reply.
- Memory: on Linux the backend installs a soft `RLIMIT_DATA` from its build's budget and keeps the
  inherited hard limit. The daemon widens the soft limit to `compile slots x budget`, the envelope of the
  same number of one-shot backends; each compile still plans within its own request's budget.

### Cache identity and job transitions

The cache and single-flight table use the same typed identity: the caller's
source key, configuration namespace, complete `DaemonJobTarget`, and admitted
semantic environment. Native
identity includes both target triple and object/archive kind. WASM identity
includes relocation mode and all three layout overrides, retaining absent
versus explicit zero. Structural equality and hashing derive from the existing
closed target/options types; adding an option therefore changes identity
without maintaining a second field list or encoding.

Schema 2 of `src/molt/backend_environment.json` owns request transport,
semantic target applicability, raw/presence projection, native-worker capture,
diagnostics bypass and reuse-only constraints. Both Python and the generated
Rust enums consume these roles. Resource budgets, observation destinations and
optional cache publication paths are transported without fragmenting artifact
identity; diagnostics force execution. A stdlib output path contributes presence,
while entry module, module symbols and expected reuse constraints contribute
actual values. The core frontend variant excludes reuse constraints derived
from that variant, avoiding recursive stdlib keys.

Cache setup resolves profile defaults, selected entry/stdlib context and the
admitted WASM runtime layout before computing keys. It carries that environment
and layout to dispatch; daemon and one-shot execution apply the same selected
partition contract. Later runtime binding checks still reject changed files.
Native worker schema 2 carries typed keys with raw strings/null so existing
failure replay manifests remain usable.

The module and function keys share one immutable target/namespace/environment context.
Request strings move into the keys; the LRU and single-flight
retain shared handles rather than formatting and copying composite strings.
Equal module/function source keys share a handle. A bypassed cache or an empty
source key creates no key or context. Key preparation allocates one context and up to two keys,
plus a native triple clone when present. An eligible keyed request lazily computes
one typed 32-byte SHA-256 identity per selected target; subsequent jobs copy that
identity. It hashes borrowed admitted values, with no raw projection allocation.
Unkeyed and bypassed jobs need no digest. Canonical catalog order, explicit
target/domain and tagged length-delimited names/raw values/presence markers
preserve target, absence/empty and relocation distinctions. Native worker replay
continues to carry raw values independently. Process-only consumers stream
fallible environment reads without an intermediate map.

The existing `molt_ir::content_digest::CompilationCacheKey` owns framing for both
this identity and persistent TIR keys; its relocation preserves persistent bytes.
This follows [Go's fixed-width compiler cache hashes](https://go.dev/src/cmd/go/internal/cache/hash.go)
and [Bazel's declared-input environment identity](https://bazel.build/remote/caching).
Equality assumes SHA-256 collision resistance, as other content-addressed compiler
caches do; it does not authenticate a caller's IR claim. Cold preparation pays
one linear SHA-256 pass over selected bytes plus catalog/setting lookup. Warm
map hashing compares the fixed identity, rather than traversing or retaining
raw semantic bytes such as stdlib manifests. Source key, namespace and native
triple hashing still scale with their lengths. This adds no file read, syscall,
serialization, process spawn or guest operation. These costs belong to compiler
requests, never emitted binaries.

A Darwin arm64 development-profile measurement uses the actual process accessor
and derived identity hashing, with nine rounds per setting size. A 64 KiB raw
projection costs 15.05 us per hash; the fixed identity costs about 9 ns. Cold
process capture rises from 6.41 to 29.77 us for 64 KiB and from 2.89 to 3.92 us
for one byte. This named trade buys bounded residency and amortizes after two
large-settings hashes in this aperture. It is not a scoped-request or product
latency result. Optimizing the existing `sha2` dependency at O1 for development
reduces the initially measured 234.20 us large cold capture to 29.77 us; release
profiles retain their normal optimization. No end-to-end speedup is claimed.

### Resident ownership and eviction

The daemon and TIR memory tier share `molt_passes::resident_cache::ResidentCache`.
It uses the already pinned [hashlink ordered map](https://docs.rs/hashlink/0.12.2/hashlink/linked_hash_map/struct.LinkedHashMap.html)
with Rust's randomized hasher: one lookup promotes a live node in place, and
oldest removal drops its key/value owners immediately. Both custom heaps and
logical clocks are deleted. Warm hits allocate nothing and retain no historical
generations. Expected lookup/promotion is constant work after key hashing;
source/namespace/triple hashing still scales with their lengths.
An insertion that evicts E entries does O(E) map operations. Sparse storage is
shrunk only on mutation when table capacity exceeds four times live entries;
that amortized linear reclamation never runs on a hit.

Limits measure charged retained ownership, rather than payload length alone.
Each entry charges its inline key/value/charge storage and four pointer words
for links/table slots. The daemon additionally charges the complete source key,
namespace, native triple, typed context (including the inline semantic digest),
output allocation and Arc headers. TIR charges its hash, precomputed quarantine
path capacity, payload and Arc headers. Raw semantic settings are not retained
or charged by resident entries. Metadata accounting inspects field
lengths/capacities on insertion; it
performs no byte scan, serialization, I/O or process spawn. Equal identities
with different allocation capacity remain semantically equal. A single oversized
entry is refused before it can evict unrelated work; replacement retires the old
value even when the new one cannot be retained. Zero-sized payloads still pay
for metadata; byte arithmetic cannot overflow admission.

Shared contexts/outputs are conservatively charged per entry. This can retain
fewer artifacts than unique-allocation accounting, in exchange for avoiding a
second ownership registry, pointer-identity bookkeeping and synchronization on
every cache operation. Artifact buffers remain shared, not copied. The budget
is explicitly an ownership charge: allocator headers/slack, table capacity
rounding, active compiles, flight waiters and outputs retained by callers are
not process RSS. The mutation reclamation rule bounds historical table/free-node
capacity relative to live entries. Health `cache_bytes` and TIR
`resident_bytes` report that same charge; their configured limits use those
units. Full process/resource acceptance remains V1-07.

The ownership weighers destructure the closed key/context/target types without
`..`; adding a field fails the weigher until its allocation policy is explicit.
The semantic identity has fixed inline storage and no transitive allocations.
Retention accounting never participates in structural identity.

A 2026-10-17 Darwin arm64 mechanism comparison compiles the frozen 79b4892ec
heap owner and this owner into one optimized executable with identical surrogate
string keys: 128 resident entries, 200,000 warm hits, nine alternating rounds.
Median loop time falls from 15.42 ms to 7.19 ms. This isolates order maintenance
and lookup overhead; it does not measure full request context hashing, frontend
compilation, product startup or release performance. Real TIR warm-hit controls
retain zero allocations for 10,000 shared 1 MiB hits, and native/WASM daemon
producer/replay controls exercise the typed request boundary. Independent tiny
payload controls reject metadata above budget, and weak handles prove immediate
release of replaced/evicted allocations. V1-07 still owns full acceptance.

This follows the explicit resource-limit policy of [LLVM cache pruning](https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/Support/CachePruning.h)
while preserving the existing authenticated Go-style disk store. Disk integrity,
quarantine, trimming and request single-flight keep their existing owners; no
cache policy is added to the emitted program or runtime dependency graph.

The existing job state machine remains:

```text
target admission -> cache probe -> hit: publish/sync output -> reply
                         |
                         +-> probe-only miss: needs_ir -> reply
                         |
                         +-> single-flight -> re-probe -> compile slot
                                                           |
                                              complete IR -> compile
                                                           |
                                               publish -> cache -> reply
```

A different typed target cannot hit an entry or wait on its flight, even when
the caller supplies equal source/configuration keys. Cache-only operations do
not open unused IR leases. Output paths and job IDs select delivery, so changing
them does not change artifact identity. Diagnostics and native shared-stdlib
admission keep their existing checks before reuse.

This follows [sccache's inclusion of compiler arguments and architecture in
cache identity](https://github.com/mozilla/sccache/blob/main/docs/Caching.md)
and [Bazel's explicit action inputs and command](https://bazel.build/remote/caching).
Molt's in-process cache keeps structural keys instead of introducing a second
serialized target schema. The configuration namespace and source key remain
caller-provided. Actual admitted semantic settings now bind cache and flight identity even
when a caller repeats its configuration claim. Binding consumed IR bytes to
caller source claims remains open under V1-01; target/environment isolation
does not establish authenticated compiler-input or release provenance.

### Retirement

- A `{"retire": true}` request, or `MOLT_BACKEND_DAEMON_IDLE_TIMEOUT` seconds
  without a live connection, starts retirement. The default timeout is 1800
  seconds; zero disables it. The idle clock starts at daemon creation and resets
  when the last connection finishes. A long compile cannot consume the next idle
  interval. The existing listener/wake poll owns the timer; no timer thread or
  repeated process scan is added. Interrupted polls retain their original
  monotonic budget, and positive fractional milliseconds round up rather than
  busy-polling. Disabled idle timeouts read no timer clock.
- Retirement unlinks the socket before writing the wake byte. The accept loop
  stops only on that byte, drains the accepted backlog, waits for every
  connection thread, and exits. Accepted work finishes. Before unlinking, the
  daemon compares the path's device/inode with the socket it bound. Cooperating
  launchers serialize startup and preserve a verified live socket; replacement
  starts after unlink. This preflight identity check is not an atomic defense
  against arbitrary external pathname replacement or concurrent direct launches.
- Health reports `active_jobs` (jobs admitted and not answered) and `retiring`.
- `tests/molt_diff.py` retires a verified daemon whose RSS exceeds `MOLT_DIFF_DAEMON_MAX_RSS_KB` only
  when the observed process generation matches its identity and the responder
  reports that pid with no active jobs. The daemon owns its retiring state;
  Python writes no parallel retirement marker. The harness never signals a
  daemon based on age, missing health or a missing socket. A silent/busy daemon
  remains under its own lifecycle and explicit owner cancellation. Disabled RSS
  maintenance scans no processes or identity files, and ordinary small daemons
  trigger no identity-file scan. Before this change the pruner sent SIGTERM to
  oversized daemons mid-compile; three WASM builds in the 2026-10-15 corpus failed
  with "no linked module" that way.
- A verified daemon with no socket may be draining accepted work. The launcher
  starts its successor without stopping it. Missing readiness alone does not
  replace a verified live daemon with a socket.
- The CLI orphan sweep removes a dead retired daemon's identity but leaves its socket path, which now
  belongs to the successor.

## Prior art (primary sources, read 2026-10-15)

| Concern | Prior art | Molt |
|---|---|---|
| Per-request settings | sccache sends `Compile { exe, cwd, args, env_vars }` per request and applies them only to the compiler child ([protocol.rs](https://github.com/mozilla/sccache/blob/main/src/protocol.rs#L109-L118), [compiler.rs](https://github.com/mozilla/sccache/blob/main/src/compiler/compiler.rs#L217)). Bazel's `WorkerKey` includes the environment; multiplex workers get arguments per `WorkRequest` ([WorkerKey.java](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/worker/WorkerKey.java#L37)). Gradle mutates the daemon's environment per build, which is safe only because it runs one build at a time ([ApplyClientEnvironmentVariables.java](https://github.com/gradle/gradle/blob/master/platforms/core-runtime/launcher/src/main/java/org/gradle/launcher/daemon/server/exec/ApplyClientEnvironmentVariables.java)). | Typed per-request scope; the process environment is never written after startup. |
| Context into worker threads | rustc's `rustc_thread_pool` captures the creator's thread-local context in every job and sets it before the job runs ([job.rs](https://github.com/rust-lang/rust/blob/main/compiler/rustc_thread_pool/src/job.rs#L84-L130)). | `carry()` captured on the request thread, entered per parallel item. |
| CPU bound across concurrent compiles | The make/Cargo jobserver bounds total threads; rustc workers hold a token only while they work ([sleep/mod.rs](https://github.com/rust-lang/rust/blob/main/compiler/rustc_thread_pool/src/sleep/mod.rs#L245-L254)). | Compile slots per core and one shared TIR pool per thread count. |
| Duplicate concurrent work | sccache, Bazel workers, Gradle and Kotlin daemons do not coalesce identical concurrent compiles; Go's `singleflight` is the reference pattern. | Single flight per cache key. |
| Memory | Bazel admits actions by declared resources and evicts idle workers first under `--experimental_total_worker_memory_limit_mb` ([WorkerLifecycleManager.java](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/worker/WorkerLifecycleManager.java#L152)); Gradle expires a daemon gracefully after the current build under heap pressure ([HealthExpirationStrategy.java](https://github.com/gradle/gradle/blob/master/platforms/core-runtime/launcher/src/main/java/org/gradle/launcher/daemon/server/health/HealthExpirationStrategy.java#L106-L120)); Buck2 freezes or retries actions under cgroup v2 PSI pressure but never the oldest one ([scheduler.rs](https://github.com/facebook/buck2/blob/main/app/buck2_resource_control/src/scheduler.rs#L173-L200)). | Envelope of `slots x budget`; retire only idle daemons; retirement drains. |
| Panics | sccache runs each compile under `catch_unwind` and replies with an error. | `compile_contained_job`. |
| Replacement | nginx keeps the old master draining (`.oldbin`); Envoy drains over `--drain-time-s`. Unlinking a Unix socket leaves the bound socket usable and new `connect()` calls fail with `ENOENT` ([unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html)). | Unlink, drain the backlog, wait, exit. |

## Open work

- Qualify the idle policy on supported daemon hosts and measure reuse intervals,
  memory retention and cold rewarming costs before claiming an optimal default.
- Memory admission by declared request budgets, corrected by measured pressure (Linux PSI, cgroup v2),
  instead of the static envelope.
- A client that disconnects does not cancel its compile; the output still reaches the cache.
- The rest of HF-211: the process settings outside the catalog (cache roots, daemon limits) could move
  to typed accessors as well.
