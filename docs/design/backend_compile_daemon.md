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
  across a compile. Keys carry the request's configuration digest.
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

### Retirement

- A `{"retire": true}` request unlinks the socket first, so the next build finds no socket and starts a
  fresh daemon. The daemon then writes the wake byte; the accept loop stops only on that byte, so it
  accepts every connection queued before the unlink, waits for every connection thread, and exits.
  Accepted work always finishes.
- Health reports `active_jobs` (jobs admitted and not answered) and `retiring`.
- `tests/molt_diff.py` retires a verified daemon whose RSS exceeds `MOLT_DIFF_DAEMON_MAX_RSS_KB` only
  when the daemon at the socket reports that daemon's pid and no active jobs. It records `retiring` in
  the daemon's identity first; no pruner stops a retiring daemon, and the CLI's start path treats a
  retiring daemon without a socket as absent instead of stopping it. Before this change the pruner sent SIGTERM to any oversized daemon,
  mid-compile; three WASM builds in the 2026-10-15 corpus failed with "no linked module" that way.
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

- HF-219: an idle timeout. Prior art runs the timer from the last completion with no work in flight
  (sccache's timer runs from the last arrival and can abort long compiles); defaults are 10 min
  (sccache), 2 h (Kotlin) and 3 h (Gradle, Bazel), with a short never-used timeout (Kotlin, 60 s). Size it
  from measured reuse intervals.
- Memory admission by declared request budgets, corrected by measured pressure (Linux PSI, cgroup v2),
  instead of the static envelope.
- A client that disconnects does not cancel its compile; the output still reaches the cache.
- The rest of HF-211: the process settings outside the catalog (cache roots, daemon limits) could move
  to typed accessors as well.
