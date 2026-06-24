<!--
Foundation blueprint 54 — THROUGHPUT: the concurrency / async / parallelism FACT PLANE.
Arc: THROUGHPUT (concurrency, async, parallelism).
Author: portfolio-architect.
Date: 2026-06-24.
Status: DESIGN ONLY / EXECUTABLE PLAN. No implementation landed. Read-only investigation + this one Write.

This doc does NOT duplicate doc 28 (asyncio runtime architecture), doc 33 (threading &
parallelism ladder), or doc 26 (real async generators). It sits ABOVE them: it supplies the
ONE missing structural layer those three docs each presuppose but none builds — the first-class,
generated, checkable CONCURRENCY FACT PLANE that (a) makes a whole CLASS of data races
*unexpressible* and (b) lets the scheduler *fuse/elide* tasks and await edges. docs 28/33/26
become CONSUMERS of these facts; this doc is the producer + transport + validator contract.

All file:line anchors verified against the live worktree on branch `main` at the session HEAD
(2026-06-24). Where doc 33 (HEAD bd0b76d3, 2026-06-06) and doc 28 (2026-06-06) cite anchors,
this doc re-verifies the load-bearing ones and flags drift inline. The doc-33 anchors that still
hold at HEAD: scheduler uses `crossbeam_deque::{Injector, Worker}` (scheduler.rs:11), worker
count `num_cpus::get().max(1)` (scheduler.rs:378), task state on `MoltHeader` flag bits
(HEADER_FLAG_TASK_{DONE,QUEUED,RUNNING,WAKE_PENDING}, scheduler.rs:19-20), isolates spawn a fresh
RuntimeState per thread (isolates.rs:422 `thread_main`, :456 `thread_main_shared`,
MOLT_THREAD_ISOLATED escape hatch :511). Confirmed ABSENT at HEAD: any `__molt_spawn` re-exec
entry, any `Sharable`/isolation trait, any cross-isolate Send/Sync transfer proof. Those gaps are
what this plan's facts make impossible to get wrong.

