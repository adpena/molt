<!-- 100-year concurrency architecture (GIL / free-threaded / subinterpreter). Principal-architect synthesis of read-only research workflow wq6q37loo, 2026-06-27. -->

Verification complete. The reports are accurate on every load-bearing point, with one correction I'll carry: `IsolationClass`/`SchedulerDomain` are **designed-only, not implemented** (grep-confirmed zero matches) — the escape *substrate* (`EscapeState`) is live, but the isolation *projection* is greenfield. I'll write the architecture with that distinction precise.

---

# molt Concurrency Architecture — Foundation Design Doc (100-Year Plan)

**Status:** Principal-architect decision packet. Recommended defaults are binding unless overridden.
**Scope:** The unified concurrency model for molt — execution, RC discipline, scheduler, and the GIL/free-threaded/subinterpreter mode axis — optimized for extreme long-horizon performance AND future-proofing.
**Inputs:** Report A (current-state audit), Report B (CPython semantics/trajectory), Report C (SOTA design-space), plus direct source verification (2026-06-27).

---

## 0. The strategic thesis (one line, then the packet)

> **molt wins concurrency not with a faster runtime but with a richer representation.** Compile-time ownership/escape facts make data races *unexpressible* and let the compiler emit **non-atomic (or zero) reference-counting on the proven-confined common path**, reserving atomic/biased RC and per-object locks for the proven-*shared* minority — **strictly less work than CPython 3.13t's biased-everything model.** This is molt's structural edge, and it is the reason free-threading can be made *fast* here in a way CPython cannot match.

Everything below serves that thesis. The decisive design rule, repeated throughout: **mode is a compile-time FACT, never a runtime flag** — so the default fast path emits *none* of the machinery for modes it doesn't use.

---

## 1. (Q1) Are molt's CURRENT GIL/async/threading/socket implementations conducive to extreme performance?

### VERDICT: **NO for multicore CPU-bound; YES for single-thread and I/O/async-bound. The architecture is CPython-bounded — a hard ceiling, not a tuning gap.**

This is a two-part verdict because the runtime is genuinely well-engineered *within* the model it chose, and that model is the problem.

**Evidence FOR (single-thread + I/O is well-tuned):**
- The GIL has a real single-thread fast path that skips the mutex entirely when `GIL_THREAD_COUNT <= 1` — verified at `concurrency/gil.rs:209-230`. On wasm32 the whole `GilGuard`/`PyToken` is a zero-cost no-op (`gil.rs:75-154`).
- Async releases the GIL across blocking waits via `GilReleaseGuard` (`async_rt/scheduler.rs:3561,3602,3618`) — CPython-faithful, so I/O-bound async scales correctly.
- Refcount elision already exists: the `contains_refs`/`_nogil` accessor optimizations cut atomic traffic on the hot path (Report A §5.3).
- Sockets/selectors are full CPython sources bound to Rust intrinsics — API-correct (Report A §2).

**Evidence AGAINST (the ceiling — this is the decisive part):**
1. **One process-global GIL serializes everything.** `static PREINIT_GIL: Mutex<()>` at `concurrency/gil.rs:177` — verified. Every refcount op, every container mutation, every allocation, every task poll runs under it. **True multicore Python parallelism is impossible today.** This is CPython's exact ceiling.
2. **Unconditional atomic-RC tax on native, paid for nothing.** `MoltRefCount` is `AtomicU32` on native (`object/refcount.rs:16-19`) — and the doc-comment (verified, `refcount.rs:3-5`) explicitly says this is for *signal-handler/C-ABI safety*, **not** parallelism. So every `inc_ref`/`dec_ref` is an atomic RMW *while the GIL already serializes* — pure overhead. (5-15 ns/op per Report C's cost model vs ~2 ns for CPython's inlined non-atomic.)
3. **The `RuntimeState` mutex farm is a latent contention cliff.** Verified: `runtime_state.rs:274-376` is ~50+ fields, nearly every one its own `Mutex<HashMap>` (`asyncio_*`, `task_*`, `weakrefs`, `thread_tasks`…). Uncontended under the default single-thread loop, but every task transition takes several lock-acquire + HashMap lookups — a constant-factor tax and a contention cliff the instant `MOLT_ASYNC_THREADS>0` or multiple `threading` threads run.
4. **Three overlapping threading mechanisms, no shared back-pressure:** the `isolates.rs` `threading` path, the *separate* `async_rt/threads.rs` pool, and the (default-off) work-stealing executor (`scheduler.rs:2551`). Duplicated state.

**Conclusion:** molt is at the CPython performance frontier and well-tuned within it — but it has *adopted CPython's ceiling wholesale*. For a perf-contract language with a 100-year horizon, that ceiling is unacceptable. The good news (Q3): molt's compiler has the unique machinery to break it.

---

## 2. (Q2) Are they conducive to EASILY supporting experimental/future features (PEP 703 / 684 / 734)?

### VERDICT: **NO — there is no mode dimension at all. But three real, verified seams cut the cost of adding one. Best description: "a GIL runtime with isolation scaffolding started but not wired."**

**Evidence AGAINST (low conduciveness as-is):**
1. **No mode flag/cfg anywhere.** The only GIL-related Cargo feature is `molt_debug_gil` (assertion strictness, not a mode). No `free-threaded`/`Py_GIL_DISABLED`/`subinterpreter` cfg exists — grep-confirmed in Report A and corroborated by my own search (zero hits for `SchedulerDomain`/`IsolationClass` in `molt-passes`).
2. **The GIL is process-global, not per-interpreter.** `PREINIT_GIL` is one static; it is **not** a field on `RuntimeState` (I verified the full struct at `runtime_state.rs:274-376` — no `gil` field). PEP 684's per-interpreter GIL is therefore *not expressible* without reworking the GIL itself. Even "isolated" threads share this one mutex.
3. **The started free-threading infrastructure is DEAD CODE.** Verified by reading `object/gil.rs:1-59`: it is titled "GIL removal infrastructure — Phase 1", defines `ObjectLock` (per-container mutex), `GIL_RELEASED`/`is_gil_released()`/`release_gil()`, and `gil_check()`. None referenced outside the file (Report A grep-confirmed). Per-object locking was scaffolded and abandoned.
4. **Non-atomic dec_ref/resurrection RMWs would race under free-threading.** `dec_ref` does a non-atomic `load(Acquire)` underflow check then a separate `fetch_sub`; resurrection does `load`-then-conditional-`fetch_sub` (`object/mod.rs:2030-2038,2235-2253`) — not single RMWs (Report A §3).
5. **Container mutations guarded ONLY by the GIL.** `dict_set_in_place` (`dict_set_tables.rs:2146`) and even the `_nogil` list setter (`specialized_list.rs:245-251`) rely on `gil_assert()`. `_nogil` means "no refcount work," **not** "free-threaded."
6. **Subinterpreter Python surface is a stub.** `_interpreters` raises `RuntimeError("not fully lowered yet…")` (`stdlib/_interpreters.py:14-17`); `_interpchannels`/`_interpqueues` are top-level stubs.