Provenance (study + reimplement; PSF/MIT = semantics reference only, no GPL ingested): PEP 703
(biased RC, per-object lock, immortal, mimalloc page-sequence), PEP 734 (per-interpreter GIL,
sharable objects, InterpreterPoolExecutor), the Choi/Shull/Torrellas BRC paper (PACT'18), Reinking
et al. Perceus (PLDI'21, via doc 27), Rust's Send/Sync auto-trait discipline (the structural model
this doc maps onto a Python-semantic IsolationClass), Tokio's task budget + structured JoinSet,
Trio/structured-concurrency nursery semantics, OpenMP `parallel for` (the @par disjointness
contract). Cited inline.
-->

# 54 — THROUGHPUT: the concurrency / async / parallelism fact plane

**Document status:** Foundation blueprint, design-only, executable. The THROUGHPUT arc's
single structural map. Composes with — and is the missing keystone under — docs 28, 33, 26.

**Scope (the arc):** the end-state for concurrency, async, and parallelism in molt: true
GIL-free parallelism with exact CPython async/threading/multiprocessing semantics, exceeding
CPython throughput and matching/beating PyPy on concurrent workloads. This doc designs the
**structural model** as a FACT PLANE — the IR/runtime facts that make data races unexpressible
and that let the scheduler fuse/elide — not as another pile of runtime mechanism. The mechanism
already has two excellent design docs (28 runtime, 33 ladder) and a third for generators (26).
What is missing, and what every one of those docs silently assumes, is the *representation* that
makes their guarantees checkable obligations rather than conventions. That representation is this
doc.

---

## 0. The end-state, stated crisply (the time-traveler's terminus)

Work backward from the 5-to-100-year terminus:

> **Concurrency in molt is a property the compiler PROVES, not a discipline the programmer
> maintains.** Every value carries a machine-checked `IsolationClass`; every coroutine/function
> carries a machine-checked `ConcurrencyEffect`; every task and every await edge carries a
> machine-checked `TaskShape`/`AwaitFact`. From these four fact families it is **structurally
> impossible** to (a) move a thread-confined object across a thread/isolate boundary, (b) share a
> mutable object between OS threads without the per-object lock the unleashed tier requires,
> (c) emit a scheduler suspension where the program never actually suspends, or (d) keep an
> await-chain edge the optimizer could collapse. The default tier is byte-identical to CPython
> under the GIL; the unleashed tier is byte-identical to CPython 3.13t; the multiprocessing tier
> is byte-identical to CPython spawn but ~10-75× faster to start; and on every tier molt is faster
> than CPython and matches/beats PyPy and uvloop on concurrent workloads — because the facts let
> the scheduler do work CPython's interpreter and PyPy's tracer cannot: statically elide
> suspensions, fuse await-chains, and confine RC traffic to a single thread.**

The CLASSES this terminus retires (the compression ladder, one fact family per class):

| Fact family (new IR/runtime fact) | The CLASS of wrongness/slowness it makes UNEXPRESSIBLE |
|---|---|
| **`IsolationClass`** (`ThreadConfined / Shared / Sharable / Immortal`) on every value's Repr/TirType | the entire class of **data races and illegal cross-boundary transfers**: a `ThreadConfined` value *cannot* be the operand of a thread-spawn payload, channel `put`, or isolate transfer op — it is a compile error, not a runtime UB. This is the keystone fact: it is to concurrency what `Repr` is to boxing. |
| **`ConcurrencyEffect`** (`Sync / MayBlock / MaySuspend / Pure`) on every callable | the class of **GIL-held-across-blocking-syscall parity bugs** (doc 33 §2-b io.rs/subprocess) — a `MayBlock` intrinsic that is reached GIL-held is a *verifier* failure; AND the class of **spurious scheduler suspensions** — a coroutine proven `Sync`/`Pure` between awaits is run inline with no frame save/restore (the fuse/elide lever). |
| **`TaskShape`** (`Detached / Joined(scope) / Inline / Elided`) + **`AwaitFact`** (edge: `Suspends / Immediate / Fused`) on every task and await | the class of **structured-concurrency leaks** (an orphaned task outliving its nursery) AND the class of **await-graph overhead** (a `gather` of N immediately-ready coroutines that allocates N tasks + N waiter edges) — an `Immediate`/`Fused` await edge generates *zero* task object, *zero* waiter-graph entry, *zero* suspension. |
| **`SchedulerDomain`** (`GilSerial / Isolate(id) / FreeThreaded / DataParallel`) on every runtime-state + queue | the class of **dual-source-of-truth GIL/scheduler bugs** (doc 28 §1.4 HEADER_FLAG vs FutureState; doc 33 §1.2 two GIL authorities) — there is exactly one authority per domain, selected by a build-time + per-region fact, never a runtime flag race. |

These four families are the deliverable. Everything in §§3-7 is how to build, transport, validate,
and consume them, in dependency order, with green gates.

---

## 1. Why a fact plane (not "more runtime") — the method, applied to throughput

The roadmap's recurring root cause (doc 51 §1): *the compiler reconstructs Python semantics from
low-level events after the high-level meaning was already lost.* Concurrency is the worst-hit
victim of this in the current tree:

- **Data-race safety is reconstructed, not represented.** Doc 33 §3.5 commits to a precise
  memory model ("memory-safe, logically-racy, == CPython 3.13t"), but at HEAD there is *no fact*
  on any value that says "this object may cross a thread boundary." The `unsafe impl Send` that
  cross-thread transfer needs is, at HEAD, simply absent from isolates.rs — so the *only* thing
  keeping `threading.Thread(target=fn, args=(obj,))` from racing is that the GIL serializes
  everything. The moment the GIL is removed (doc 33 P3), nothing structural prevents a
  thread-confined object from being raced. **The fix is not "add locks everywhere" — it is to make
  the illegal transfer unexpressible** by giving every value an `IsolationClass` the transfer ops
  consume.
- **Scheduler overhead is reconstructed per-poll, not elided at compile time.** Doc 28 §1.2-1.3
  inventories the cost: 8+ Mutex acquires in exception save/restore *per task poll*, a `Vec` alloc
  per `run_once`, a Task heap object + waiter-graph edges per `create_task`. Doc 28 attacks these
  with better runtime data structures (intrusive ready list §2.1, timer wheel §2.3). That is
  correct and necessary — but it leaves on the table the bigger win: **a coroutine that never
  actually suspends between two awaits should not be a task at all.** CPython cannot know this;
  PyPy discovers it dynamically via tracing. molt can *prove* it at compile time with a
  `ConcurrencyEffect`/`AwaitFact` and emit a direct call — zero scheduler interaction. This is the
  fuse/elide lever, and it is a *representation* the runtime docs presuppose but do not define.
- **Two GIL authorities, two task-done authorities.** Doc 33 §1.2 (two GIL impls: concurrency/gil.rs
  authoritative vs object/gil.rs stub) and doc 28 §1.4 (HEADER_FLAG_TASK_DONE vs FutureState.done)
  are *the same disease*: a semantic property with two sources of truth that drift. The cure is a
  single fact (`SchedulerDomain`) and a single done-authority, not a patch at each drift site.

The throughput fact plane is therefore the structural prerequisite for *both* halves of the
contract: it makes the unleashed tier *safe* (IsolationClass) and makes every tier *fast*
(ConcurrencyEffect + AwaitFact + TaskShape let the scheduler do work no interpreter can).

**Composition, stated once:**
- **This doc is the producer; docs 28/33/26 are consumers.** Doc 33's per-object lock (§3.4) is
  *applied only to `Shared` values* (the IsolationClass tells it which); doc 33's BRC owner-bias
  (§3.2) is *the lowering of `ThreadConfined`*; doc 28's intrusive ready list (§2.1) and timer
  wheel hold only `Detached`/`Joined` tasks (TaskShape removes `Inline`/`Elided` ones before they
  reach the queue); doc 26's generator fusion bails exactly where `ConcurrencyEffect = MaySuspend`
  crosses a fusion barrier. **No mechanism in 28/33/26 is rebuilt here; each gains a fact input.**
- **Depends on the memory-safety / ownership-lattice arc** (the docs 45/48/49/50 family + the
  council's ownership lattice; the prompt's "doc 55"). `IsolationClass` is a *projection* of the
  ownership lattice onto the thread/isolate axis: a value that is `Owned` and non-escaping in the
  ownership lattice is `ThreadConfined`; a value that escapes to a channel/thread becomes `Shared`
  or `Sharable`. The two lattices share one `alias-root` substrate. **IsolationClass does not
  re-derive escape — it consumes the ownership lattice's escape fact and adds the thread axis.**
- **Depends on / extends generator fusion** (doc 26 / doc 51 §generator-fusion). `AwaitFact = Fused`
  is the async analogue of generator fusion: a `MaySuspend`-free await sub-chain fuses into the
  caller's frame exactly as a `def-yield` generator fuses. The two share the resumable-frame
  ownership model (doc 52 §C.2-9). **Async fusion is generator fusion on the await edge.**

---

## 2. The current state, audited at HEAD (what the facts must cover)

This grounds the plan in the tree. Anchors re-verified this session.

### 2.1 Scheduler / task substrate (the fuse/elide target)
- `runtime/molt-runtime/src/async_rt/scheduler.rs` (153,416 bytes at HEAD — a god-file, see §6
  decomposition): `MoltScheduler` (scheduler.rs:2743) wraps `Arc<Injector<MoltTask>>`
  (:2744); worker pool sized `num_cpus::get().max(1)` (:378). `execute_task` (:2991) does the
  per-poll flag dance on `MoltHeader` (HEADER_FLAG_TASK_{DONE :3000, QUEUED, RUNNING :3033,
  WAKE_PENDING}) and the exception state save/restore doc 28 §1.3 measured at 8+ Mutex acquires.
  `enqueue_task_ptr` (:3400) is the single enqueue authority; `block_on` is the run-loop.
- **Fact gap:** every coroutine, ready or not, becomes a `MoltTask` and rides the Injector. There
  is no fact distinguishing "this await will suspend" from "this await is already-ready." The
  100k-immediate-gather benchmark (doc 28 §1.5 `bench_async_spawn_100k`) pays 100k task allocs +
  100k waiter edges for coroutines that never suspend. **`TaskShape`/`AwaitFact` is the missing
  input that lets `execute_task` skip the queue entirely.**

### 2.2 Threading / isolate substrate (the data-race target)
- `runtime/molt-runtime/src/concurrency/isolates.rs`: `thread_main` (:422) allocates a fresh
  `RuntimeState` per isolate thread; `thread_main_shared` (:456) reuses the parent's state (the
  `threading.Thread` shared-globals default); `MOLT_THREAD_ISOLATED` (:511) is the per-thread
  isolate escape hatch. `molt_isolate_bootstrap` (:45) / `molt_isolate_import` (:46) are the FFI
  bootstrap.
- **Fact gap:** the payload crossing into `thread_main`/`thread_main_shared` is a `Vec<u8>` with
  no isolation typing. Nothing prevents a `ThreadConfined` Python object's pointer from being put
  in that payload. Under the GIL this is benign (serialized); under doc 33's unleashed tier it is a
  race. **`IsolationClass` on the payload operand is the missing input that makes the illegal
  transfer a compile error.**
- `runtime/molt-runtime/src/concurrency/gil.rs` (authoritative GIL) vs
  `runtime/molt-runtime/src/object/gil.rs` (the stub doc 33 §1.2 marks for deletion). **Two
  authorities** — `SchedulerDomain` collapses them.
- `runtime/molt-runtime/src/concurrency/locks.rs` (116,698 bytes): Lock/RLock/Condition/Event/
  Semaphore/Barrier/Queue, all `std::sync::Mutex + Condvar`; `MoltLocal` is a tid-keyed
  `Mutex<HashMap>` (doc 33 §2-f teardown gap). **`IsolationClass` + the per-object-lock-on-Shared
  policy tells locks.rs which uncontended fast path applies.**

### 2.3 Channels / IPC (the transfer target)
- `runtime/molt-runtime/src/async_rt/channels.rs` (121,729 bytes, modified at HEAD): `MoltChannel`
  (:71), `MoltStream` (:76), `MoltWebSocket` (:105) over `crossbeam_channel` (:1). These are
  async byte/object channels.
- **Fact gap + the multiprocessing superpower hole:** there is NO `Sharable` trait and NO
  `__molt_spawn` re-exec entry at HEAD (confirmed absent). Doc 33 §4-c's "AOT spawn superpower"
  (re-exec self in ~4ms vs CPython's 50-300ms import-per-worker) is undesigned in code. **The
  channel `put`/isolate-transfer/spawn-payload ops are exactly the three sites that must consume
  `IsolationClass` — and `Sharable` is the IsolationClass value that says "transfer by zero-copy
  shm / structured-clone," vs `Shared` = "needs the per-object lock," vs `ThreadConfined` =
  "illegal to transfer, pickle-or-error."**

### 2.4 Frontend async lowering (where the facts are first attached)
- `src/molt/frontend/__init__.py`: `visit_AsyncFunctionDef` (many sites; the canonical lowering
  cluster around :2147, :2284, :3602, :6211+), `visit_AsyncFor` (:2334, :6151, :7151),
  `visit_AsyncWith` (:2369, :6163, :7161), `visit_Await` (:8277, :8307). `is_coroutine` flag
  (:4250) → `__molt_is_coroutine__` attr (:4464).
- **Fact gap:** the frontend knows the await structure (the AST is right there) but lowers every
  `await` to the same generic suspend protocol. **The frontend is where `ConcurrencyEffect` and
  `AwaitFact` are first computed (a function with no `await` that can block is `MayBlock`; an
  `await` whose operand is a known-immediate future is `Immediate`) and attached to the op so the
  midend/backend can fuse.** This composes with doc 44 (frontend architecture F2) and the
  decomposition of `__init__.py` (doc 21c mixin split — async lowering becomes a cohesive mixin).

---

## 3. The four fact families (producer / transport / consumer / validator)

Each fact follows the binding fact-plane contract (doc 52 §B-loop-4, doc 46 semantic control
plane): **producer + transport (round-trip!) + consumer + a validator (Alive2-style checkable
obligation, doc 51 §1 / #75) + a test at each layer.** The recurring landmine (doc 52): facts die
silently at representation boundaries (serialization, re-lifts). Each family below names its
boundary crossings explicitly.

### 3.1 `IsolationClass` — the keystone (makes data races unexpressible)

**The fact.** A 2-bit lattice attached to every value's `Repr`/`TirType` (so it rides the type,
not a side-table — same discipline as the boxing Repr):

```
IsolationClass (join-semilattice, ⊥ = ThreadConfined, ⊤ = Immortal):
  ThreadConfined   -- created and used on one thread; never escapes to another. The default.
  Shared           -- reachable from >1 thread under the unleashed tier; mutation needs ob_mutex.
  Sharable         -- transferable across an isolate/process boundary by value
                      (bytes/int/float/str/None/memoryview-over-shm) — PEP 734 "sharable".
  Immortal         -- None/True/False/small-int/interned-str/type objects; inc/dec no-op (PEP 703).
join: ThreadConfined ⊔ Shared = Shared; anything ⊔ Immortal = the other; Sharable is orthogonal
      (a value is Sharable XOR Shared — Sharable transfers by copy, Shared by reference+lock).
```

- **Producer.** A dataflow analysis on the ownership-lattice substrate (the docs 45/48/49/50
  family). Seed: literals/constructors are `ThreadConfined`; the immortal set is `Immortal`
  (constant); the *transfer ops* (`ThreadSpawnPayload`, `ChannelPut`, `IsolateTransfer`,
  `SpawnArg`) are the escape points that JOIN their operand up to `Shared` (if the build tier is
  unleashed and the object stays a reference) or require `Sharable` (if crossing a heap boundary).
  This is a *projection* of the ownership lattice's escape fact onto the thread axis — **it does
  not re-run escape analysis; it consumes it.** New file `runtime/molt-tir/src/tir/passes/
  isolation_class.rs` (an `AnalysisId::IsolationClass` registered with the AnalysisManager, doc 00
  §S1), reading `AnalysisId::{Alias, Liveness, Escape}` and the ownership lattice.
- **Transport.** Rides `Repr`/`TirType` through every pass and across the serialization boundary
  (`src/molt/frontend/lowering/serialization.py` — modified at HEAD; this is exactly a boundary the
  landmine warns about). Round-trip test: a value's IsolationClass survives lower→serialize→re-lift.
- **Consumer (the unexpressibility).** The four transfer ops are made **non-exhaustive over
  IsolationClass at the type level**: their lowering `match`es the operand's IsolationClass and has
  *no default arm that accepts `ThreadConfined` by reference*. A `ThreadConfined` operand to
  `ChannelPut`/`IsolateTransfer`/`SpawnArg` is either (a) auto-pickled if it is picklable
  (CPython parity — `multiprocessing` pickles args) producing a `Sharable` copy, or (b) a
  compile error `TypeError: cannot share <thread-confined object> across <boundary>` if not
  picklable. **There is no code path that puts a `ThreadConfined` reference on another thread.**
  Under the unleashed tier, `ChannelPut` of a `Shared` reference is allowed and the consumer
  (locks.rs / object ops) applies the per-object `ob_mutex` (doc 33 §3.4) *because the operand is
  `Shared`* — the lock is applied exactly where the fact says it is needed, nowhere else.
- **Validator (checkable obligation, #75).** `MOLT_ASSERT_ISOLATION`: a debug-build runtime check
  that, on every transfer op, asserts the runtime object's actual reachability matches its static
  IsolationClass (a `ThreadConfined`-typed pointer must not be observed from a second thread —
  checked by a per-object owning-tid stamp under debug). Plus a static Alive2-style obligation:
  the producer's join is monotone and the consumer's match is exhaustive (compile-time, via the
  op_kinds registry classifier set — doc 25). This is the analogue of `MOLT_ASSERT_NO_LEAK`.

**Why this is the keystone:** it is the single fact that turns doc 33's *prose* memory model
(§3.5 "memory-safe, logically-racy") into a *structural guarantee*. The unleashed tier is safe
not because every container has a lock, but because the only objects that *can* be shared are
`Shared`, and `Shared` is exactly the set that carries the lock. Data races on `ThreadConfined`
objects are unexpressible because such objects cannot reach a second thread.

### 3.2 `ConcurrencyEffect` — the blocking/suspension fact (parity + fuse)

**The fact.** A small effect lattice on every callable (function/coroutine/intrinsic), in the
CallFacts family (doc 51 §5, doc 47):

```
ConcurrencyEffect:
  Pure       -- no blocking syscall, no suspension, no shared mutation. Fully reorderable/elidable.
  Sync       -- runs to completion without yielding to the scheduler or blocking a syscall.
  MayBlock   -- may enter a blocking syscall (file read, socket recv, lock acquire, proc wait).
  MaySuspend -- a coroutine that may actually suspend (await an incomplete future).
order: Pure < Sync < {MayBlock, MaySuspend} (MayBlock and MaySuspend are incomparable peers).
```

- **Producer.** Frontend seeds (the AST knows: a function with no `await` is not `MaySuspend`;
  a function calling a known-blocking intrinsic is `MayBlock`) refined by the call-graph IP-summary
  pass (doc 00 §S4 `ip_summary`): a function's effect is the join of its callees' effects. The
  blocking-intrinsic set is a *generated* op_kinds classifier (doc 25 op_kinds.toml `[[opcode]]`
  rows + a `blocking` classifier), so "which intrinsics block" is one authority, not a hand-list.
- **Transport.** A CallFacts field on the call op; survives serialization (round-trip tested at the
  call site, per the landmine).
- **Consumer #1 — the parity guard (retires doc 33 §2-b).** The verifier
  (`runtime/molt-tir/src/tir/verify.rs`) gains an obligation: **a `MayBlock` intrinsic reached on a
  code path that statically holds the GIL without a `GilReleaseGuard` is a verify failure.** This
  is doc 33's proposed `MOLT_ASSERT_GIL_RELEASED_ON_BLOCK` runtime harness *promoted to a
  compile-time obligation* — the io.rs/subprocess GIL-held-across-read bugs become unexpressible,
  not just caught at runtime. (The runtime assert stays as the debug belt-and-suspenders.)
- **Consumer #2 — the fuse/elide lever.** In a coroutine, an `await` of a callee whose
  `ConcurrencyEffect ≤ Sync` (i.e. `Pure` or `Sync` — it cannot actually suspend) is lowered as a
  **direct call**, not a scheduler suspension: no frame save, no `MoltTask`, no waiter edge. The
  await "completes inline." This is the single biggest async-throughput unlock and is *exactly*
  what produces the `AwaitFact = Immediate` edges §3.3 consumes. CPython cannot do this (its
  interpreter always drives the coroutine protocol); PyPy does it dynamically; molt proves it
  statically. **This is the throughput thesis for async: most awaits in real code (`await
  self._validate(x)`, `await cache.get(k)` on a hot cache) never suspend, and molt elides them to
  direct calls.**

### 3.3 `TaskShape` + `AwaitFact` — the structured-concurrency + await-graph fact

**`TaskShape`** (on every task-producing op — `create_task`, `gather`, `ensure_future`):

```
TaskShape:
  Elided   -- the coroutine is ConcurrencyEffect ≤ Sync end-to-end: no task object at all;
              the body is inlined at the await site (§3.2 consumer #2 produced this).
  Inline   -- runs on the current task's frame to completion before yielding (a tail-await).
  Joined(scope) -- structured: the task is owned by a nursery/TaskGroup `scope`; the scope's exit
                   is a join barrier. Cannot outlive `scope` (structured-concurrency guarantee).
  Detached -- a bare create_task with no owning scope (CPython-legal; the leak-prone case).
```

**`AwaitFact`** (on every `await` edge in the await-waiter graph):

```
AwaitFact:
  Immediate -- the awaited future is provably already-done at the await (ConcurrencyEffect ≤ Sync,
               or an already-resolved future). Lowered to a value read; no waiter-graph edge.
  Fused     -- the awaited coroutine's frame is fused into the awaiter's frame (async analogue of
               generator fusion, doc 26): one frame, no cross-frame waiter edge, no second task.
  Suspends  -- a genuine suspension: the only case that allocates a waiter-graph edge and may
               re-enter the scheduler. The current code's universal case becomes the rare case.
```

- **Producer.** `TaskShape` from `ConcurrencyEffect` (Elided/Inline) + a structured-concurrency
  scope analysis (the frontend `async with TaskGroup()`/nursery binds tasks to the scope's
  lifetime). `AwaitFact` from `ConcurrencyEffect` of the awaited callee + the fusion-eligibility
  predicate shared with doc 26. New analysis `runtime/molt-tir/src/tir/passes/await_graph.rs`
  (consumes `IsolationClass` — a `Fused` await must not cross a thread boundary — and
  `ConcurrencyEffect`).
- **Transport.** On the task/await ops; round-trip tested.
- **Consumer (retires two classes at once).**
  1. **Await-graph overhead (perf):** doc 28's intrusive ready list (§2.1) and the waiter graph
     (§2.7, the "3 nested Mutex per edge") receive **only `Suspends` edges**. An `Immediate` await
     is a value read (zero edge, zero task); a `Fused` await shares the frame (zero second task).
     The `bench_async_spawn_100k` of immediately-ready coroutines drops from 100k tasks+edges to
     ~0. This is the structural reason molt beats uvloop on the gather-storm benchmark.
  2. **Structured-concurrency leaks (correctness):** a `Detached` task that escapes its lexical
     region without an owner is a **warning under the strict (unleashed) structured-concurrency
     mode** (doc 28 §2.8's Trio-strict tier) and is *guaranteed-joined* under `Joined(scope)` — the
     scope's exit op is a join barrier that cannot be elided. An orphaned task outliving its
     nursery is unexpressible in strict mode, and observable (warned) in default mode. This is the
     structured-concurrency guarantee made structural, composing with doc 28 §2.8 (CPython cancel
     semantics default; Trio-strict opt-in).

### 3.4 `SchedulerDomain` — the one-authority fact (kills dual-source-of-truth)

**The fact.** A per-`RuntimeState` + per-queue tag selecting the execution model:

```
SchedulerDomain:
  GilSerial      -- default tier: one GIL per interpreter, threads serialized (doc 33 Layer A).
  Isolate(id)    -- this RuntimeState is an isolate with its own GIL+heap (doc 33 Layer B / PEP 734).
  FreeThreaded   -- unleashed tier: GIL gone, biased RC + per-object locks active (doc 33 Layer C).
  DataParallel   -- a @par region's worker domain (doc 33 §4-f): Raw-data only under default tier.
```

- **Producer.** Build-time (the `--unleashed` build sets the default domain to `FreeThreaded`;
  default build is `GilSerial`) + per-region (`@molt.unleashed`/`@par` decorators set a region's
  domain). It is **a fact, not a runtime flag** — selected once, never raced (this is the precise
  cure for doc 33 §9 refusal #1 "no global runtime free-threading switch").
- **Transport / Consumer (the convergence).** This fact is what lets doc 33 §1.2 *delete*
  `object/gil.rs`: the GIL lives on `RuntimeState`, and which GIL discipline applies is read from
  `SchedulerDomain`, not from a second static. Likewise doc 28 §1.4's HEADER_FLAG vs FutureState
  duality collapses: the task-done authority is the `MoltHeader` flag *in every domain*, and
  `FutureState` for native-managed tasks is deleted (doc 28 §1.4's chosen fix), so there is one
  done-authority gated by one domain fact. **Validator:** a grep-level + verify-level obligation
  that exactly one GIL type and one task-done authority exist (doc 33 §7-P1 gate: `ObjectLock`
  appears zero times; `struct GilGuard` once).

---

## 4. How the facts let the scheduler FUSE and ELIDE (the throughput thesis, concretely)

The mandate: name the IR/runtime facts that let the scheduler fuse/elide. Three concrete
transformations, each gated on a fact above, each a class-kill:

1. **Suspension elision (`ConcurrencyEffect ≤ Sync` ⇒ direct call).** An `await f(x)` where `f`'s
   `ConcurrencyEffect` is `Pure`/`Sync` becomes `let r = f(x)` — no coroutine state machine, no
   `MoltTask`, no scheduler round-trip, no exception save/restore. Measured target: the
   `bench_async_sleep0`-shaped hot path where the awaited thing is already done runs at direct-call
   speed (>> uvloop's ~1.5M/s, toward Tokio's ~5M/s, because there is no scheduler at all on the
   elided edge).

2. **Await-chain fusion (`AwaitFact = Fused` ⇒ one frame).** A chain `await a()` → `a` does
   `await b()` → `b` does `await c()`, where none of a/b/c suspends *before* the leaf, fuses into a
   single frame (the async analogue of doc 26 generator fusion; shares the resumable-frame
   ownership model). Only the leaf suspension (if any) re-enters the scheduler. This collapses the
   waiter graph depth from O(chain length) to O(1) and is the structural answer to doc 28 §1.3's
   "await-waiter graph: 3 nested Mutex per edge" — the edges simply do not exist for the fused
   prefix.

3. **Task elision in `gather`/`TaskGroup` (`TaskShape = Elided`).** `asyncio.gather(*coros)` where
   the coros are `Elided`/`Immediate` collects results by direct evaluation, allocating tasks only
   for the `Suspends` subset. A gather of 100 coroutines where 95 are immediately ready allocates 5
   tasks, not 100. This composes with doc 28's intrusive ready list: the list only ever holds the
   genuinely-suspending tasks.

And the facts let the scheduler do work CPython/PyPy structurally cannot:
- **CPython** always drives the full coroutine protocol (no static effect knowledge) → pays the
  per-await overhead unconditionally.
- **PyPy** discovers immediacy *dynamically* via tracing → pays warmup + guard cost and cannot
  elide across an un-traced boundary.
- **molt** proves it at compile time → zero runtime discovery cost, elision across module
  boundaries (via the IP-summary `ConcurrencyEffect`). This is the "match/beat PyPy on concurrent
  workloads" lever named structurally.

For the **parallel** (not async) throughput, the facts are what make doc 33's tiers *fast and
safe simultaneously*: `IsolationClass` confines RC traffic to the owning thread (a `ThreadConfined`
value's inc/dec is the BRC owner-local non-atomic store — doc 33 §3.2 — *because the fact says it
never escapes*), repaying doc 33 §3.3's thesis (molt's nogil tax < CPython's) structurally rather
than by hope; and `@par` (doc 33 §4-f) consumes `IsolationClass = ThreadConfined ∨ Sharable` per
iteration plus the L4 disjointness proof (doc 04) to fire — a body touching a `Shared` object
without a lock cannot be parallelized (fail-closed).

---

## 5. PHASES (dependency order, each independently landable with green gates)

The unit of work is the complete structural change (CLAUDE.md). Phases are ordered so each ships a
complete fact family (producer+transport+consumer+validator) that delivers value alone, and so the
high-risk free-threading work (which depends on IsolationClass + the ownership lattice + doc 27)
comes last. **This plan's phases COMPOSE with doc 33's P0-P4 and doc 28's phases — they are not a
re-do.** Where a doc-33/28 phase needs a fact this plan produces, the dependency is named.

### Phase T0 — `ConcurrencyEffect` fact + the blocking-GIL obligation (parity, fact-first)
**Why first:** it is the smallest complete fact, it retires a live parity class (doc 33 §2-b
GIL-held-across-blocking-IO), and it is the prerequisite for every fuse/elide phase.
**Scope:** (1) `ConcurrencyEffect` producer (frontend seeds + IP-summary join), transport on
CallFacts, the generated `blocking` op_kinds classifier (doc 25). (2) Consumer #1: the verify.rs
obligation that a `MayBlock` intrinsic GIL-held is a verify failure — and FIX the io.rs/subprocess
sites doc 33 §2-b lists (`io.rs:3777,4243,6432,6608`, subprocess_ext.rs) to wrap blocking
read/write/wait in `GilReleaseGuard`, now *driven by the fact* (the verifier finds them). (3) The
`MOLT_ASSERT_GIL_RELEASED_ON_BLOCK` runtime debug assert as belt-and-suspenders.
**Composes with:** doc 33 P0 (this IS doc 33 P0's §2-b, done fact-first so the fix is complete by
construction, not by enumeration). Independent of doc 33's §2-a switch-interval (that ships in T-par
P1 alongside the safepoint).
**Gate:** `bench_blocking_io_release` (doc 33 §6) passes (concurrent compute runs full speed during
a blocking read); the verifier flags a deliberately-reintroduced GIL-held-blocking site (negative
test); full differential suite green native+LLVM+WASM; round-trip test: ConcurrencyEffect survives
serialization; no single-thread regression. **Owner-lane:** A (parity/safety). **LoC ~600-900.**

### Phase T1 — `IsolationClass` fact + transfer-op unexpressibility (the keystone)
**Why second:** it is the keystone that makes the unleashed tier *possible to build safely*; it must
exist before any GIL-removal. It depends on the ownership-lattice arc (doc 55 family) being landed
enough to provide the escape fact (gate the phase on that, per the council's "gated on doc-27/lattice"
discipline).
**Scope:** (1) `isolation_class.rs` analysis (producer) as a projection of the ownership lattice's
escape onto the thread axis. (2) Transport on Repr/TirType through every pass + the serialization
round-trip. (3) Consumer: make `ChannelPut`/`IsolateTransfer`/`SpawnArg`/`ThreadSpawnPayload`
non-exhaustive over IsolationClass — `ThreadConfined` operand ⇒ auto-pickle-or-compile-error;
`Sharable` ⇒ zero-copy transfer; `Shared` ⇒ (unleashed only) reference + ob_mutex. (4) Validator
`MOLT_ASSERT_ISOLATION` (debug owning-tid stamp) + the static exhaustiveness obligation.
**Composes with:** doc 33 §3.4 (per-object lock applied only to `Shared`), §3.2 (BRC owner-local for
`ThreadConfined`), §4-c/4-d (transfer ops). The whole memory model of doc 33 §3.5 becomes the
*consumer contract* of this fact.
**Gate:** a `ThreadConfined` object passed to a channel/thread/isolate is a compile error if
unpicklable, an auto-pickle if picklable (differential vs CPython multiprocessing pickling
behavior); `MOLT_ASSERT_ISOLATION` catches a deliberately-forged cross-thread `ThreadConfined`
access (negative test); IsolationClass round-trips through serialization (the landmine test);
default-tier programs unaffected (every value is effectively confined under the GIL — no behavior
change). **Owner-lane:** A (safety). **LoC ~1200-1800.**

### Phase T2 — `TaskShape` + `AwaitFact` + suspension elision (the async throughput unlock)
**Why third:** it depends on `ConcurrencyEffect` (T0) and consumes doc 28's runtime structures.
This is the fuse/elide phase — the headline async-throughput win.
**Scope:** (1) `await_graph.rs` analysis producing `TaskShape`/`AwaitFact` (consumes T0's effect +
T1's IsolationClass for the `Fused`-not-across-threads guard). (2) Frontend lowering: `await` of an
`≤ Sync` callee → direct call (suspension elision); `Fused` await → frame fusion (shared resumable-
frame model with doc 26). (3) `gather`/`TaskGroup` task elision for the `Elided`/`Immediate` subset.
(4) Structured-concurrency scope binding (`Joined(scope)` join barrier; `Detached` warning under
strict mode).
**Composes with:** doc 28 §2.1 (intrusive ready list holds only `Suspends` tasks), §2.7 (waiter
graph holds only `Suspends` edges), §1.4 (the single done-authority via T3's SchedulerDomain — land
T2's gather-elision after or with the done-authority fix); doc 26 (async fusion = generator fusion
on the await edge, shared frame-ownership). **Cross-dependency:** doc 28's runtime data-structure
phases (intrusive list, timer wheel) are the *substrate* T2's facts prune; land doc 28's intrusive
list first or concurrently so the pruned queue has a home.
**Gate:** `bench_async_spawn_100k` of immediately-ready coroutines allocates ~0 tasks (verified by
an alloc counter — a DIMENSIONAL win reported as such per the constitution, plus the warm-time win);
`bench_async_sleep0` hot immediate path > uvloop baseline; a genuinely-suspending workload is
byte-identical to CPython asyncio (no elision of a real suspension — negative test: a coroutine that
*does* suspend is NOT elided); structured-concurrency: an orphaned task is warned (strict) /
joined (Joined). **Owner-lane:** B (perf frontier). **LoC ~1500-2500.**

### Phase T3 — `SchedulerDomain` + one-authority convergence (kills dual-source-of-truth)
**Why fourth:** it is the convergence that doc 33 §1.2 (delete object/gil.rs) and doc 28 §1.4
(delete native-task FutureState) both need, and it is the prerequisite for the free-threaded domain.
**Scope:** (1) `SchedulerDomain` on RuntimeState (build-time default + per-region). (2) Move the GIL
onto RuntimeState (doc 33 §5-P1), DELETE `object/gil.rs` entirely, with the per-interpreter
bootstrap barrier (the Miri-clean proof doc 33 §7-P1 mandates). (3) Collapse the task-done duality:
`MoltHeader` flag is the sole done-authority in every domain; delete native-managed-task
`FutureState` (doc 28 §1.4). (4) Wire the multiprocessing spawn superpower (doc 33 §4-c): the
`__molt_spawn` re-exec entry + entry-point manifest + pickle5/shm transfer — consuming T1's
`Sharable` for the zero-copy argument path.
**Composes with:** doc 33 P1 (this IS the one-GIL + spawn arc, now with SchedulerDomain as the
selecting fact instead of two statics), doc 28 §1.4.
**Gate:** exactly one GIL type + one task-done authority (grep + verify obligation); the
per-interpreter GIL barrier is Miri-clean (re-run doc 33's referenced cross-test race); doc 33's
`bench_processpool_startup` shows 10×+ vs CPython (the AOT superpower headline); `bench_interp_pool`
≥ CPython 3.14; full suite green. **Owner-lane:** A (the GIL move is load-bearing safety) feeding B
(the spawn superpower is throughput). **HIGH RISK** (doc 33 §7-P1's flagged riskiest phase — the GIL
move). **LoC ~2000-3000.**

### Phase T4 — the `FreeThreaded` domain (unleashed: BRC + per-object-lock-on-Shared + mimalloc)
**Why last:** it depends on T1 (IsolationClass tells BRC/locks which values are confined vs shared),
T3 (the FreeThreaded SchedulerDomain), AND doc 27 (Perceus borrow inference — the tax repayment).
This is doc 33 P3, now *gated on the facts that make it safe and fast*.
**Scope:** doc 33 P3's mechanism (biased RC two-field layout, 1-byte `ob_mutex`, mimalloc 3-heap
page-sequence, immortal objects, the `CrossThreadBorrow` lattice fact, deopt-counter atomicization)
— but **driven by IsolationClass**: BRC owner-local for `ThreadConfined`, atomic-shared + ob_mutex
for `Shared`, no-op for `Immortal`. The per-object lock is applied *exactly where IsolationClass =
Shared*, nowhere else — this is the structural reason molt's per-object-lock tax is below CPython's
(CPython locks every container; molt locks only proven-`Shared` ones).
**Composes with:** doc 33 P3 (the mechanism), doc 27 (`CrossThreadBorrow` extends the borrow
lattice; intra-thread borrows stay elided in both tiers — doc 33 §3.3), doc 26 (R-genfuse×tls).
**Gate:** doc 33 P3 gates — `bench_ft_singlethread_tax` < CPython 3.13t's 5-8% (the §3.3 thesis,
measured); `bench_par_scaling` > CPython 3.13t to 8 cores; loom/TSan clean on the optimistic-read
path; `MOLT_ASSERT_NO_LEAK` + `MOLT_ASSERT_ISOLATION` clean under multi-thread. **HIGHEST RISK.**
**LoC ~3000-5000.** **Gated on doc 27 + the ownership-lattice arc landing.**

### Phase T5 — per-target completion (WASM Workers-as-isolates, Luau host-Actors, wasi-threads)
Doc 33 P4, consuming the facts: each target's isolate transfer consumes `Sharable`; the WASM
SharedArrayBuffer tier applies the `FreeThreaded` domain only where COOP/COEP isolation exists.
**Gate:** per-target matrix (doc 33 §5) green; a native opt that doesn't survive to WASM is an
IR-fact gap (doc 51 §2), so a fused/elided await must fuse/elide identically on WASM. **LoC
~2000-3000.** **Lane:** C feeding B.

**Phase independence & sequencing rationale:** T0 (effect) and T1 (isolation) are pure fact-plane
additions that retire parity/safety classes with *zero* free-threading risk and are independently
valuable (T0 fixes the blocking-IO parity bug; T1 makes the memory model structural). T2 delivers the
async throughput headline on the GIL tier alone (no free-threading needed — most async workloads are
single-threaded). T3 delivers the multiprocessing superpower and the one-authority convergence. Only
T4 incurs free-threading complexity, and it is gated on the facts (T1) and the tax-repayment (doc 27)
that make it safe and fast. **The first three phases deliver the bulk of the throughput value before
any free-threading risk** — the correct sequencing for a perf-contract language where most concurrent
workloads are async-single-thread, process-parallel, or isolate-parallel.

---

## 6. Composition with the decomposition (21a-e) and the multi-agent model

- **God-file decomposition (doc 21, STRUCTURAL_AUDIT_BOARD).** Three of this arc's primary files are
  on the god-file ratchet: `scheduler.rs` (153KB), `channels.rs` (121KB), `sockets.rs` (271KB —
  the largest in the tree), plus `src/molt/stdlib/asyncio/__init__.py` (7188 lines) and
  `concurrency/locks.rs` (116KB). **The fact-plane work must land its NEW analysis files
  (`isolation_class.rs`, `await_graph.rs`) as cohesive new modules and must NOT add lines to the
  god-files** — the consumer changes in scheduler.rs/channels.rs should be *net-neutral-or-negative*
  on line count by routing through the fact (the council's "decompose, don't re-pin" rule, MEMORY
  structural-debt note). Specifically: T2's elision DELETES task-creation code paths (Elided tasks
  have no path), and T3's convergence DELETES `object/gil.rs` (20KB) and native FutureState — both
  are *deletions* that help the ratchet. Sequence the async-lowering mixin extraction (doc 21c
  frontend mixin decomposition) so `visit_Async*`/`visit_Await` become a cohesive `AsyncLoweringMixin`
  that is the natural home for the T0/T2 frontend producers.
- **Crate graph (doc 21b).** The async runtime and concurrency modules are candidates for a
  `molt-runtime-async` / `molt-runtime-concurrency` crate split; the fact-plane *analyses* live in
  `molt-tir` (the IR crate), so the producer/consumer split is already crate-clean (analysis in tir,
  mechanism in runtime). Keep the IsolationClass/ConcurrencyEffect *types* in the shared facts crate
  so both tir and runtime read one definition (doc 49 "no second authority").
- **Multi-agent execution model (doc 52 §Resources, council three-lane).** Map phases to the
  non-overlapping lanes: T0/T1/T3-GIL-move are **Lane A** (parity/safety — the IsolationClass keystone
  and the GIL convergence are memory-model-load-bearing); T2/T3-spawn/T4-perf-gates are **Lane B**
  (throughput frontier); the god-file decomposition + the new scoreboards (§7) are **Lane C**. The
  lanes touch non-overlapping files: Lane A in `isolation_class.rs` + `concurrency/` + `verify.rs`;
  Lane B in `await_graph.rs` + `scheduler.rs` + frontend async mixin; Lane C in the extracted modules
  + `tools/`. **A blocks B only where memory unsafety would make throughput numbers untrustworthy**
  (T4 perf is meaningless until T1's isolation is sound) — the exact council rule. ≤3 agents, ≤2
  build-triggering, each with its own `MOLT_SESSION_ID` (CLAUDE.md). The lead integrates serially.

---

## 7. Verification / gates per phase (measurement discipline)

Per the Performance Constitution and doc 52's honesty protocol, every phase reports the full matrix
and the perf claim is quiescent/repeated/attributed/classified (GREEN/RED_STABLE/RED_NOISY/TIE/
DIMENSIONAL_WIN).

- **Parity oracle (the un-gameable gate).** Every phase: byte-identical stdout vs system CPython on
  the differential async/threading corpus, on every target × profile. The corpus must be EXTENDED
  per phase with the new class's witnesses: T0 → blocking-IO-release + a GIL-held-blocking negative
  test; T1 → cross-boundary-transfer (pickle vs error, vs CPython multiprocessing); T2 → immediate-
  await elision + a real-suspension non-elision negative test + structured-concurrency leak/join; T3
  → one-authority grep + spawn-roundtrip; T4 → loom/TSan model-check + the memory-model DEFINED/UB
  cases (doc 33 §3.5). Randomized differential sampling (doc 52 §C.1-4) so no fixed target is
  memorized.
- **New CI-gated scoreboards (the throughput dimension).** Add to the four+1 scoreboards (doc 51 §3):
  a **concurrency scoreboard** with the doc-28 §1.5 + doc-33 §6 benches (`bench_async_sleep0`,
  `bench_async_echo_server` p50/p99, `bench_async_spawn_100k`, `bench_async_timer_churn`,
  `bench_gil_fairness`, `bench_blocking_io_release`, `bench_lock_uncontended/contended`,
  `bench_processpool_startup`, `bench_interp_pool`, `bench_par_scaling`, `bench_ft_singlethread_tax`).
  Each row: benchmark → target → backend → profile → CPython ratio → PyPy ratio (uvloop where
  applicable) → Go ratio (goroutines, for `bench_par_scaling`) → RSS → binary size → compile time →
  log artifact. Any row < 1.00× CPython is RED and blocks. The two headline gates (doc 33 §6):
  `bench_processpool_startup` (10×+ superpower) and `bench_ft_singlethread_tax` (molt's nogil tax <
  CPython's). NEW headline this arc adds: `bench_async_spawn_100k` task-elision (the fuse/elide
  thesis — must show ~0 tasks for immediate coroutines AND a warm-time win, not just the dimensional
  alloc win).
- **Fact-plane validators (checkable obligations, #75).** Each fact ships its Alive2-style
  obligation: IsolationClass join is monotone + transfer-op match is exhaustive; ConcurrencyEffect
  join is monotone + the blocking-GIL verify obligation; AwaitFact `Fused` never crosses a thread
  boundary (consumes IsolationClass); SchedulerDomain selects exactly one GIL/done authority. Plus
  the round-trip test at every serialization boundary (the landmine: facts die silently at
  boundaries — test the fact AT the consumer, doc 52 §B-loop-4).
- **Memory-safety gates (Lane A).** `MOLT_ASSERT_NO_LEAK` + the new `MOLT_ASSERT_ISOLATION` clean on
  the RC/finalizer/concurrency corpus under safe_run caps (CLAUDE.md — never run a raw binary). T4
  adds loom/TSan on the optimistic-read + BRC merge-queue paths.
- **Omitted-gate honesty (binding).** Every phase report lists any gate not run with its reason;
  never imply an unrun gate is green (doc 52 §honesty).

---

## 8. Risks + structural (not band-aid) treatment

| # | Risk | Structural treatment (no band-aid) |
|---|---|---|
| R1 | **IsolationClass is unsound (a `ThreadConfined` value actually escapes uncaught).** A false-`ThreadConfined` under the unleashed tier is a data race = memory unsafety. | Fail-closed lattice: the JOIN defaults to `Shared` (the safe-but-locked class) on ANY uncertainty; `ThreadConfined` is asserted only where the ownership lattice *proves* non-escape. The `MOLT_ASSERT_ISOLATION` owning-tid stamp catches a forged confinement at runtime in debug. **IsolationClass is gated on the ownership-lattice arc (doc 55 family) being sound — do not build T4 on an unproven T1.** This is the council's "no rushed memory surgery" + "gated on the lattice" rule. |
| R2 | **Suspension elision elides a real suspension** (a `Sync`-classified callee actually suspends). Silent wrong async behavior. | `ConcurrencyEffect` is fail-closed UP: a callee of *unknown* effect is `MaySuspend` (the conservative top), never elided. Elision fires only on a *proven* `≤ Sync` effect. The negative test (a coroutine that does suspend is NOT elided) is a standing differential. **The fact is proven, not assumed — same discipline as Repr-precision boxing.** |
| R3 | **The GIL move (T3) introduces a pre-init race** (doc 33 §7-P1's flagged risk). | The per-interpreter bootstrap barrier publishes the interpreter's GIL before any second thread can observe it; gated behind a Miri-clean proof (re-run the cross-test race doc 33 §1.2's gil.rs:160 comment references) BEFORE the spawn work builds on it. **If it cannot be made Miri-clean in a session, leave the baton and land nothing** (CLAUDE.md no-half-arc). |
| R4 | **Facts die at the serialization boundary** (the documented landmine — doc 52). | Every fact family has a round-trip test through `serialization.py` (modified at HEAD — a live boundary) asserting the fact survives lower→serialize→re-lift, tested AT the consumer, not at the producer. A fact-graph drift detector (doc 52 §C.3-13) is the standing institution. |
| R5 | **The fact plane adds compile-time cost** (four new analyses). | Work-budget discipline (doc 52 §C.2-11 / #73): budgets in op counts, never wall-clock. The analyses are AnalysisManager-cached (doc 00 §S1) and run once; IsolationClass/ConcurrencyEffect reuse the existing Alias/Escape/IP-summary results (they project, not recompute). dev-fast profile may compute coarser facts (more `Shared`/`MaySuspend`) trading precision for latency — but NEVER a silent runtime regression (doc 51 §2 profile invariant). |
| R6 | **Two-tier blending** (the binding directive's cardinal sin — a default program silently gets unleashed behavior). | `SchedulerDomain` is a build-time + per-region FACT, never a runtime flag (doc 33 §9 refusal #1). The default binary's default domain is `GilSerial`; `FreeThreaded` requires `--unleashed` or an explicit `@molt.unleashed` region. The validator asserts no default-tier op reads a `FreeThreaded`-only fact. |
| R7 | **`@par`/data-parallel false-disjointness** (doc 33 §4-f) — a dependent loop parallelized = silent race. | `@par` fires only on the conjunction of L4-proven disjointness (doc 04) AND every iteration's touched values being `ThreadConfined ∨ Sharable ∨ Immortal` (IsolationClass) — a `Shared`-without-lock touch fails closed to sequential. Two independent proofs must agree; fail-closed otherwise. |

---

## 9. Explicit refusals (rejected approaches, with why)

1. **REFUSED: re-deriving the memory model / GIL ladder / asyncio runtime here.** Docs 33, 28, 26
   already own those. This doc adds the *fact plane* they consume and would otherwise reconstruct
   ad-hoc. Duplicating their mechanism would be a second source of truth (doc 49 violation).
2. **REFUSED: a runtime "free-threading on/off" flag.** It blends tiers and makes every program pay
   the tax (doc 33 §9 #1). `SchedulerDomain` is a build/region fact.
3. **REFUSED: per-container locks everywhere (the naive nogil).** Locks apply only to `Shared`
   values (IsolationClass). Locking proven-`ThreadConfined` objects is the tax CPython pays and molt
   structurally avoids.
4. **REFUSED: making suspension elision a runtime fast-path check.** That is PyPy's dynamic model;
   molt proves immediacy at compile time (`ConcurrencyEffect`) — zero runtime discovery cost. A
   runtime check would re-introduce the per-await overhead the fact exists to delete.
5. **REFUSED: IsolationClass as a side-table.** It rides `Repr`/`TirType` (like boxing Repr) so it
   cannot drift from the value it describes; a side-table is exactly the boundary-death landmine.
6. **REFUSED: building T4 (free-threading) before T1 (IsolationClass) + doc 27 land.** Without the
   isolation fact the unleashed tier is unsafe; without doc 27's borrow elision the BRC tax is
   unrepaid and `bench_ft_singlethread_tax` fails the perf contract (doc 33 §9 #6).

---

## 10. Cross-arc dependency summary (for the lead's scheduling)

- **Depends on (must land first or concurrently):** the ownership-lattice / memory-safety arc
  (docs 45/48/49/50 family + the council ownership lattice — the prompt's "doc 55") for T1's escape
  fact; doc 27 (Perceus borrow inference) for T4's tax repayment; doc 28's runtime data-structure
  phases (intrusive ready list, timer wheel) as the substrate T2 prunes; the op_kinds registry
  (doc 25) for the generated `blocking` classifier; CallFacts (doc 51 §5) as T0's transport.
- **Composes with / feeds:** doc 33 (this doc is the fact layer under doc 33's whole ladder — T0=§2-b,
  T1→§3.4/§3.5, T2→§4-f, T3=§1.2+P1, T4=P3, T5=P4); doc 26 (async fusion = generator fusion on the
  await edge, shared frame-ownership; doc 51 §generator-fusion); doc 04 (L4 disjointness for @par);
  doc 05 (L2-SIMD for the @par+SIMD numeric headline); doc 44 + doc 21c (frontend async-lowering
  mixin home for the producers).
- **Unblocks:** every concurrent benchmark beating CPython/uvloop/PyPy (the concurrency scoreboard);
  the multiprocessing superpower (T3); the safe unleashed tier (T4). Demos/web-stack (doc 35)
  depend on T2 (async throughput) + T3 (process parallelism).
- **The one-line strategic claim:** *molt wins concurrency not by a faster runtime but by a
  richer representation — the four facts make races unexpressible and let the scheduler do
  compile-time work no interpreter or tracer can.*