**Evidence FOR (the seams that de-risk the future — these are real and verified):**
1. **TLS-overridable `RuntimeState`** (`runtime_state.rs:599-618`): TLS takes priority over the global singleton, and this *already powers* a working shared-vs-isolated thread split (`isolates.rs`: `thread_main_shared` points child TLS at parent's state; `thread_main` gives a fresh state + `molt_isolate_bootstrap()`). This is exactly the per-interpreter-state substrate PEP 684/734 need.
2. **Atomic refcounts already default on native** — the single hardest free-threading prerequisite is partially met (though see Q3: we want this *fact-gated*, not unconditional).
3. **`scheduler: OnceLock<MoltScheduler>` is already a field on `RuntimeState`** (verified `runtime_state.rs:297`) — so a per-interpreter scheduler is partially seeded; only the GIL needs to follow it onto the struct.
4. **The escape-analysis substrate is live** (`molt-passes/.../escape_analysis.rs`): `EscapeState` lattice (`NoEscape`/`ArgEscape`/`GlobalEscape`) already drives stack-promotion and RC elision and shares a generated authority (`op_kinds.toml`) with `alias_analysis.rs`. **This is the substrate the entire free-threading-perf plan rides on** — and it already exists.

**Critical correction to the inputs:** Report C states the `IsolationClass` keystone's "substrate is built." Precisely: the **escape substrate is built; the isolation *projection* (`IsolationClass`, `SchedulerDomain`, `isolation_class.rs`) is designed-only and does not exist in code** (I grep-confirmed zero matches). This matters for the roadmap — it is greenfield work *on a real foundation*, not wiring of existing parts.

**Conclusion:** Extensible *in principle* via four genuine seams, *committed to single-GIL* in practice. The future is reachable without a rewrite, but the GIL itself, every `gil_assert`-guarded mutation, the dec_ref RMWs, and the `_interpreters` surface must change.

---

## 3. (Q3) THE ARCHITECTURE — resolving the central fork

### 3.0 The decision

**RESOLVED: Neither pure GIL-emulation nor pure free-threaded-by-design. Ship a LAYERED model with a single unified primitive, where the mode is a compile-time fact and free-threading is the apex tier — made fast by escape-analysis-driven RC atomicity selection.**

This is decisive, not a hedge. "GIL-emulation vs free-threaded-by-design" is a false binary for an AOT language:
- **Pure free-threaded-by-design is wrong** because it taxes the single-threaded majority (CPython 3.13t pays 5-8%; PyPy's STM paid 25-40% and died for it — Report C). Most Python is single-thread, async-single-thread, or process-parallel. A perf-contract language must not make them all pay.
- **Pure GIL-emulation is wrong** because it forecloses multicore forever and is what Q1 indicts.

The resolution is the **tier ladder**, selected by a compile-time `SchedulerDomain` fact:

```
SchedulerDomain ∈ { GilSerial, Isolate(id), FreeThreaded, DataParallel }
                    └ default ┘  └ PEP 734 ┘  └ PEP 703 ┘  └ @par/OpenMP ┘
```

Because the domain is a *fact* (monomorphized once), a `GilSerial` build **emits plain non-atomic RC and no per-object locks — the free-threaded machinery is not behind a runtime `if`, it is not emitted at all.** This is the answer to "the fast path must not pay for unused modes."

### 3.1 Can escape analysis elide atomic-RC overhead to make free-threading FAST? — YES. This is the structural edge.

**The assessment the operator asked for, decisively: YES, and it is molt's single biggest structural advantage over CPython. The mechanism is sound; precision is the only risk, and it is trackable.**

The argument from doing-less-work:

| Runtime | RC tax per dynamic op |
|---|---|
| **CPython 3.13t** | biased RC on **every** op (owner-tid check + local write, or atomic shared-counter add) — it *cannot* prove thread-locality statically, so it pays the check at runtime, always |
| **molt FreeThreaded** | **non-atomic** (or **zero**) on the proven-confined majority; **biased/atomic only on the proven-shared minority** |

CPython pays biased RC on *all* ops because it discovers ownership dynamically. An AOT compiler with escape analysis discovers it *statically* and emits the cheapest correct op per object. **Strictly less work ⇒ lower tax** — provided escape analysis is precise. The per-op cost ladder (Report C, `rc_gc_redesign.md`), confirmed against the live `EscapeState` lattice:

| Variant | Cost | vs CPython |
|---|---|---|
| today: out-of-line call + atomic RMW | 5-15 ns | **loses** |
| + non-atomic (confined/GIL) | 4-8 ns | ~5× cheaper memory op (bigger on ARM: `ldaxr/stlxr`→plain add) |
| + inlined (AOT folds tag/immortal test) | ~2 ns | **at/below CPython** |
| + borrowed/cursor (Perceus) | **0** (no op emitted) | — |
| + immortal / raw-lane | **0** | — |

The compiler's job is to push ops to the bottom rows. This is why molt's *single-thread* FreeThreaded overhead can fall **below** CPython-3.13t's 5-8%: the tax multiplies a strictly smaller op count.

**Why the substrate is real, not aspirational:** `escape_analysis.rs` already computes `EscapeState`, already elides IncRef/DecRef for `NoEscape`, and already shares the `op_kinds.toml` authority with `alias_analysis.rs` and the ownership lattice. `IsolationClass` is a **projection** of this — it consumes the existing escape fact and adds one thread axis:

```
IsolationClass ∈ { ThreadConfined, Sharable, Shared, Immortal }   // 2-bit, rides the value's Repr/TirType
  Owned ∧ non-escaping        → ThreadConfined → non-atomic RC (or zero via Perceus)
  Immortal (None/bool/smallint/interned/code) → no-op (statically folded — not even a branch)
  escapes to channel/spawn     → Sharable      → transfer by copy/shm (no shared count needed)
  escapes to another live thread → Shared       → biased/atomic RC + per-object ob_mutex — ONLY here
```

**The three honest caveats (and their resolutions):**
1. **Precision is the whole ballgame.** Win = `Tax × op_volume`; conservative analysis that over-classifies `Shared` erodes the edge toward CPython's. **Discipline (binding): when precision is lost, add *facts* (monomorphic call-site guards, class-version guards), NEVER widen the unsafe class.** The edge is largest on typed/numeric/structured code (where molt is already strongest) and smallest on maximally-dynamic introspected code — acceptable.
2. **Cross-thread borrow re-materialization** — the one place molt *adds* an op. Perceus borrow-elision was proven sound *under the GIL*; under free-threading a `Borrowed` value read from a shared container can be freed concurrently between load and use (the PEP 703 borrowed-reference hazard). **Resolution:** a fifth fact `CrossThreadBorrow` forces a real biased incref via the optimistic `_Py_TRY_INCREF`+revalidate path *at the cross-thread read site only*. Intra-thread borrows stay elided in both tiers; cross-thread borrows re-materialize an incref *only* under FreeThreaded. Contained and honest.
3. **Fail-closed safety gate (binding).** The `IsolationClass` JOIN defaults to `Shared` (safe-but-locked) on any uncertainty; `ThreadConfined` is asserted *only* where the ownership lattice **proves** non-escape. A false-`ThreadConfined` under free-threading is a data race (memory unsafety). Same discipline that already gates Free-eligibility. Debug `MOLT_ASSERT_ISOLATION` stamps owning-tid to catch a forged confinement at runtime (analogous to `MOLT_ASSERT_NO_LEAK`).

### 3.2 The ONE unified primitive and scheduler

**The unified primitive is the `SchedulerDomain`-parameterized task, executing on a core-affine loop, with RC discipline selected by `IsolationClass`.** There is exactly one scheduler abstraction; the four modes are four *lowerings* of it, not four runtimes.

**Scheduler shape (resolving the Rust-runtime question from Report C):**
- **Python coroutine graph = core-affine / glommio-shaped (NOT work-stealing).** asyncio is single-threaded *by contract* (Report B §5: deterministic `call_soon` FIFO, Task stepping on one loop thread; `Future`/`Task` not thread-safe). A coroutine and the objects it touches are `ThreadConfined`; stealing a half-run coroutine would either break asyncio ordering or force every awaited object to be `Send`+locked. So **the loop owns its tasks; no stealing of Python coroutines.** This also lets each isolate run its own loop in true parallel.
- **Work-stealing (tokio/crossbeam) pool ONLY for the `Send`-safe offload tier** — `run_in_executor`, blocking-IO offload, `@par` kernels over `Raw`/`Sharable` data, isolate worker pools. This is exactly where work-stealing earns its keep (spiky, balanceable, carries no `ThreadConfined` refs). molt's existing `crossbeam_deque` executor (`scheduler.rs:2551`) is repurposed here, not on the Python heap.
- **Pluggable reactor — no hard io_uring dependency.** io_uring where available (Linux 5.x), epoll/kqueue otherwise, host-poll on WASM. molt already abstracts this (`io_poller.rs`). Portability across native/WASM/WASI/Luau is non-negotiable for the 100-year plan.
- **The bigger async win is compile-time, not scheduler choice:** the `ConcurrencyEffect`/`AwaitFact`/`TaskShape` facts let the compiler **statically elide suspensions** (an `await` of a provably-`≤Sync` callee lowers to a direct call — zero task, zero waiter edge) and **fuse await-chains**. This removes tasks from the queue *before any scheduler sees them* — the `bench_async_spawn_100k`-of-immediately-ready-coroutines case drops from 100k tasks to ~0. No interpreter or tracer can do this.

**How each subsystem sits on the one primitive:**

| Subsystem | How it lowers onto the unified scheduler |
|---|---|
| **threading** | Real OS threads (existing `isolates.rs`), each carrying a TLS `RuntimeState`. Under `GilSerial`: serialized by the per-interpreter GIL (CPython parity). Under `FreeThreaded`: true parallel, RC by `IsolationClass`. **One** spawn path (collapse `molt_thread_spawn`/`molt_thread_submit`/the second pool into one). |
| **asyncio** | The core-affine loop *is* the scheduler in single-loop mode. `call_soon`/`call_at`/`call_later`, Task/Future state machine, `call_soon_threadsafe`/`run_coroutine_threadsafe` shims (Report B §5). Selector-backed via the pluggable reactor. |
| **sockets/selectors** | Blocking/non-blocking/timeout tri-state + `BlockingIOError` (Report B §6) over the reactor. API contract is the parity target; backend (host async net on WASM) may differ. Consolidate `sockets.rs`+`sockets_net.rs`+`socket_pure.rs`. |
| **subinterpreters (PEP 734)** | `Isolate(id)` domain: N interpreters, each a `GilSerial` runtime internally, each its own loop, true parallel. Built on the *existing* TLS-`RuntimeState` + fresh-state `thread_main` seam. Communication via `Sharable` channels (copy-not-share; `memoryview` shares buffer). Surfaces the `concurrent.interpreters` API (`Interpreter`/`Queue`, copy-only object passing). |
| **locks/channels** | `threading.Lock` etc. and channels become `IsolationClass`-aware: a lock guarding `ThreadConfined` data is a no-op under `GilSerial`; channels are the `Sharable` transfer ops (`ChannelPut`/`IsolateTransfer`/`SpawnArg`). |

### 3.3 The flag/mode architecture (future-proof WITHOUT the fast path paying)

Five binding principles:

1. **Mode is a compile-time fact, monomorphized once.** `--unleashed` / `@molt.unleashed` / `@par` regions set the domain; default is `GilSerial`. **Each mode is a separate lowering of the same IR — zero dynamic dispatch on mode on the hot path.** A `GilSerial` binary's `inc_ref` is a non-atomic add with no owner-check; the free-threaded machinery *is not emitted*. This is the literal mechanism by which the fast path pays nothing.
2. **RC atomicity is `cfg`/fact-gated, never runtime-gated.** Non-atomic RC under GIL is the *default*; atomic/biased RC is gated for cpython-abi/`@par`/FreeThreaded. **This is also the fix for Q1's "unconditional atomic tax"**: today's native build pays atomics for signal-safety; the gated design pays them only where a mode actually needs them, and routes signal/ABI-hook safety through a narrower mechanism (see Q5).
3. **One GIL authority, one task-done authority.** Today there are two GIL impls (`concurrency/gil.rs` authoritative + dead `object/gil.rs` stub) and two task-done sources (header flag vs `FutureState`). Converge: **GIL moves onto `RuntimeState` (per-interpreter)** — it already sits next to `scheduler: OnceLock<MoltScheduler>` there — **`object/gil.rs` is deleted**, header flag is the sole task-done authority. This is both a bug-class fix *and* the prerequisite for parallel isolates (a process-global GIL would serialize isolates that must run in parallel).
4. **Anti-blending validator (the cardinal rule).** A validator asserts **no default-tier op reads a `FreeThreaded`-only fact**, and a program never silently gets unleashed semantics via a runtime flag race. The tier is a build-time/region *fact*.
5. **Mis-mixing is unexpressible.** Because `IsolationClass` rides the type, the transfer ops (`ChannelPut`/`IsolateTransfer`/`SpawnArg`) are non-exhaustive over it: a `ThreadConfined` operand is auto-pickled (CPython multiprocessing parity) or a compile error. There is no code path that puts a confined reference on another thread. **Structural guarantee replaces programmer discipline.**

**The mode matrix:**

| Mode | RC | Container locks | Async loop | Parity target |
|---|---|---|---|---|
| **GilSerial** *(default)* | non-atomic / Perceus-elided | none | single core-affine loop | byte-identical CPython ≥3.12 |
| **Isolate** (PEP 734) | non-atomic per isolate | none (shared-nothing) | loop per isolate, parallel | `concurrent.interpreters`, copy-only sharing |
| **FreeThreaded** *(opt-in apex)* | `IsolationClass`-driven: non-atomic confined / biased shared | header `ob_mutex` on `Shared` **only** + optimistic reads | core-affine loop per thread + Send offload pool | == CPython 3.13t (surrenders the *same four* guarantees) |
| **DataParallel** (`@par`) | raw-lane (0) or biased | only if body touches `Shared` | work-stealing pool | OpenMP-like disjointness contract |

### 3.4 The free-threading perf plan (biased/deferred RC + escape-elision + sharded structures)

Ranked by leverage (the whole point: keep the *set* of objects that need expensive RC/locks small):

1. **Escape-analysis RC-atomicity selection** (§3.1) — non-atomic/zero on confined, atomic only on `Shared`. The AOT superpower; biggest lever.
2. **Perceus borrow inference deletes RC ops before they can become atomic tax.** `Borrowed` ⇒ no dup/no drop (borrowed-parameter ABI: `+0` operand / `+1` result). **Prerequisite for FreeThreaded, not a nice-to-have** — it is what makes the surviving-op count small. Land it first.
3. **Immortalization, statically folded.** `None`/`True`/`False`/small ints/interned strings/code/types are compile-time-known ⇒ the immortal inc/dec is *not even a runtime branch* — better than CPython's `ob_ref_local=UINT32_MAX` runtime test. Kills contention on the hottest shared cache lines.
4. **Deferred RC** for cross-thread-shared immutable callables (functions/modules/methods/code) — skip RC, reconcile at GC stop-the-world. Extend the existing `refcount_elim.rs` deferred step to PEP 703's deferred set.
5. **Biased RC** (Choi/Shull/Torrellas PACT'18: +7.3% server throughput / -22.5% client exec-time) for the residual `Shared`-but-one-thread-dominated objects: owner-local non-atomic counter + shared atomic counter, `ob_tid` owner id, non-owner decrements queued and merged at safepoint. The *fallback*, not the common case.
6. **Raw-scalar lane** (`int47`/`bool`/unboxed `f64`) carries **no refcount at all** — Codon-class numeric path, composes with free-threading for free.

**Sharded / lock-free structures — the resolved policy:**
- **Per-object header lock (`ob_mutex`, 1 byte) + optimistic lock-free reads + mimalloc sequenced page reuse (PEP 703 model), applied ONLY to `IsolationClass = Shared` containers.** Reads (`list[i]`/`dict[k]`/iteration) take the optimistic path (atomic-load slot → conditional-incref → revalidate → retry); writes take the header lock. **This is the right default** — proven in CPython 3.13t, wait-free common-case reads, cheap header lock. The dead `object/gil.rs` `ObjectLock` (free-standing `Mutex<()>`) is **deleted** in favor of the header-field lock.
- **mimalloc restricted-page-reuse is a HARD prerequisite, not optional.** The optimistic read is sound *only* because a freed slot's memory cannot be recycled into a different-size object before readers finish (three heaps + sequence-number protocol). Gate it under loom/TSan + Miri strict-provenance. This is the subtlest correctness condition in the whole design.
- **Sharding: DEFER until measured.** It changes iteration/ordering semantics and complicates dict-changed-size detection (B8 parity); optimistic reads already make reads contention-free. Prefer per-object-lock + internal striping over a user-visible sharded type, and only if a benchmark proves single-container *write* contention dominates.
- **Fully lock-free (CAS) containers: AVOID.** Enormous UB surface under NaN-boxing + strict provenance, reintroduces the memory-reclamation hazard mimalloc solves, no demonstrated win over PEP 703's optimistic-read-over-locked-write for Python semantics. A research project, not a deliverable.

**The decisive efficiency lever is upstream:** escape analysis keeps the *set* of `Shared` containers small, so locking cost is paid rarely. molt locks only proven-`Shared` containers where **CPython locks every container** — the structural reason molt's per-object-lock tax is *below* CPython's.

---

## 4. (Q4) The phased 100-year-plan outcome entries (FOUNDATION first, then features)

The sequencing is **lowest-risk-first, foundation before features**. The first three phases deliver most of the throughput value *before any free-threading risk* — correct for a language where most concurrent workloads are async-single-thread or process-parallel.

> **T0 — Concurrency-effect foundation + GIL-release parity.**
> Introduce the `ConcurrencyEffect`/`AwaitFact` fact plane. Achieve exact CPython blocking-IO GIL-release parity. Reproduce the observable atomicity contract: bytecode-boundary atomicity, the FAQ atomic-ops list (`L.append`, `D[x]=y`, …), and `sys.get/setswitchinterval` (5 ms default) tuning molt's scheduler. *Outcome: the fact substrate exists; default-build behavior is CPython-faithful and measured.*

> **T1 — The `IsolationClass` keystone (greenfield on the live escape substrate).**
> Build `isolation_class.rs` as a *projection* of the existing `EscapeState`/ownership lattice — add the single thread axis (`ThreadConfined`/`Sharable`/`Shared`/`Immortal`), fail-closed to `Shared`. Make transfer ops non-exhaustive over it (mis-mixing unexpressible). *Outcome: every value carries a thread-isolation fact; the compiler knows which RC discipline to emit, even though only `GilSerial` lowering is wired yet.*

> **T2 — Async suspension elision + await-chain fusion.**
> Lower `await` of provably-`≤Sync` callees to direct calls; fuse await-chains. *Outcome: massive async-throughput win on the GIL tier alone, before any parallelism work. The 100k-ready-coroutine benchmark collapses to ~0 tasks.*

> **T3 — `SchedulerDomain` convergence + the AOT-spawn multiprocessing superpower.**
> Move the GIL onto `RuntimeState` (per-interpreter); delete `object/gil.rs`; unify the three thread paths and the dual task-done sources. Ship `Isolate(id)` + `concurrent.interpreters` (PEP 734) on the existing TLS-state seam. Exploit AOT re-exec (molt ~3.6 ms startup vs CPython ~18.7 ms, wider for spawn-with-import) to make **process/isolate parallelism the recommended shared-nothing substrate** — sidestepping the GIL for the common server/pool case *without* free-threading. *Outcome: true multicore via isolates; per-interpreter GIL (PEP 684 model) expressible.*

> **T4 — The `FreeThreaded` apex domain (PEP 703 parity, made fast).**
> Land Perceus borrow inference first (T4a, the tax-repayment prerequisite). Then biased/deferred RC + immortal folding + per-object `ob_mutex` + optimistic reads + mimalloc sequenced page reuse + the `CrossThreadBorrow` fact. RC atomicity selected by `IsolationClass`. *Outcome: opt-in free-threading at == CPython 3.13t semantics but with single-thread overhead at/below CPython's, because the tax multiplies a strictly smaller (Perceus-pruned, escape-confined) op count.*

> **T5+ — Long-horizon hardening (the 100-year tail).**
> Per-benchmark escape-precision tracking as the headline metric; loom/TSan/Miri-gated correctness for every `Shared`-path change; reactor backends as platforms evolve (io_uring successors, new WASM threading proposals); track CPython's free-threading trajectory (3.14 supported-optional → eventual default) so flipping molt's default tier is *additive, not a rewrite*. *Outcome: the architecture absorbs a century of CPython evolution as new lowerings of the same IR + fact plane.*

---

## 5. (Q5) What in the CURRENT implementation MUST be refactored to be conducive (the foundation work)

Named files from the audit, ordered as foundation-first. These are the *structural* refactors that unblock everything in Q4.

**A. GIL authority — converge and relocate (unblocks isolates + free-threading).**
- `runtime/molt-runtime/src/concurrency/gil.rs:177` — `PREINIT_GIL` is process-global. **Move the GIL onto `RuntimeState`** (per-interpreter), next to the already-present `scheduler` field (`runtime_state.rs:297`). Keep the `'static` preinit mutex only for the documented pre-init window. *This is the keystone refactor — PEP 684 is not expressible until this lands.*
- `runtime/molt-runtime/src/object/gil.rs` (entire file) — **DELETE.** It is dead-code scaffolding (`ObjectLock`, `GIL_RELEASED`, `is_gil_released`, `gil_check`; verified `:1-59`). Its per-object-lock intent is replaced by the header-field `ob_mutex` (PEP 703 model). Dual GIL sources are a bug class.

**B. Refcount — fact-gate atomicity, fix the racy RMWs.**
- `runtime/molt-runtime/src/object/refcount.rs:16-19` — `MoltRefCount` is unconditionally `AtomicU32` on native for signal/ABI-hook safety. **Refactor to fact-gated atomicity:** non-atomic under `GilSerial` (the default, eliminating Q1's pointless atomic tax), atomic/biased only under `FreeThreaded`/cpython-abi/`@par`. Route signal-handler/ABI-hook safety through a narrower mechanism than "atomic on every op."
- `runtime/molt-runtime/src/object/mod.rs:2030-2038, 2235-2253` — the `dec_ref` free path (non-atomic `load(Acquire)` underflow check + separate `fetch_sub`) and the resurrection window (`load`-then-conditional-`fetch_sub`) are **not single RMWs and would race under free-threading.** Rewrite as single atomic RMWs (or biased-RC equivalents) on the `FreeThreaded` lowering.

**C. Container mutation — independent synchronization (remove sole-GIL reliance).**
- `runtime/molt-runtime/src/object/ops/dict_set_tables.rs:2146,2352` and `runtime/molt-runtime/src/object/ops/specialized_list.rs:245-251` — both rely solely on `gil_assert()`. **Under `Shared`, guard with the per-object header `ob_mutex` + optimistic-read path; under `GilSerial`/`ThreadConfined`, emit no lock at all.** Clarify that `_nogil` means "no refcount work," and introduce a genuinely free-threaded path distinct from it.

**D. `RuntimeState` mutex farm — restructure for the contention cliff.**
- `runtime/molt-runtime/src/state/runtime_state.rs:274-376` — ~50+ independent `Mutex<HashMap>` fields, several taken per task transition. **Foundation work:** (1) under `Isolate`, each interpreter owns its `RuntimeState` (the TLS seam at `:599-618` already enables this — lean on it); (2) consolidate the hot `asyncio_*`/`task_*` maps the scheduler touches per transition into fewer, lock-amortized structures to defuse the cliff before `MOLT_ASYNC_THREADS>0`.

**E. Thread/scheduler unification — one primitive, not three.**
- `runtime/molt-runtime/src/concurrency/isolates.rs:421,456,503,530` (the `threading` spawn paths, shared vs isolate), `runtime/molt-runtime/src/async_rt/threads.rs:50,169` (the *second* pool), and `runtime/molt-runtime/src/async_rt/scheduler.rs:2551` (the default-off work-stealing executor) — **collapse into the single `SchedulerDomain`-parameterized scheduler**: core-affine loop for Python coroutines, work-stealing pool for `Send` offload only. Unify the two task-done authorities (header flag vs `FutureState`) onto the header flag.
- `runtime/molt-runtime/src/async_rt/scheduler.rs:130-139` — `async_worker_threads()` defaulting to 0 is *correct* for `GilSerial` (deterministic single loop); formalize it as the `GilSerial` lowering rather than an env-var accident.

**F. Compiler — build the `IsolationClass` projection on the live substrate.**
- `runtime/molt-passes/src/tir/passes/escape_analysis.rs` — the `EscapeState` lattice is **live and correct** (verified). Note `ArgEscape` is flagged "future refinement" (`:26-28`); tightening it directly improves `ThreadConfined` precision.
- **NEW `runtime/molt-passes/src/tir/passes/isolation_class.rs`** — does not exist yet (grep-confirmed). Build it as a *projection* consuming `escape_analysis.rs` + `ownership_lattice_min.rs` + `alias_analysis.rs`, sharing the `op_kinds.toml` authority. Add the `CrossThreadBorrow` fact for the cross-thread-read hazard. *This is greenfield work on a real foundation — the single most important new compiler pass for the whole plan.*

**G. Subinterpreter surface — implement past the stub.**
- `src/molt/stdlib/_interpreters.py:14-17` (raises "not fully lowered yet") and the `_interpchannels`/`_interpqueues` stubs — **lower onto the `Isolate(id)` domain** with copy-only object passing (`memoryview` shares buffer; sync primitives not shareable), surfaced as `concurrent.interpreters` (PEP 734).

**Foundation-first ordering:** A (GIL onto `RuntimeState` + delete `object/gil.rs`) and F (`isolation_class.rs`) are the two keystones — nothing else in Q4 is expressible without them. B/C/D/E are the correctness-and-contention substrate they stand on. G is the first feature that rides the finished foundation.

---

### Appendix — verification ledger (what I confirmed in-source, 2026-06-27)

- `object/gil.rs:1-59` — **dead free-threading scaffolding confirmed verbatim** (`ObjectLock`, `GIL_RELEASED`/`is_gil_released`/`release_gil`/`acquire_gil`, `gil_check`). → DELETE.
- `object/refcount.rs:1-22` — **atomic-for-signal-safety-not-parallelism rationale confirmed verbatim.** → fact-gate.
- `concurrency/gil.rs:160-230` — **`PREINIT_GIL` process-global static + single-thread fast path confirmed**; the Miri-data-race rationale for one static is real (informs how carefully A must be done).
- `state/runtime_state.rs:274-376` — **~50+ `Mutex<HashMap>` farm confirmed; `gil` is NOT a field; `scheduler: OnceLock<MoltScheduler>` IS a field (:297)** → GIL relocation has a home.
- `molt-passes/.../escape_analysis.rs:1-70` — **`EscapeState` lattice live, drives stack-promotion + RC elision, shares `op_kinds.toml` with `alias_analysis.rs`; `ArgEscape` is "future refinement."**
- Grep across `molt-passes/src` for `IsolationClass|ThreadConfined|isolation_class|SchedulerDomain` — **zero matches. Correction to Report C: the isolation projection is designed-only, not built. The escape substrate is built.**