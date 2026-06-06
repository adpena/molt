<!--
Foundation design 33 — Threading & Parallelism Ladder. The frontier concurrency-model
design for molt. Architect: read-only research-granted agent, 2026-06-06. DESIGN ONLY;
no implementation landed. This doc number (33) was reserved by the supervisor in the
doc-29 remapping note (29 §header: "its 'Doc 28 Threading' -> slot 33"). Do not renumber.

All file:line anchors verified against the live worktree at HEAD commit
bd0b76d3180a94952971f82a3473bfa579225d00 (branch main, 2026-06-06). The doc-29
SUBSYSTEM 1 audit was written at 951938075; every claim it makes is re-verified here
against HEAD with fresh anchors, and three divergences from that audit are flagged
inline (§2-b: io.rs holds the GIL across blocking file reads; §2-e: the locks use
std Mutex+Condvar not parking_lot; §5-P1: the two-GIL convergence target).

Research provenance (RESEARCH GRANT, standing): PEP 703, PEP 734, the Choi/Shull/
Torrellas BRC paper (PACT'18), the Reinking et al. Perceus paper (PLDI'21, via doc 27),
rayon/crossbeam, mimalloc. Cited inline. License discipline: study + reimplement;
PSF-licensed CPython is a semantics reference only; no GPL code ingested.
-->

# Threading & Parallelism Ladder (Design 33)

**Document status:** Implementation-ready frontier design. **MM/concurrency-ladder root doc.**
**Scope:** The complete parallelism story for molt across all targets (native
macOS/Linux, WASM-browser, WASI, Luau) and all profiles. Defines the
**two-tier concurrency model** (DEFAULT = byte-identical CPython-GIL parity;
UNLEASHED = explicit opt-in free-threading/parallel), the six-rung ladder, the
per-target matrix, the benchmark gates, the phased build plan with deletions, and
the composition risks against the RC substrate (designs 20/27), the asyncio
runtime (design 28), and generator fusion (design 26).

**This doc decides the load-bearing question doc 29 §SUBSYSTEM 1 raised and left
open:** *what does molt's memory model commit to for concurrent execution?* The
answer (§1): **per-interpreter-GIL is the spine; free-threading is an opt-in tier
layered on biased reference counting whose nogil tax is structurally repaid by the
Perceus borrow inference of design 27.** molt does not choose between GIL-removal
and message-passing — it ships both as named tiers, and the AOT re-exec advantage
makes the message-passing tiers (multiprocessing/subinterpreters) a *superpower*,
not a consolation.

---

## 0. The binding directive, restated as the engineering target

> *Two named tiers, never blended. DEFAULT = exact CPython ≥3.12 under the GIL
> (the turn-blocking parity contract). UNLEASHED = explicit opt-in. Every
> unleashed deviation documents exactly which parity guarantee it trades.*

This is the same dual-contract spine design 28 §2.8 established for asyncio
(CPython cancel semantics = default; Trio-strict = opt-in) and design 27 §0 made
structural for RC (correctness floor = byte-identical; elision-to-zero = the
target). This doc applies it to parallelism. The engineering target is **not**
"remove the GIL." It is:

1. **DEFAULT tier**: the GIL-serialized model is *byte-identical to CPython* on
   every observable surface (thread scheduling fairness via a switch interval,
   `threading.local`, daemon-thread teardown, signal delivery on the main thread,
   `__del__` ordering). Every gap to that (§2) is a bug to close, not a deviation
   to permit.
2. **UNLEASHED tier**: a value the developer *opts into* per build (`molt build
   --unleashed`) or per region (`@molt.unleashed`/`@par`), where molt trades a
   *named* parity guarantee for parallelism, and the trade is documented in a
   per-deviation table (§1.4). The unleashed tier never silently changes a
   default-tier program's behavior.

The temptation this doc must reject (per CLAUDE.md): *"ship a 'mostly works'
free-threading mode behind a flag and call it done."* No. The unleashed tier is
shippable only when its memory model is **precisely specified** (§3.5: what races
are defined vs UB) and its RC substrate is **sound without the GIL** (biased RC,
§3.2). A free-threading tier that races dict internals into UB is not an unleashed
tier — it is a memory-safety hole wearing a flag.

### 0.1 Where this rung sits in the MM ladder (composition with designs 20/27)

| Ladder | Substrate | Status | Relation to this doc |
|---|---|---|---|
| MM rung 0 | Runtime RC primitives (`dec_ref_ptr`, immortal/arena flags) | landed (design 20 §5) | the inc/dec sites this doc must make atomic-or-biased under unleashed |
| MM rung 1 | `DropInsertion` (insert DecRef at last use) | landed; active LLVM/WASM/Luau | the pass whose *output volume* sets the atomic-RC tax (§3.3) |
| MM rung 2 | Perceus borrow inference (Owned/Borrowed lattice) | design 27, design-only | **the tax repayment**: a `Borrowed` value gets no dup/drop, so it generates *zero* atomic-RC traffic under unleashed (§3.3) |
| **Concurrency ladder** | **two-tier model + 6 rungs (this doc)** | **this design** | **here** |

The single most important cross-design claim, stated once and proven in §3.3:
**Perceus borrow inference (design 27) is the structural prerequisite that makes
molt's free-threading tier *faster* than CPython 3.13t's, not merely as fast.**
CPython pays biased-RC's local-counter write on every `Py_INCREF`/`Py_DECREF`
because it cannot statically prove a reference is borrowed. molt's design-27
lattice proves exactly that, at compile time, and emits **no RC operation at all**
for borrowed values. The nogil tax CPython measures at 5–8% (PEP 703 Performance
table, Skylake 6% / Zen3 5% single-thread) is levied per surviving RC op; design
27 deletes the majority of those ops before they exist. This is the thesis.

---

## 1. THE MODEL: per-interpreter-GIL spine + opt-in free-threading tier

### 1.1 The decision (the question doc 29 left open)

molt commits to a **three-layer concurrency model**, layered, not blended:

```
Layer A (DEFAULT):   one GIL per interpreter (isolate).  Threads within an
                     interpreter are GIL-serialized — byte-identical to CPython.
                     This is today's model (concurrency/gil.rs PREINIT_GIL),
                     corrected to true per-interpreter scope (§1.2).

Layer B (DEFAULT):   N interpreters (isolates), each with its own GIL + heap,
                     run in true parallel.  Communication is by message passing
                     (channels / pickled buffers).  This is PEP 734 fidelity and
                     is molt's AOT superpower (re-exec = ~4ms, §4-c/§4-d).

Layer C (UNLEASHED): within ONE interpreter, free-threading — the GIL is gone,
                     RC is biased (§3.2), containers carry per-object locks
                     (§3.4), the memory model is the one in §3.5.  Opt-in only.
```

This is the **correct frontier for a compiled AOT system** (doc 29 §43 anticipated
this: "the correct frontier is not GIL removal in the CPython sense"). The reason
layering wins over a single choice:

- A pure free-threading model (CPython 3.13t) pays the nogil tax on *every*
  program, including the 99% that are single-threaded or use processes. Wrong
  default for a perf-contract language.
- A pure message-passing model (Go-style, or pure PEP 734) cannot express
  shared-memory data parallelism (a 10GB array two threads both read) without a
  copy. Wrong for the ML/data-science domain.
- Layering gives each workload its cheapest correct substrate: default GIL for
  parity, isolates for shared-nothing parallelism (the common server/pool case),
  free-threading for the rare genuine shared-mutable-parallel case.

### 1.2 Layer A precisely: per-interpreter GIL, ONE authority

Today there are **two** GIL implementations (doc 29 §21, re-verified at HEAD):

- **Authoritative**: `runtime/molt-runtime/src/concurrency/gil.rs` — a single
  `static PREINIT_GIL: Mutex<()>` (`gil.rs:177`), per-thread depth via
  `GIL_DEPTH` TLS, a single-thread fastpath gated on `GIL_THREAD_COUNT <= 1`
  (`gil.rs:209`), a TLS-destruction fallback (`GIL_FALLBACK_OWNER`/`DEPTH`,
  `gil.rs:420-422`), and `GilReleaseGuard` (`gil.rs:320`) that saves/restores
  depth across blocking sections. **One process-global mutex.** The header comment
  (`gil.rs:160-175`) documents *why* it is process-global, not per-state: an
  earlier per-state design produced a Miri-caught data race when a pre-init thread
  and a post-init thread took *different* mutexes.

- **Stub**: `runtime/molt-runtime/src/object/gil.rs` — a Phase-1 GIL-removal
  scaffold: `GIL_RELEASED: AtomicBool` (`object/gil.rs:30`) for a future `@par`
  mode, an `ObjectLock` (`object/gil.rs:10`, a per-object `Mutex<()>`), and a
  `gil_check()` no-op (`object/gil.rs:51`). **Nothing in production codegen
  activates this** (verified: `is_gil_released()` has no production caller; the
  flag is never `release_gil()`'d outside tests).

**The structural defect:** today's `PREINIT_GIL` is *process-global*, but Layer B
(parallel interpreters) requires it to be *per-interpreter*. The Miri-race comment
(`gil.rs:160`) is correct that the *pre-init/post-init* split was the bug — but the
fix it chose (one process-global static) is the wrong end-state for parallel
isolates: two isolates that should run in parallel would serialize on the single
`PREINIT_GIL`. **The end-state (§5-P1): the GIL lives on the `RuntimeState`
(per-interpreter), and the pre-init synchronization gap is closed not by sharing
one static but by a per-interpreter bootstrap barrier that publishes the
interpreter's own GIL before any second thread can observe it.** The
`object/gil.rs` stub is **deleted** (its `ObjectLock` is superseded by the
real per-object lock design of §3.4, which lives on the object header, not a
free-standing `Mutex`; its `GIL_RELEASED` flag is superseded by the per-interpreter
"free-threaded mode" bit on `RuntimeState`). **There must be exactly one GIL
authority after P1.** This is the doc-29 mandate ("the two-GIL split must converge
to ONE authority").

REFUSAL — *"keep both, gate object/gil.rs behind a feature so we don't have to
delete it":* rejected. Two coexisting GIL types is precisely the dual-source-of-
truth the binding directive forbids; `object/gil.rs`'s `ObjectLock` as a
free-standing `Mutex<()>` (8+ bytes, separate allocation) is also the *wrong data
structure* for per-object locking (§3.4 uses a 1-byte header field à la PEP 703
`ob_mutex`). Deletion, not feature-gating.

### 1.3 Layer B precisely: isolates already exist; channels are the gap

`concurrency/isolates.rs` (re-verified at HEAD) already spawns real OS threads
with **independent runtime states**: `thread_main` (`isolates.rs:422`) allocates a
fresh `RuntimeState` (`isolates.rs:425`), sets it as the thread's state
(`set_thread_runtime_state`, `isolates.rs:427`), runs `molt_isolate_bootstrap`
(`isolates.rs:437`), and tears it down on exit (`runtime_teardown_isolate`,
`isolates.rs:446`). This is **already a per-interpreter heap+state model** — it is
PEP 734's interpreter, minus the channel protocol and minus the per-interpreter
GIL (today all isolates share `PREINIT_GIL`, which §1.2/§5-P1 fixes).

Note the `molt_thread_spawn` default (`isolates.rs:519`): it defaults to
**shared-runtime** threads (`thread_main_shared`, `isolates.rs:456`, which reuses
the parent's `state_ptr`) for CPython parity of thread-visible global/module state,
with `MOLT_THREAD_ISOLATED` (`isolates.rs:511`) as the escape hatch to the
isolate-per-thread mode. This is correct: `threading.Thread` must see shared module
globals (Layer A); the isolate mode (Layer B) is for `InterpreterPoolExecutor`.
**The gap for Layer B is the PEP 734 channel/`Queue` protocol** (§ rung-d): the
`_interpreters.py`/`_interpchannels.py` stubs (doc 29 §37) need a real
sharable-object queue backed by a Rust MPMC channel that *copies* (pickles) or
*moves* (sharable buffers) objects across the heap boundary.

### 1.4 The parity-required vs unleashed-eligible boundary (THE mandated section)

This is the boundary section the user mandates (the doc-27 §3 / doc-28 §3
template). Every row is a concrete observable; the column says whether it is
**fixed by parity** (default tier must match CPython exactly) or **eligible for an
unleashed trade** (and if traded, what guarantee is surrendered).

| # | Observable surface | DEFAULT-tier contract (parity-required) | UNLEASHED-tier eligibility (what it trades) |
|---|---|---|---|
| B1 | **Bytecode-step atomicity** (`L.append(x)` is one indivisible step; `i += 1` on a shared int is *not*, exactly as CPython) | byte-identical: the GIL makes each bytecode/intrinsic boundary a serialization point; observable interleavings match CPython | **traded.** Under free-threading, only *individual container ops* are atomic (per-object lock, §3.4). Multi-op sequences (`d[k] += 1`) are NOT atomic — same hazard CPython 3.13t has. Documented: "compound ops need explicit locks under unleashed." |
| B2 | **`threading.local` semantics** | per-thread isolation, value dies with thread (§2-f: today's impl has a teardown-timing gap to close) | unchanged — TLS is *more* natural under free-threading; no trade |
| B3 | **GIL switch interval / fairness** (`sys.setswitchinterval`, default 5ms; a CPU-bound thread yields so others progress) | byte-identical: molt must implement a real switch interval (§2-a: TODAY MISSING — this is the largest DEFAULT-tier gap) | **N/A** — free-threading has no GIL to switch; threads run on real cores. The switch interval becomes a no-op (documented). |
| B4 | **Object identity & `id()` across threads** | stable (GIL-serialized allocation) | unchanged — `id()` is the address; biased RC does not move objects. No trade. |
| B5 | **`__del__` / finalizer timing & thread** | runs at RC-zero on the thread that drops the last ref, GIL-held (parity) | **traded.** Under free-threading, the dropping thread may differ and `__del__` may run *concurrently* with other threads' code (CPython 3.13t has the same property). The finalizer-ordering guarantee within a thread is kept; cross-thread ordering is surrendered. |
| B6 | **Signal delivery** (handler runs on the **main thread** only, between bytecodes, via a pending flag) | byte-identical: §2-d audits the signal path; the pending-flag-checked-by-main-thread model must hold | unchanged — signals are still main-thread-only under free-threading (CPython keeps this). No trade. |
| B7 | **Module import lock** (one thread imports a module; others block until done — `importlib._bootstrap._ModuleLock`) | byte-identical per-module lock | unchanged — per-module lock is *more* necessary under free-threading; no trade |
| B8 | **Dict/list iteration vs concurrent mutation** | `RuntimeError: dict changed size during iteration` (single-thread parity); cross-thread under GIL cannot interleave mid-bytecode | **traded.** Under free-threading, a concurrent writer can be observed by an optimistic reader (§3.4/§3.5): the read either sees a consistent old-or-new element or retries — never a crash, but the "changed size" RuntimeError is best-effort, not guaranteed mid-iteration. |
| B9 | **Refcount value visible to `sys.getrefcount`** | exact (GIL-serialized) | **traded.** Biased RC splits the count into local+shared (§3.2); `sys.getrefcount` returns the *sum*, which may transiently differ from CPython's single counter by in-flight cross-thread decrements. CPython 3.13t has the identical caveat. |
| B10 | **`gc` collection determinism** (cycle collection timing) | parity (RC is primary; cycles deferred per design 20 §10.1 / 27 §4.6) | unchanged in *what* leaks (cycles still leak without the rung-3 collector); free-threading changes *when* the collector could run (stop-the-world vs concurrent) — deferred with the collector. |

**The contract in one line:** the default tier surrenders **nothing**; the
unleashed tier surrenders exactly {B1 compound-atomicity, B5 cross-thread
finalizer ordering, B8 iteration-vs-mutation RuntimeError guarantee, B9 refcount
exactness} — *and these are precisely the four guarantees CPython 3.13t also
surrenders.* molt's unleashed tier is therefore **semantically equal to CPython
free-threading**, not a weaker thing. That equality is the parity claim for the
unleashed tier (you get CPython-3.13t semantics, not molt-specific surprises).

---

## 2. RUNG (a)–(b): threading under the GIL — the DEFAULT-tier gap audit

The default tier is *mostly* built (doc 29 §29-31: ~55 intrinsics, real OS
threads, `ThreadPoolExecutor`). This section audits **what is missing vs CPython**
under the GIL — these are bugs against the parity contract, scored IMPORTANCE×GAP
(doc 26-32 house scale, 1–3 each).

### 2-a. GIL fairness / switch-interval semantics — IMPORTANCE 3 × GAP 3 = **9 (CRITICAL)**

**CPython model:** the GIL is not just a mutex — it has a **switch interval**
(`sys.setswitchinterval`, default 5ms). A CPU-bound thread holding the GIL is
asked to drop it after the interval (via `eval_breaker` / `gil_drop_request`) so
another runnable thread can acquire it. Without this, a tight-loop thread starves
all others.

**molt at HEAD:** `concurrency/gil.rs` is a plain `Mutex<()>` with **no switch
interval**. A molt thread running a tight Python loop holds `PREINIT_GIL` and
never voluntarily yields — the only release points are explicit `GilReleaseGuard`
blocking-IO sections (§2-b) and the (rare) re-acquire on a fresh `GilGuard::new()`
after a full release. **Two CPU-bound `threading.Thread`s in molt do not
fair-share; one runs to completion, then the other.** This is the single largest
default-tier parity break. CPython programs that rely on cooperative progress of
multiple CPU-bound threads (rare but real — e.g., a watchdog thread) will hang or
starve under molt.

**Fix (§5-P0):** a real switch-interval. The compiler already emits periodic
safepoint checks for other reasons (the async `eval_breaker`-equivalent); thread
the GIL-drop-request through the same safepoint. On the safepoint, if
`now - gil_acquired_at > switch_interval` AND another thread is waiting, the
holding thread drops and immediately re-requests the GIL (round-robin via a
ticket/`Condvar` fairness queue, replacing the bare `Mutex` with a fair lock).
This is a **default-tier** fix — it must land for parity regardless of the
unleashed tier.

### 2-b. Blocking-IO GIL-release coverage — IMPORTANCE 3 × GAP 2 = **6 (HIGH)**

**The contract:** every blocking syscall must release the GIL so other threads run
during the wait (CPython's `Py_BEGIN_ALLOW_THREADS`). molt's mechanism is
`GilReleaseGuard` (`gil.rs:320`). **Audit at HEAD** (16 files hold call sites;
grep-verified):

COVERED (release the GIL): `time.sleep` (`object/ops.rs:2585`, confirmed —
`GilReleaseGuard::new()` before `thread::sleep`), socket I/O
(`async_rt/sockets.rs:2030`), all async channels (`async_rt/channels.rs` ×8),
io_poller (`io_poller.rs:940,944`), HTTP (`functions_http.rs:3382`,
`runtime-http`), `select` (`builtins/select.rs:552,1144`), all lock/condition/
semaphore/queue waits (`concurrency/locks.rs` ×11 — correct, blocking lock
acquisition must release the GIL), `thread.join` (`isolates.rs:651`), tkinter
(`runtime-tk` ×7), the C-API blocking shim (`c_api/molt_api.rs:1538`).

**NOT COVERED (hold the GIL across a blocking syscall) — the gaps:**

1. **`io.rs` plain file I/O holds the GIL.** `runtime/molt-runtime/src/builtins/
   io.rs` has **zero** `GilReleaseGuard` (grep-confirmed: `grep -c GilReleaseGuard
   io.rs` = 0). `molt_file_read` (`io.rs:3777`), `molt_file_readall`
   (`io.rs:4243`), `molt_file_write` (`io.rs:6432`), `molt_file_writelines`
   (`io.rs:6608`) all call `std::io::Read`/`Write` **with the GIL held**. A thread
   blocked on a slow disk/pipe read (`f.read()` on a FIFO, a network filesystem)
   **freezes every other Python thread** for the duration. This is a real
   parity break: CPython releases the GIL around `read()`/`write()`.
   **This corrects/refines doc 29's audit, which did not enumerate io.rs.**
2. **`subprocess` holds the GIL.** `builtins/subprocess_ext.rs` has zero
   `GilReleaseGuard` (grep-confirmed). A blocking `proc.wait()` / `communicate()`
   that does not route through the async `process.rs` path holds the GIL.

**Fix (§5-P0):** wrap the blocking `read`/`write`/`wait` calls in `io.rs` and
`subprocess_ext.rs` with `GilReleaseGuard`. Default-tier, mechanical, but must be
*complete* (every blocking syscall, not 80% — the binding directive). Add a
`debug_assert!(!gil_held())`-style audit harness that fails CI if a known-blocking
intrinsic is reached with the GIL held (a structural guard against regression,
mirroring design 27's `MOLT_ASSERT_NO_LEAK`).

### 2-c. Daemon threads & interpreter shutdown — IMPORTANCE 2 × GAP 2 = **4 (MEDIUM)**

**CPython model:** daemon threads are killed (not joined) at interpreter exit;
non-daemon threads are joined. The `ThreadRegistry` (`isolates.rs:118-135`) tracks
`daemon: bool` per entry (`isolates.rs:121`) — so the *bookkeeping* exists. **The
gap:** the shutdown path. At `molt_runtime_shutdown`, does molt (a) join all
non-daemon threads, and (b) abandon daemon threads? The registry tracks daemons
but the shutdown-ordering contract (join non-daemons, drop daemons, run no further
Python on daemons after main exits) needs an explicit shutdown sequence — analogous
to the asyncio shutdown sequence design 28 §2.8 specifies (cancel→drain→close).
**Fix (§5-P1):** a `threading._shutdown()`-equivalent that walks the registry,
joins non-daemons (with the GIL released during join), and marks daemons abandoned.

### 2-d. Signal handling (main-thread-only) — IMPORTANCE 2 × GAP 2 = **4 (MEDIUM)**

**CPython model:** signal handlers run **only on the main thread**, between
bytecode boundaries, via a pending-call flag checked by the eval loop. A handler
installed via `signal.signal` is invoked from the main thread even if the signal
was delivered to another thread.

**molt at HEAD:** `builtins/signal_ext.rs` installs an OS handler
(`install_os_handler`, called from `molt_signal_signal`, `signal_ext.rs:296`) and
stores the Python handler bits on `RuntimeState.signal` (`signal_ext.rs:336`). The
gap to audit: (1) is the Python handler **deferred to a main-thread safepoint**
(CPython model) or run directly in the async-signal context (unsafe — cannot run
Python from a signal handler)? The presence of `set_wakeup_fd`
(`signal_ext.rs:461`) suggests the self-pipe / pending-flag model is intended.
(2) Does molt enforce that `signal.signal` raises `ValueError` when called off the
main thread (CPython's contract)? **Fix (§5-P1):** confirm/implement the
pending-flag-on-main-thread model: the OS handler sets an atomic pending bit (+
wakeup fd write), and the *main thread's* safepoint (same safepoint as §2-a)
checks the bit and runs the Python handler with the GIL held. Off-main-thread
`signal.signal` → `ValueError`. This composes with §2-a's safepoint (one safepoint
mechanism serves switch-interval, signals, and async eval_breaker).

### 2-e. Lock primitive throughput (uncontended) — IMPORTANCE 2 × GAP 2 = **4 (MEDIUM)**

**molt at HEAD:** every threading primitive uses `std::sync::Mutex<State> +
Condvar` (`concurrency/locks.rs:31-32,114-115,128-129,...` — Lock, RLock,
Condition, Event, Semaphore, Barrier, Queue all `Mutex<…> + Condvar`).
`std::sync::Mutex` on Linux is a futex (fast uncontended), but on macOS pre-Rust-
1.62-runtime semantics it could be a pthread mutex (heavier). **This refines doc
29's audit, which did not characterize the lock primitive.** The default-tier
parity is *correct* (a `threading.Lock` blocks correctly); the *throughput* gap is
that an uncontended `lock.acquire()/release()` pair does a full Mutex lock/unlock
where CPython 3.12's `_thread.lock` uses a single atomic CAS on the fast path.
**Fix (perf, not parity, §5-P2):** an uncontended fast path — an `AtomicBool`
(or the §3.4 1-byte `ob_mutex`) CAS for the *uncontended* acquire, falling to the
`Mutex+Condvar` slow path only on contention. This is the same optimistic
structure PEP 703 uses for `ob_mutex` (PEP 703, "Optimistically Avoiding
Locking"), so the lock fast-path and the §3.4 container lock share one
implementation.

### 2-f. `threading.local` correctness — IMPORTANCE 2 × GAP 2 = **4 (MEDIUM)**

**molt at HEAD (verified, `locks.rs:1818-1880`):** `molt_local_new` allocates a
`MoltLocal` whose state is a `Mutex<HashMap<u64, u64>>` — a map from
`current_thread_id()` (the monotonic counter, `concurrency/mod.rs:23`) to a
per-thread dict's bits. `molt_local_get_dict` (`locks.rs:1828`) looks up the
current tid, creating the dict lazily. **This is NOT real OS TLS — it is a
tid-keyed shared map.** Two parity gaps:

1. **Teardown timing.** A real `threading.local`'s per-thread dict is destroyed
   when the *thread* dies (its `__dict__` is dropped, running any value
   finalizers). molt's `MoltLocal` only frees entries at `molt_local_drop`
   (`locks.rs:1865`, when the `local` *object* dies) — so a dead thread's
   thread-local values **persist** (and their `__del__`s do not run) until the
   `local` object itself is collected. A thread that creates thread-local state,
   stores an object with a `__del__`, and exits, leaks that object's finalizer
   past the thread's death. CPython runs it at thread death.
2. **tid reuse.** `current_thread_id()` is a monotonic `AtomicU64` counter
   (`mod.rs:23`, `fetch_add`) — it never reuses, so #2 (a *new* thread seeing a
   *dead* thread's local dict via tid collision) cannot happen. Good — this is
   *more* correct than an OS-tid-keyed map. The counter design is sound; only the
   teardown (#1) is the gap.

**Fix (§5-P1):** hook the thread-teardown path (`isolates.rs:448`
`clear_thread_runtime_state` / the isolate teardown) to walk all live `MoltLocal`
objects and drop *this thread's* entry, running finalizers — OR (cleaner) invert
the ownership: store thread-locals in the per-thread `RuntimeState` (which is
already torn down per-thread at `isolates.rs:446`) keyed by `local`-object
identity, so thread death drops them for free. The inverted design is structurally
superior (thread death already tears down `RuntimeState`) and is the chosen fix.

### 2-g. Summary scoreboard (default-tier gaps)

| Gap | Score | Tier | Phase |
|---|---|---|---|
| 2-a GIL switch interval | **9** | DEFAULT parity | P0 |
| 2-b blocking-IO GIL release (io.rs, subprocess) | **6** | DEFAULT parity | P0 |
| 2-c daemon-thread shutdown | 4 | DEFAULT parity | P1 |
| 2-d signal main-thread model | 4 | DEFAULT parity | P1 |
| 2-e lock uncontended fast path | 4 | DEFAULT perf | P2 |
| 2-f threading.local teardown | 4 | DEFAULT parity | P1 |

**The default tier is not done.** Before any unleashed work, §2-a and §2-b must
land (they are pure parity bugs). This is the doc-29 "what's missing vs CPython"
answer, made precise with HEAD anchors.

---

## 3. RUNG (e): the UNLEASHED free-threading tier (the deep rung)

This is the deepest rung. It is opt-in (`molt build --unleashed` produces a
free-threaded binary; default builds are untouched). The design borrows PEP 703's
proven mechanisms (BRC, immortal objects, per-object locks, mimalloc page-reuse
sequencing) and **adds molt's structural advantage**: design-27 borrow inference
deletes the majority of RC traffic before it can become atomic-RC tax.

### 3.1 The nogil tax, quantified (CPython's measured cost)

PEP 703's Performance table (fetched from peps.python.org/pep-0703, "Performance"
section): single-thread overhead **6% Skylake / 5% Zen3**; multi-thread (1 thread
of a free-threaded build) **8% / 7%**. The PEP states: *"The largest contribution
to execution overhead is biased reference counting followed by per-object
locking."* The BRC paper (Choi/Shull/Torrellas, PACT'18,
iacoma.cs.uiuc.edu/iacoma-papers/pact18.pdf) reports a **7.3% average throughput
increase** of BRC over naive atomic RC — i.e., naive atomic-everywhere RC is
~7% *slower* than BRC, which is the tax BRC repays vs the naive approach.

**The two taxes, separated:**
- **Tax 1 (RC atomicity):** every surviving inc/dec must be biased (local
  non-atomic + shared atomic) instead of a plain non-atomic `+1`. BRC minimizes
  but does not eliminate this.
- **Tax 2 (per-object locking):** container mutations take a per-object lock;
  reads take the optimistic lock-free path with a retry loop.

### 3.2 Biased reference counting for molt (the RC substrate change)

molt's `MoltRefCount` today (`object/refcount.rs:17-22`, verified) is a single
`AtomicU32` on native (`Relaxed` inc, `Release` dec, `Acquire` fence at zero —
`refcount.rs:70,85,102`) and `Cell<u32>` on WASM. Under the GIL this single atomic
is correct (the doc-29 §27 note: it is for signal-handler/cpython-abi safety, not
multi-thread, because the GIL serializes). **For free-threading, a single atomic is
both insufficient (no owner-bias = full atomic tax) and the wrong shape.** The
end-state mirrors PEP 703's two-field BRC (PEP 703 "Reference Counting"):

```
MoltHeader (unleashed layout):
  ob_ref_local : u32     -- owning thread's count + 2 MSB flags (immortal/deferred)
  ob_ref_shared: usize   -- other threads' count << 2 | 2-bit state
  ob_tid       : u64     -- owning thread id (0 = unowned/merged)
```

- **Owning-thread inc/dec:** non-atomic `ob_ref_local += 1` / `-= 1` (the fast
  path — *cheaper than today's `Relaxed` atomic*, because it is a plain store).
- **Non-owning-thread inc/dec:** atomic on `ob_ref_shared` (`fetch_add`/`sub`).
- **The merge protocol (PEP 703's state machine, 4 states `0b00`..`0b11`):** when
  a non-owning thread's decrement drives `ob_ref_shared` negative, the object is
  *queued to the owner* (state `0b10` Queued) via the owner thread's merge queue,
  and the owner is notified at its next safepoint (the §2-a safepoint, reused
  again). The owner merges `local + shared` and, if zero, deallocates. State
  `0b11` Merged = the object became unowned (owner died) and uses `ob_ref_shared`
  only.

**License/provenance:** the BRC algorithm is from the Choi/Shull/Torrellas paper
(study + reimplement). The specific field layout and state encoding (`ob_ref_local`
2-MSB-flags, `ob_ref_shared` 2-LSB-state, the eval_breaker merge queue) is PEP 703
(PSF, semantics reference — reimplemented in molt's header, not copied). The
mapping onto molt's `MoltHeader` is original.

**WASM stays single-threaded** unless `wasm32-unknown-unknown` + SharedArrayBuffer
threads are the target (§ per-target matrix): the `Cell<u32>` path is correct for
single-threaded WASM and is *kept* for that target; the BRC layout is native
(+wasi-threads) only.

### 3.3 THE THESIS: Perceus (design 27) repays Tax 1 structurally

This is the load-bearing composition claim and the answer to the user's mandate
("how does Perceus drop-insertion + borrow inference REDUCE the atomic-RC tax by
eliminating RC traffic entirely?").

**CPython cannot statically prove borrows.** Its interpreter dups/drops every
reference dynamically; BRC makes those ops *cheaper* (owner-local non-atomic) but
every op still executes. CPython's 5–8% nogil tax is levied across the *full*
volume of dynamic RC ops.

**molt proves borrows at compile time (design 27).** The design-27 ownership
lattice (doc 27 §1) classifies every (alias-root, program-point) as
`Owned`/`Borrowed`/`Raw`/`MaybeUninit`. A **`Borrowed` value receives no `dup`
and no `drop` — zero RC operations** (doc 27 §0.1: "Elided to zero on
non-escaping becomes the definition of the Borrowed class"). Therefore:

```
molt unleashed RC tax  =  Tax1(biased) × (volume of SURVIVING Owned RC ops)
CPython 3.13t RC tax   =  Tax1(biased) × (volume of ALL dynamic RC ops)

and  SURVIVING_owned  ≪  ALL_dynamic   because design-27 deletes every Borrowed dup/drop.
```

Concretely (doc 27 §3.3 bench corpus): the `Borrowed`-parameter ABI (doc 27 §2.2,
"callee borrows all args, returns owned") means a function argument used read-only
generates **zero** inc/dec in molt — where CPython does an incref-on-call +
decref-on-return *per call*, biased but present. In a hot dispatch loop (the
`bench_method_default_binding` shape from the asyncio arc), that is the dominant RC
volume, and molt's is zero. **The structural consequence: molt's free-threading
tier pays biased-RC cost only on the Owned values design-27 could not elide — a
strict subset of CPython's RC volume — so molt-unleashed single-thread overhead is
*below* CPython-3.13t's 5–8%.** This is the perf-contract claim for the unleashed
tier (molt must beat CPython on every rung, every profile).

**Composition hazard (R-rc×atomic, §6):** design-27's lattice was proven sound
*under the GIL* (its dup/drop placement assumes serialized execution). Under
free-threading, a `Borrowed` value that is borrowed *across a thread boundary*
(passed to another thread, or read from a shared container) is no longer safely
borrow-elided — the owner could drop it concurrently (the PEP 703 borrowed-
reference hazard: "another thread might modify the list leading to `item` being
freed between the access and the `Py_INCREF`"). **Resolution (§3.4):** the
optimistic-read `_Py_TRY_INCREF` path *re-materializes* a borrow as a temporary
`Owned` (a conditional incref) exactly at the cross-thread read site, and the
design-27 lattice gains a fifth fact — `CrossThreadBorrow` — that forces a real
biased incref where a borrow escapes its owning thread. **This is the one place the
unleashed tier *adds* RC ops design-27 had elided** — and it is added only at
genuine cross-thread reads (rare relative to intra-thread borrows). The default
tier (GIL) keeps design-27's elision wholesale. This is the precise statement of
how the two designs compose: **intra-thread borrows stay elided in both tiers;
cross-thread borrows re-materialize an incref under unleashed only.**

### 3.4 Per-object locking for container consistency (dict/list/set)

The `object/gil.rs` `ObjectLock` stub (a free-standing `Mutex<()>`) is **deleted**
(§1.2). The real design (PEP 703 "Optimistically Avoiding Locking", reimplemented):

- **A 1-byte `ob_mutex` in `MoltHeader`** (not a separate allocation) — the same
  field §2-e's lock fast-path uses.
- **Writes** (`list.append`, `dict.__setitem__`, `set.add`, `setattr`) acquire
  `ob_mutex`. This is the per-object lock that replaces the GIL for container
  mutation under unleashed.
- **Reads** (`list.__getitem__`, `dict.__getitem__`) take PEP 703's optimistic
  lock-free path: atomically load the slot, conditional-incref (`_Py_TRY_INCREF`),
  re-validate the slot did not change, retry on conflict; fall to the locked path
  only when the optimistic read loses a race. (PEP 703 code, reimplemented.)
- **The mimalloc dependency (the deep correctness condition).** The optimistic
  read is sound *only because* the memory backing a freed element is not reused
  for an object whose refcount field lands at a different offset before the reader
  finishes. PEP 703 enforces this with **three mimalloc heaps** (non-GC / GC+dict /
  GC) and **page-reuse sequence numbers** (an empty page is reusable for a
  different heap/size-class only once the global read sequence passes the page's
  tag). **molt must adopt mimalloc with this same restricted-page-reuse policy for
  the unleashed allocator.** molt's current allocator (`object/mod.rs`
  `size_class_for`) is GIL-single-threaded-safe; the unleashed tier needs the
  mimalloc-backed, sequence-tagged allocator. (mimalloc: MIT-licensed, study +
  reimplement the page-tag policy; the allocator itself is a dependency, not
  copied code.)

**Resize under concurrent read:** a `list` resize (realloc) under a writer's
`ob_mutex` produces a new backing array; an optimistic reader holding the *old*
pointer re-validates (`ob_item != atomic_load(&a->ob_item)` → retry) and re-reads
from the new array. The old array's memory is freed only after the page-sequence
guard guarantees no reader can still observe it. This is exactly B8's "never a
crash, but RuntimeError is best-effort" contract.

### 3.5 The memory model molt commits to (precisely — what is UB vs defined)

The user mandates this be documented precisely. molt's unleashed memory model,
stated as defined-behavior vs undefined-behavior:

**DEFINED (no crash, well-specified outcome):**
1. **Two threads mutating the *same* container concurrently** (`L.append` from two
   threads): serialized by `ob_mutex`; the result is *some* interleaving of the
   two appends — both elements present, order unspecified between them. Never a
   torn write, never a crash, never a leaked/double-freed element.
2. **One thread reading while another writes the *same* container:** the reader
   observes either the pre-write or post-write value of each slot it reads (a
   *consistent* old-or-new per slot via the optimistic-incref+revalidate), never a
   half-updated pointer, never a use-after-free of a concurrently-freed element.
3. **Concurrent inc/dec of the *same* object's refcount:** biased RC (§3.2) makes
   this well-defined; the object is freed exactly once when local+shared reaches
   zero.
4. **Immortal objects** (`None`, `True`, `False`, small ints, interned strings,
   type objects): inc/dec are no-ops (PEP 703 immortal, `ob_ref_local =
   UINT32_MAX`); concurrent access is always safe, never contended.
5. **Publication via a synchronizing operation** (a `Lock`, a `Queue.put/get`, a
   container write-then-read across the same `ob_mutex`): standard
   acquire/release; the writer's prior stores are visible to the reader.

**UNDEFINED / NOT GUARANTEED (the developer must add synchronization):**
1. **Compound operations are NOT atomic** (B1): `d[k] += 1` from two threads may
   lose an update (read-modify-write is three ops; only each is locked). Same as
   CPython 3.13t. *Not UB in the Rust/memory-safety sense — no crash — but the
   logical result is unspecified.* This is a **data race at the Python semantic
   level**, defined to be "lost-update possible," not memory-unsafe.
2. **Object-field reads of *non-container* user objects without a lock**: reading
   an attribute another thread is concurrently `setattr`-ing yields old-or-new
   (the instance dict is a dict → optimistic read applies), but a sequence of
   attribute reads is not a consistent snapshot.
3. **Cross-thread `__del__` ordering** (B5): unspecified which thread runs a
   finalizer and when, relative to other threads' progress.

**The molt guarantee, stated as the contract:** *under the unleashed tier, no
data race produces memory-unsafety (no crash, no UAF, no torn pointer) — molt is
memory-safe by construction (Rust + biased RC + per-object locks + mimalloc
page-sequencing). What a data race CAN produce is a logically-unspecified Python
result (lost update, stale read, non-snapshot). This is identical to CPython
3.13t's guarantee.* molt does **not** offer the Java/Go "sequentially consistent
for race-free programs, but races have bounded outcomes" model beyond this —
because matching CPython 3.13t exactly is the parity target, and CPython's model
*is* "memory-safe, logically-racy." REFUSAL — *promising a stronger
(SC-DRF/Java-like) model:* rejected; it would diverge from CPython 3.13t, breaking
the unleashed-tier parity claim (B-table: unleashed == 3.13t).

### 3.6 Specialization / deopt under free-threading

CPython 3.13t **disables the specializing adaptive interpreter inline-cache
rewrites under free-threading initially** (the inline caches were not thread-safe;
3.14 re-enables some). molt's analogue is its **deopt/OSR skeleton** and the
inline-cache-style dispatch (the `bench_method_default_binding` devirt+deopt-guard
arc in MEMORY). **The unleashed composition hazard (R-deopt×ft, §6):** a deopt
guard that rewrites a call site's cached target is a *write to shared code state*;
under free-threading two threads hitting the same site could race the rewrite.
**Resolution:** molt's specialization is **AOT** (the guard/cache is emitted at
compile time, not mutated at runtime in the CPython sense) — so the *static* caches
are immutable and race-free by construction. The only runtime-mutable state is the
deopt *counter* (how many times a guard failed), which must become an atomic
(or per-thread, summed) under unleashed — a small, contained change. molt's
AOT-specialization is therefore *structurally friendlier* to free-threading than
CPython's runtime-mutating inline caches: there is no cache-rewrite race because
there is no runtime cache rewrite.

### 3.7 Score

UNLEASHED free-threading: IMPORTANCE 3 (web/engineering true-parallel) × GAP 3
(zero implementation, only the deleted stub) = **9**, but **gated** on (a) §2
default-tier completion, (b) design-27 landing (the tax-repayment prerequisite),
(c) the mimalloc allocator swap. This is correctly the *last* rung to build.

---

## 4. RUNG (c)–(d), (f): multiprocessing, subinterpreters, data-parallelism

### 4-c. multiprocessing / spawn — the AOT SUPERPOWER — IMPORTANCE 3 × GAP 3 = **9**

**The structural advantage (user mandate):** CPython's `multiprocessing.spawn`
re-execs `python`, which must **re-initialize the interpreter and re-import every
module** — typically 50–300ms of startup per worker. **A molt AOT binary re-execs
*itself* and is running compiled `main` in ~4ms** (MEMORY: startup measured 3.58ms
vs CPython 18.7ms; the spawn path is even more lopsided because CPython spawn pays
full import cost per worker, molt pays only the OS `fork+exec` + the binary's own
fast init). **molt's `ProcessPoolExecutor` startup is ~10–75× faster than
CPython's.** This is not an optimization — it is a category advantage that makes
process-based parallelism molt's *recommended* parallel substrate (it sidesteps
the GIL entirely with true OS-process isolation, and the startup cost that makes
CPython's `ProcessPoolExecutor` painful is gone).

**The spawn-for-AOT protocol (the novel design, doc 29 §33/§43 flagged it
undesigned):**

1. **Entry-point manifest.** The compiled binary registers a table of
   *spawn-targets*: each is a top-level function reachable as a process entry
   (the `if __name__ == "__main__"` guard, plus any function passed to
   `Process(target=fn)` / `Pool.map(fn, …)`). At compile time, molt collects the
   set of functions that can be a subprocess entry (closure-free top-level
   functions; the frontend already has the call graph for this — design 03/S4)
   and emits a `__molt_spawn_entry(idx, payload_fd)` dispatcher.
2. **Re-exec.** `Process.start()` calls the OS to `fork+exec` **the same binary**
   with `argv = [self_path, "--molt-spawn", entry_idx, ipc_fd]`. The child's
   `main` detects `--molt-spawn`, skips the user `main`, and jumps to
   `__molt_spawn_entry(entry_idx, ipc_fd)`.
3. **Argument transfer.** The parent pickles `(args, kwargs)` (pickle protocol 5,
   which molt has — doc 29 §103) and writes them to `ipc_fd`; the child unpickles.
   For large array arguments, **shared-memory via mmap** (the
   `multiprocessing.shared_memory` path): the parent puts the buffer in a
   POSIX shm segment (`shm_open`+`mmap`), pickles only the *name+shape* (pickle5
   out-of-band buffers — doc 29 §103/§111), and the child `mmap`s the same segment
   **zero-copy**. This composes with doc 29's zero-copy spine (Subsystem 2) and the
   mmap doc (Subsystem 5 / doc-slot 36).
4. **Result transfer.** Symmetric: child pickles the return, writes to `ipc_fd`,
   parent reads. `Queue.put/get` (the `queues.py` stub, doc 29 §33) becomes a
   pickle5-over-pipe (or shm-ring for buffers) channel.

**`fork` vs `spawn`:** on POSIX, CPython's default `fork` copies the parent's
memory (COW). molt CAN support real `fork` (the child inherits the parent's heap),
but **`fork` + threads + the GIL is the classic deadlock footgun** (a thread
holding the GIL at `fork` leaves the child's GIL locked forever). molt's stance
(parity + safety): **`spawn` is the default and the well-supported path** (matching
CPython 3.14's move to `spawn`-default on macOS/Linux); `fork` is available but
documented as unsafe-with-threads (parity with CPython's own warning). The
`_core.py`/`_api_surface.py` mapping (doc 29 §33: "fork/forkserver map to spawn")
is *correct* as the default and becomes a real spawn, not a `NotImplementedError`.

**Score 9, but tractable** (no compiler-arc blocker — the call graph exists; the
mechanism is OS `fork+exec` + the pickle5 path molt already has). This is the
**second-highest-value, lowest-blocker rung** after the §2 parity fixes —
build it early (§5-P1).

### 4-d. subinterpreters / isolates (PEP 734) — IMPORTANCE 2 × GAP 3 = **6**

The isolate machinery exists (§1.3, `isolates.rs`). PEP 734 fidelity needs:
- **Per-interpreter GIL** (§1.2/§5-P1 — the GIL moves to `RuntimeState`).
- **`concurrent.interpreters` / `_interpreters.py`** real implementation (replace
  the RuntimeError stubs, doc 29 §37): `interpreters.create()` →
  `molt_isolate_spawn` (a variant of `thread_main` that does *not* share state);
  `interp.exec(code)` → run a code object in the isolate; `interp.call(fn, args)`.
- **Sharable-object `Queue`** (PEP 734's `interpreters.Queue`): a Rust MPMC channel
  that transfers *sharable* objects (bytes, memoryview-over-shm, int/float/str via
  copy, None) across the heap boundary — pickling non-sharable objects. This is the
  **same channel substrate as 4-c's spawn IPC**, minus the process boundary
  (intra-process, so shm is a plain `Arc<[u8]>` instead of `shm_open`).
- **`InterpreterPoolExecutor`** (PEP 734 / 3.14): a `ThreadPoolExecutor` variant
  where each worker thread owns an isolate. molt's `ThreadPoolExecutor`
  (`builtins/concurrent.rs`) is the base; the variant gives each worker a fresh
  `RuntimeState`.

**Provenance:** PEP 734 (PSF, semantics reference). The channel design is original
(reuses molt's crossbeam-channel infra, already a dep — `Cargo.toml:180`).

### 4-f. structured data-parallelism (`@par` loops) — IMPORTANCE 2 × GAP 2 = **4**

The `object/gil.rs` `GIL_RELEASED` flag (§1.2, deleted) *hinted* at this. The
end-state design: a `@molt.par` / `@par` decorator (or an auto-recognized
parallel-`for`) that runs a **TIR-provably-disjoint** loop body across a
rayon-style work-stealing pool.

**The substrate already exists.** molt's async scheduler *already* uses
`crossbeam_deque::{Injector, Worker}` FIFO work-stealing (`async_rt/scheduler.rs:
11,2730,2805,2837`) sized to `num_cpus::get()` (`scheduler.rs:378`) — the same
Chase-Lev work-stealing deque rayon is built on (rayon: "relies primarily on
crossbeam-deque", github.com/rayon-rs/rayon). **molt does not need to add rayon as
a dependency** — it has the work-stealing primitive in-tree. The `@par` design:

1. **Disjointness proof (the gate, composing with design 04 L4 loop-opt).** A
   parallel-`for` is sound only if iterations are **independent**: no
   loop-carried dependence, no aliased writes across iterations. The L4 loop arc
   (doc 04) already builds the dependence machinery (SCEV, the loop-carried IV
   analysis). `@par` consumes it: the loop is parallelizable iff L4 proves no
   cross-iteration dependence on any written memory. **Fail-closed: if L4 cannot
   prove disjointness, `@par` falls back to sequential** (a warning under a strict
   mode, silent otherwise) — never a racy parallel execution of a dependent loop.
2. **Body execution under the tier.** Under the **default tier**, `@par` bodies
   that touch shared Python objects still need the GIL — so default-tier `@par` is
   limited to bodies that operate on `Raw` (unboxed int/float) data and disjoint
   array slices (no RC, no shared-object mutation) — e.g. a numeric kernel over a
   `memoryview`/array. Under the **unleashed tier**, `@par` bodies can touch shared
   objects (biased RC + per-object locks make it safe). This is the honest scoping:
   *default-tier `@par` is a numeric-kernel data-parallel construct (no Python
   object RC in the body); full-generality `@par` is unleashed.*
3. **Work distribution.** Split the iteration space into chunks (rayon's
   divide-and-conquer), push to the `Injector`, workers steal. Reuse the
   `scheduler.rs` deque (or a sibling pool to avoid contending with async tasks).

**Provenance:** rayon's `join`/work-stealing model (Apache-2.0/MIT — study +
reimplement the chunk-splitting; molt uses its own crossbeam deque, not rayon
code). OpenMP's `#pragma omp parallel for` is the semantic reference for the
disjointness contract.

**Score 4** — valuable but gated on L4 (doc 04) for the disjointness proof and on
the unleashed tier for general bodies. The default-tier numeric-kernel subset is
buildable earlier (it composes with design 05 L2-SIMD: a `@par` + SIMD numeric
loop is the data-parallel headline).

---

## 5. PER-TARGET MATRIX (first-class per target, no degraded-native)

The user mandate: native = full ladder; each other target gets a **first-class**
design for the honest subset, not a degraded fallback.

| Rung | native macOS/Linux | WASM-browser | WASI | Luau (host-embedded) |
|---|---|---|---|---|
| (a) threading under GIL | **full** (real OS threads, isolates.rs) | **event-loop concurrency, no OS threads** (today: `molt_thread_spawn` → `NotImplementedError`, `isolates.rs:1066`) | **wasi-threads** (real threads where the runtime supports it) | **host-scheduler** (Luau coroutines on the embedder's loop) |
| (a′) switch interval | real (§2-a) | N/A (single-thread) | real if wasi-threads | host-defined |
| (b) blocking-IO GIL release | full (§2-b) | N/A (no blocking syscalls; async only) | full | host I/O |
| (c) multiprocessing/spawn | **full superpower** (§4-c) | **NO** (no process model in browser) → Web Workers (below) | limited (wasi has no fork; `wasi-threads` or component spawn) | host-process if embedder allows |
| (d) subinterpreters | full (§4-d) | **Web Worker = the isolate** (below) | wasi-threads-as-isolate | host-VM-per-isolate |
| (e) free-threading | **full unleashed** (BRC + mimalloc) | **SharedArrayBuffer + Workers** (below) | wasi-threads + shared memory | **N/A** (Luau is GC, single-VM; free-threading is the host's job) |
| (f) `@par` data-parallel | full (crossbeam pool) | Web Workers + SharedArrayBuffer | wasi-threads pool | host threads if available |

### 5.1 WASM-browser tier (the honest subset)

**The single-thread WASM runtime is correct as-is** (`refcount.rs` `Cell<u32>`,
GIL no-op `gil.rs:75-154`, threads → `NotImplementedError`). The **parallel** WASM
story is **SharedArrayBuffer + Web Workers** (the only true-parallelism primitive
in browsers):

- **Each Web Worker is an isolate** (Layer B): a separate WASM instance with its
  own linear memory = a separate interpreter heap. This maps *exactly* onto the
  PEP 734 isolate model — `InterpreterPoolExecutor` over Web Workers is the
  natural browser API. Communication = `postMessage` (structured clone = pickle-
  equivalent) or a **SharedArrayBuffer** for zero-copy numeric data.
- **Free-threading in one WASM instance** requires the **threads proposal**
  (shared linear memory + atomics) — `wasm32-unknown-unknown` with
  `+atomics,+bulk-memory` and a SharedArrayBuffer-backed memory. Where the deploy
  target supports it (cross-origin-isolated pages with COOP/COEP headers), the BRC
  + per-object-lock design of §3 applies with WASM atomics. **Honest scoping:**
  this requires the COOP/COEP isolation headers (not always available); the
  *default* browser tier is single-instance single-thread, and the
  Workers-as-isolates tier is the recommended browser parallelism (it needs no
  special headers — `postMessage` works everywhere).
- This composes with the asyncio WASM executor (doc 28 Phase 6 / doc 18): the
  browser event loop is the single-instance concurrency model; Workers add
  cross-instance parallelism on top.

**Provenance:** the SharedArrayBuffer/Workers model is the web platform standard;
the Workers-as-isolates mapping is original (and is the *correct* PEP 734 analogue
for the browser).

### 5.2 WASI tier

`wasi-threads` (the WASI threads proposal) gives real threads with shared memory.
Where the host wasm runtime supports it (Wasmtime, etc.), the **native ladder
applies** (BRC, locks) with WASM atomics. Where it does not, WASI degrades to the
single-thread model (same as browser-default). No process model in WASI core (no
`fork`) → multiprocessing is unavailable; subinterpreters via wasi-threads-as-
isolate is the parallel path.

### 5.3 Luau tier

Luau is **GC-managed, single-VM** (design 27 §scope: "Luau is GC-managed; dup/drop/
reuse are no-ops"). Free-threading *within* a Luau VM is not molt's concern — Luau
has no multi-threaded VM. The Luau concurrency model is **host-scheduler
embedding**: molt-compiled Luau runs as coroutines on the *embedder's* event loop
(the game engine / Roblox scheduler). `threading` maps to Luau coroutines (cooperative,
not parallel); true parallelism is the host's actor/Worker model (e.g. Roblox
`Actor` parallel Luau), which molt targets by emitting per-Actor isolated scripts —
the Luau analogue of the Workers-as-isolates model. **No BRC, no per-object locks**
(GC handles reclamation). Luau's tier is Layer-B-only (isolates via host Actors),
never Layer C (no free-threading in a single Luau VM).

---

## 6. BENCHMARK LANE (the gates — molt must beat CPython on every rung)

Performance contract (CLAUDE.md): molt > CPython on every benchmark, every target,
every profile. Each rung has a gating bench with target numbers and the baseline to
beat. Baselines: CPython 3.12 (GIL), CPython 3.13t (free-threaded), Go 1.22
(goroutines), where applicable.

| Bench | Rung | Metric | Baseline to beat | molt target |
|---|---|---|---|---|
| `bench_thread_spawn` | (a) | `Thread()` create+start+join latency | CPython 3.12 ~50–80µs/thread | **< CPython** (real OS thread; molt's lean `RuntimeState` init should match or beat) |
| `bench_gil_fairness` | (a) | 2 CPU-bound threads: max stall of either thread | CPython 3.12 fair within switch interval (5ms) | **fair within molt's switch interval** (§2-a); TODAY: FAILS (one starves) — this bench *defines done* for §2-a |
| `bench_blocking_io_release` | (b) | thread A `f.read()` on slow FIFO + thread B compute: B's throughput during A's block | CPython 3.12: B runs full speed (GIL released) | **B runs full speed**; TODAY: FAILS (io.rs holds GIL, §2-b) — defines done for §2-b |
| `bench_lock_uncontended` | (e/2-e) | uncontended `lock.acquire()/release()` throughput | CPython 3.12 `_thread.lock` (1 atomic CAS) | **≥ CPython** (the §2-e fast path) |
| `bench_lock_contended` | (e) | 8 threads hammering 1 lock | CPython 3.12 / 3.13t | **≥ CPython 3.13t** |
| `bench_processpool_startup` | (c) | `ProcessPoolExecutor(8).submit(f)` cold start (8 workers) | **CPython 3.12 ~0.4–2.4s** (8× import) / CPython 3.13t same | **< 50ms** (8× ~4ms re-exec) — the **10–75× superpower headline**, §4-c |
| `bench_processpool_throughput` | (c) | CPU-bound `Pool.map` over 8 cores, large N | CPython 3.12 multiprocessing | **≥ CPython** (and beats threading-under-GIL by ~Ncore) |
| `bench_shm_roundtrip` | (c) | 1GB array parent→child→parent via shared_memory | CPython 3.12 shared_memory | **≥ CPython** (zero-copy mmap, doc 29 spine) |
| `bench_interp_pool` | (d) | `InterpreterPoolExecutor(8)` CPU-bound | CPython 3.14 InterpreterPoolExecutor | **≥ CPython 3.14** |
| `bench_par_scaling` | (e/f) | data-parallel numeric kernel, 1→8 threads, speedup curve | CPython 3.13t (the only CPython that scales) + Go | **near-linear to 8 cores**; **> CPython 3.13t** (design-27 RC-tax repayment, §3.3) |
| `bench_ft_singlethread_tax` | (e) | single-thread overhead of `--unleashed` vs default molt | CPython 3.13t: **5–8% slower** than 3.12 | **molt unleashed < CPython 3.13t's tax** (i.e. molt's unleashed-vs-default gap is *smaller* than CPython's 5–8%) — the §3.3 thesis, measured |

**The two headline gates:** `bench_processpool_startup` (the AOT superpower, must
show 10×+) and `bench_ft_singlethread_tax` (the Perceus-repayment thesis, must show
molt's nogil tax < CPython's). If either fails, the corresponding rung is not done.

---

## 7. PHASED BUILD PLAN (complete-piece phases, deletions named)

Each phase is independently shippable, no dual sources of truth, no feature-flag
interim default. The unit of work is the complete structural change (CLAUDE.md).
Phases are ordered by (parity-first, then superpower, then the deep tier).

### P0 — Default-tier parity completion (the GIL is not done)

**Scope:** (a) GIL switch interval + fairness (§2-a) — replace the bare
`PREINIT_GIL` `Mutex` acquire with a **fair lock + safepoint-driven drop-request**
(round-robin via a ticket/Condvar queue); wire the drop-request into the existing
compiler safepoint. (b) Blocking-IO GIL release (§2-b) — wrap `io.rs`
read/write/readall/readline/writelines (`io.rs:3777,4014,4243,4784,5012,6432,6608`)
and `subprocess_ext.rs` blocking waits in `GilReleaseGuard`.

**Deletes:** nothing structural; **adds** a CI audit harness
(`MOLT_ASSERT_GIL_RELEASED_ON_BLOCK`) that aborts if a known-blocking intrinsic is
reached GIL-held (regression guard, mirrors design-27 `MOLT_ASSERT_NO_LEAK`).

**Gate:** `bench_gil_fairness` passes (no starvation within the switch interval);
`bench_blocking_io_release` passes (concurrent compute runs full speed during a
blocking read); full differential suite green (native AND LLVM); no regression on
single-thread benches (the fair lock's uncontended path must be as fast as the bare
`Mutex` — verify the `GIL_THREAD_COUNT <= 1` fastpath `gil.rs:209` is preserved).
**LoC: ~400–700** (the fair-lock + safepoint wiring is the bulk; the io.rs wraps
are mechanical).

**Risk: LOW-MEDIUM.** The fair lock must not regress the single-thread fastpath
(the dominant case). Mitigation: the `GIL_THREAD_COUNT <= 1` skip stays; the fair
queue engages only at thread_count > 1.

### P1 — One GIL authority + isolates + multiprocessing superpower

**Scope (the convergence + the superpower, one structural arc):**
1. **Converge to ONE GIL authority** (§1.2): move the GIL from the process-global
   `PREINIT_GIL` static onto `RuntimeState` (per-interpreter), with a
   per-interpreter bootstrap barrier closing the pre-init race the `gil.rs:160`
   comment documents (publish the interpreter's GIL before any second thread
   observes it). **DELETE `runtime/molt-runtime/src/object/gil.rs` entirely** (the
   `ObjectLock` stub → superseded by §3.4's header `ob_mutex`, built in P3; the
   `GIL_RELEASED` flag → superseded by the per-interpreter free-threaded-mode bit).
2. **Daemon-thread shutdown** (§2-c), **signal main-thread model** (§2-d),
   **threading.local teardown** (§2-f, the inverted RuntimeState-owned design).
3. **multiprocessing/spawn** (§4-c): the entry-point manifest + `--molt-spawn`
   re-exec dispatcher + pickle5/shm argument-and-result transfer. Replace the
   `queues.py`/`pool.py`/`spawn.py` stubs (doc 29 §33) with real spawn.
4. **subinterpreters foundation** (§4-d): `concurrent.interpreters` /
   `_interpreters.py` real impl over the now-per-interpreter-GIL isolates;
   the sharable-object `Queue` channel (shared with #3's IPC channel).

**Deletes:** `object/gil.rs` (the whole file); the `_interpreters.py`/
`_interpchannels.py` RuntimeError stubs (replaced); the `queues.py` et al.
`NotImplementedError` multiprocessing stubs (replaced with spawn).

**Gate:** `bench_processpool_startup` shows **10×+ vs CPython** (the superpower
headline); `bench_interp_pool` ≥ CPython 3.14; daemon/signal/threading.local
differential tests green; **exactly one GIL type in the tree** (grep:
`struct GilGuard` appears once; `ObjectLock` appears zero times); full suite green.
**LoC: ~1500–2500** (spawn protocol + channel + the GIL move are each substantial).

**Risk: HIGH — flagged as the riskiest phase.** The GIL-move (#1) touches the most
load-bearing synchronization primitive in the runtime; the pre-init barrier must be
*provably* race-free (re-run the Miri data-race check the `gil.rs:160` comment
references — the `builtins::modules::tests` cross-test interaction — against the
new per-interpreter design). If the barrier is wrong, every multi-threaded program
is at risk. **Mitigation:** the GIL-move is gated behind its own Miri-clean proof
before the spawn work builds on it; do not ship #1 and #3 separately (the spawn
isolates *need* the per-interpreter GIL — they are one arc). If the GIL-move cannot
be made Miri-clean in a session, **leave the baton note and land nothing** (the
binding directive — no half-arc).

### P2 — `@par` numeric-kernel data-parallelism (default tier) + lock fast path

**Scope:** (a) the §2-e uncontended lock fast path (the optimistic `ob_mutex` CAS,
shared with P3's container lock — build the primitive here). (b) Default-tier
`@par` (§4-f): the L4-disjointness-gated parallel-`for` for **Raw-data numeric
kernels** (no Python-object RC in the body), running on a crossbeam work-stealing
pool (sibling to the async scheduler's). Composes with design 05 L2-SIMD.

**Deletes:** nothing (additive); the `@par` recognition consumes design 04's L4
dependence analysis (no new dependence machinery).

**Gate:** `bench_par_scaling` near-linear to 8 cores on a numeric kernel (the
default-tier subset — `memoryview`/array math); `bench_lock_uncontended` ≥ CPython;
disjointness fail-closed (a dependent loop falls back to sequential, verified by a
test that a loop-carried-dependence loop is NOT parallelized). **LoC: ~600–1000.**
**Risk: MEDIUM** — the disjointness proof must be *sound* (a false-positive
"disjoint" on a dependent loop is a silent data race). Mitigation: fail-closed +
the L4 analysis is the same one design 04 proves; `@par` only *consumes* a proven
fact, never re-derives it.

### P3 — UNLEASHED free-threading tier (biased RC + per-object locks + mimalloc)

**Scope (opt-in `molt build --unleashed`, gated on design-27 having landed):**
1. **Biased RC** (§3.2): the `MoltRefCount` → BRC two-field layout
   (`ob_ref_local` + `ob_ref_shared` + `ob_tid`) + the merge-queue state machine,
   wired into the safepoint (the same safepoint as P0's switch-interval).
2. **Per-object locks** (§3.4): the 1-byte `ob_mutex` header field; container
   writes take it; container reads take the optimistic `_Py_TRY_INCREF`+revalidate
   path.
3. **mimalloc allocator** (§3.4): swap the unleashed-build allocator to mimalloc
   with the three-heap + page-reuse-sequence policy (the optimistic-read
   correctness precondition).
4. **Immortal objects** (§3.5): mark `None`/bools/small-ints/interned-strings/type
   objects immortal (`ob_ref_local = UINT32_MAX`).
5. **Design-27 `CrossThreadBorrow` extension** (§3.3): the lattice fifth fact that
   re-materializes a biased incref where a borrow escapes its owning thread.
6. **Deopt-counter atomicization** (§3.6): the runtime-mutable deopt counter
   becomes atomic-or-per-thread (the static AOT caches are already race-free).

**Deletes:** the single-`AtomicU32` `MoltRefCount` native path is *replaced* by BRC
(the WASM `Cell<u32>` path is kept for single-thread WASM). The default (GIL) build
keeps the simpler single-atomic RC — **the BRC layout is unleashed-build-only**
(a per-build choice, not a runtime flag — no dual-source-of-truth at runtime).

**Gate:** `bench_ft_singlethread_tax` shows molt's unleashed tax **< CPython
3.13t's 5–8%** (the §3.3 thesis); `bench_par_scaling` (full-generality bodies) >
CPython 3.13t; the memory-model contract (§3.5) verified by a TSan/loom run (no
memory-unsafety under racing container ops — DEFINED-behavior cases never crash);
`MOLT_ASSERT_NO_LEAK` clean under multi-thread; immortal-object inc/dec is a no-op
(verified). **LoC: ~3000–5000** (BRC + locks + mimalloc integration is the largest
phase). **Risk: HIGH** (the second riskiest, after P1). The optimistic-read
correctness depends on the mimalloc page-sequence policy being *exactly* right (a
premature page reuse = a UAF under racing read). Mitigation: loom/TSan model
checking of the optimistic-read path; the mimalloc policy is reimplemented from
PEP 703's specified algorithm, not improvised; **gated on design-27 landing**
(without the borrow-elision, the BRC tax is not repaid and the perf gate fails).

### P4 — Container reuse + WASM-threads + Luau-Actor isolates (target completion)

**Scope:** (a) container (list/dict/set) reuse under unleashed — the design-27 §4.3
`molt_reuse_token` child-decref + weakref-clear extension, now also handling the
cross-thread case. (b) WASM SharedArrayBuffer+Workers isolate tier (§5.1). (c) Luau
host-Actor isolate emission (§5.3). (d) wasi-threads tier (§5.2).

**Gate:** per-target benches (browser Workers-as-isolates roundtrip; Luau Actor
parallel; wasi-threads pool); full per-target matrix (§5) green. **LoC: ~2000–3000
across targets.** **Risk: MEDIUM** (per-target, isolated; each target's tier is
independently verifiable).

**Phase independence:** P0 (parity) and P1 (one-GIL + spawn superpower) are the
must-ship default-tier arc — they make molt's threading *correct* and deliver the
process-parallelism superpower with **zero free-threading risk**. P2 adds
default-tier data-parallelism. P3 is the opt-in deep tier (gated on design-27). P4
completes the per-target matrix. **P0 and P1 deliver the bulk of the user value
(correct threading + the 10–75× ProcessPool superpower) before any free-threading
complexity is incurred** — which is the correct sequencing for a perf-contract
language where most workloads are process- or isolate-parallel, not
shared-mutable-thread-parallel.

---

## 8. COMPOSITION RISKS (the cross-design pre-mortem)

| Risk | Interaction | Resolution | Phase |
|---|---|---|---|
| **R-rc×atomic** | design-20/27 RC × free-threading | design-27 borrow elision holds intra-thread in both tiers; cross-thread borrows re-materialize a biased incref (the `CrossThreadBorrow` lattice fact, §3.3). The default tier keeps wholesale elision (GIL serializes). | P3 |
| **R-rc×bias-soundness** | design-27 lattice proven *under GIL* | the lattice's dup/drop placement assumes serialized execution; under unleashed, only the §3.3 cross-thread fact changes — the intra-thread placement is unchanged (a thread's own borrows are still serialized w.r.t. itself). **The lattice is re-validated for the unleashed tier in P3, not assumed.** | P3 |
| **R-deopt×ft** | the deopt/OSR skeleton × free-threading | molt's specialization is **AOT** (static caches, immutable, race-free by construction, §3.6) — unlike CPython's runtime-mutating inline caches. Only the deopt *counter* needs atomicization. **molt is structurally friendlier here than CPython.** | P3 |
| **R-genfuse×tls** | generator fusion (doc 26) × thread-local state | a fused generator (doc 26 §2.3) that captures `threading.local` state must NOT be fused across a thread boundary. Fusion is intra-frame (same thread by construction), so the captured TLS is the fusing thread's — safe. **But:** if `@par` parallelizes a loop whose body drives a fused generator touching TLS, each parallel worker must see *its own* TLS. Resolution: `@par` workers each carry their own `RuntimeState` (the §2-f inverted design), so per-worker TLS is automatic. The fusion-bail predicate (doc 26: bail on cancel-token registration) extends: **bail fusion if the generator reads `threading.local` AND the enclosing loop is `@par`** (a fused-away generator has no frame to carry per-worker TLS). | P2/P3 |
| **R-asyncio×threads** | asyncio runtime (doc 28) × threads | (1) `loop.run_in_executor` already works (`ThreadPoolExecutor`, `concurrent.rs`) — a thread runs the blocking fn, the loop awaits. Composes today. (2) **loop-per-thread:** each thread may run its own event loop (`asyncio.new_event_loop` per thread) — the event-loop registry (`event_loop.rs`, doc 28 §1.1, `Mutex<HashMap<u64, EventLoopState>>`) is keyed per-loop-handle, so per-thread loops are isolated — **but under unleashed, the registry `Mutex` becomes a contention point**; resolution: per-thread loop state moves off the global registry Mutex into the thread's `RuntimeState` (composes with doc 28's "0 heap lookups on the hot path" goal). (3) signals + asyncio: `loop.add_signal_handler` must route through the §2-d main-thread signal model (signals only deliver on the main thread, so a non-main-thread loop's signal handler is invalid — CPython parity: `add_signal_handler` requires the main thread). | P1 (signal), P3 (registry-Mutex) |
| **R-signal×ft** | signal handling × free-threading | signals stay main-thread-only under unleashed (B6) — the pending-flag is checked at the main thread's safepoint; the safepoint is the same one BRC's merge-queue and the switch-interval use (one safepoint, three consumers). **No new race** — the pending flag is a single atomic the main thread reads. | P1/P3 |
| **R-twogil-residual** | the two-GIL convergence (§1.2) | if `object/gil.rs` is deleted but a caller of `is_gil_released()`/`ObjectLock` survives, the build breaks (good — fail-closed) or, worse, a stale import lingers. **Gate:** P1 grep-asserts zero references to `object::gil::*` after deletion. | P1 |
| **R-fork×threads** | multiprocessing `fork` × threads (§4-c) | `fork` with a thread holding the GIL deadlocks the child. Resolution: **`spawn` is the default** (no inherited locks); `fork` is documented unsafe-with-threads (CPython parity). The spawn re-exec carries no parent locks. | P1 |
| **R-mimalloc×default** | the unleashed mimalloc swap (§3.4) | mimalloc is unleashed-build-only; the default build keeps its current allocator. **Risk:** two allocator codepaths. Resolution: the allocator is a *build-time* choice (cfg, not runtime) — one allocator per binary, no runtime branch, no dual-source-of-truth at runtime. The default binary never links mimalloc-page-sequencing; the unleashed binary always does. | P3 |

---

## 9. EXPLICIT REFUSALS (rejected approaches, with why)

1. **REFUSED: a single global free-threading switch that flips the whole runtime
   GIL-less at startup** (the naive reading of `object/gil.rs`'s `GIL_RELEASED`).
   Why: it makes every program pay the nogil tax, violating the perf contract for
   the 99% single-threaded/process-parallel case, and it blends the two tiers
   (the binding directive forbids). The tier is a **build choice** (`--unleashed`),
   not a runtime flag.
2. **REFUSED: keeping both GIL implementations behind a feature gate.** Why:
   two-source-of-truth; `object/gil.rs`'s `ObjectLock` is also the wrong data
   structure (free-standing `Mutex` vs header `ob_mutex`). Delete it (§1.2).
3. **REFUSED: promising a Java/Go SC-DRF memory model for the unleashed tier.**
   Why: it diverges from CPython 3.13t (the parity target), breaking the
   unleashed-tier parity claim (unleashed == 3.13t). molt commits to exactly
   CPython 3.13t's "memory-safe, logically-racy" model (§3.5).
4. **REFUSED: `fork` as the multiprocessing default.** Why: `fork`+threads+GIL is
   a deadlock footgun; CPython itself moved to `spawn`-default. `spawn` is the
   path, and the AOT re-exec makes it *fast* (the superpower) — so there is no perf
   reason to prefer `fork`.
5. **REFUSED: adding rayon as a dependency for `@par`.** Why: molt already has the
   crossbeam-deque work-stealing primitive in-tree (`scheduler.rs`); adding rayon
   duplicates it and adds binary weight. Reuse the in-tree deque (§4-f).
6. **REFUSED: implementing free-threading BEFORE design-27 lands.** Why: the
   borrow-elision is the tax-repayment that makes molt's nogil tax beat CPython's
   (§3.3). Without it, the BRC tax is unrepaid and `bench_ft_singlethread_tax`
   fails the perf contract. P3 is *gated* on design-27.
7. **REFUSED: a degraded "threads are no-ops" WASM tier as the parallel story.**
   Why: the user mandate is first-class per-target. The honest browser parallel
   tier is Workers-as-isolates (§5.1), a real PEP-734-shaped design, not a stub.

---

## 10. KEY FILE ANCHORS (verified against HEAD bd0b76d3, 2026-06-06)

- **GIL (authoritative):** `runtime/molt-runtime/src/concurrency/gil.rs`
  (`PREINIT_GIL` static :177; `molt_gil()` :181; `GIL_THREAD_COUNT` fastpath gate
  :209; `GilGuard::new` :199; `GilReleaseGuard` :320, `::new` :327; `gil_held`
  :386; fallback owner/depth :420-422; the per-state→per-static race rationale
  comment :160-175).
- **GIL (stub to DELETE):** `runtime/molt-runtime/src/object/gil.rs` (`ObjectLock`
  :10; `GIL_RELEASED` :30; `is_gil_released` :34; `release_gil`/`acquire_gil`
  :39/:44; `gil_check` :51).
- **Refcount (BRC target):** `runtime/molt-runtime/src/object/refcount.rs`
  (`MoltRefCount` :17, `AtomicU32` native / `Cell<u32>` wasm :18-21; inc `Relaxed`
  :70; dec :85; `acquire_fence` :102).
- **Isolates / threading intrinsics:** `runtime/molt-runtime/src/concurrency/
  isolates.rs` (`MoltThreadHandle` :50; `ThreadRegistry` :118-135, `daemon` field
  :121; `THREAD_REGISTRY` :134; `thread_main` isolate-state :422-453;
  `thread_main_shared` :456; `molt_thread_spawn` shared-default :503-557, isolated
  escape hatch :511-519; `molt_thread_join` GIL-release :651; wasm
  `NotImplementedError` :1066-1091).
- **Thread-id counter (monotonic, no reuse):** `runtime/molt-runtime/src/
  concurrency/mod.rs` (`THREAD_ID_COUNTER` :18; `current_thread_id` :23-30; the
  `with_gil_entry!` panic-contract macro :…).
- **Lock primitives (std Mutex+Condvar):** `runtime/molt-runtime/src/concurrency/
  locks.rs` (Lock `Mutex<LockState>+Condvar` :31-32; RLock :114-115; Condition
  :128-129; Event :141-142; Semaphore :147-148; Barrier :163-164; Queue
  :182-184; `molt_local_new` :1818; `molt_local_get_dict` tid-keyed map :1828-1849;
  `molt_local_drop` :1865).
- **threading.local backing:** `MoltLocal` = `Mutex<HashMap<tid,dict_bits>>`
  (`locks.rs:177` region; teardown gap §2-f).
- **time.sleep (GIL-releasing, the correct pattern):** `runtime/molt-runtime/src/
  object/ops.rs` (`molt_time_sleep` :2556; `GilReleaseGuard::new()` before
  `thread::sleep` :2585).
- **io.rs (GIL-HOLDING gap, §2-b):** `runtime/molt-runtime/src/builtins/io.rs`
  (zero `GilReleaseGuard`; `molt_file_read` :3777; `molt_file_readall` :4243;
  `molt_file_readline` :4784; `molt_file_readlines` :5012; `molt_file_write` :6432;
  `molt_file_writelines` :6608).
- **subprocess (GIL-HOLDING gap, §2-b):** `runtime/molt-runtime/src/builtins/
  subprocess_ext.rs` (zero `GilReleaseGuard`).
- **Signals:** `runtime/molt-runtime/src/builtins/signal_ext.rs` (`molt_signal_
  signal` :296; `install_os_handler` call site; `molt_signal_raise_signal`
  `libc::raise` :382; `set_wakeup_fd` :461 — the self-pipe model).
- **Async work-stealing pool (the `@par` substrate, already in-tree):**
  `runtime/molt-runtime/src/async_rt/scheduler.rs` (`crossbeam_deque::{Injector,
  Worker}` import :11; `num_cpus::get()` :378; `Injector` :2730,2795; `Worker::
  new_fifo` :2805; `Steal::Success` work-steal :2837-2850).
- **GilReleaseGuard coverage (16 files, the §2-b audit):** async_rt {channels,
  io_poller, scheduler, sockets}, builtins {concurrent, functions_http, select},
  concurrency {isolates, locks}, c_api/molt_api, http_bridge, object/ops,
  state/lifecycle, runtime-{http,tk}.
- **multiprocessing stubs:** `src/molt/stdlib/multiprocessing/` (`context.py` →
  `_api_surface.py`; `queues.py`/`pool.py`/`spawn.py`/`shared_memory.py` thin
  stubs; `_core.py` 77KB; `_api_surface.py` 23KB).
- **Cargo deps (parallelism substrate present):** `runtime/molt-runtime/Cargo.toml`
  (`crossbeam-deque` :179; `crossbeam-channel` :180; `num_cpus` :181; `libc` :189
  — NO rayon, NO parking_lot, NO mimalloc yet; mimalloc added in P3).
- **Composing designs:** `docs/design/foundation/20_rc-ownership-drop-insertion.md`
  (RC substrate), `27_perceus_borrow_inference.md` (the borrow lattice / tax
  repayment, §1-3), `28_asyncio-frontier-runtime.md` (§2.8 dual-contract template,
  Part 3 parity boundary; run_in_executor / loop-per-thread), `26_real-async-
  generators.md` (generator fusion × TLS, R-genfuse×tls), `04_L4-loops.md`
  (the disjointness analysis `@par` consumes), `05_L2-simd.md` (the `@par`+SIMD
  numeric headline), `29_domain-critical-portfolio.md` §SUBSYSTEM 1 (the audit this
  doc fulfills and refines).

---

## 11. Sources / provenance (RESEARCH GRANT)

Primary sources consulted (study + reimplement; PSF/MIT = semantics reference,
no code copied; no GPL ingested):

- [PEP 703 – Making the GIL Optional in CPython](https://peps.python.org/pep-0703/)
  — biased RC two-field layout (`ob_ref_local`/`ob_ref_shared`/`ob_tid`), the
  4-state merge machine (0b00–0b11) + eval_breaker merge queue, immortal objects
  (`ob_ref_local = UINT32_MAX`), deferred RC (functions/code/modules/methods),
  per-object `ob_mutex` + "Optimistically Avoiding Locking", mimalloc 3-heap +
  page-reuse-sequence policy, Performance table (6%/5% Skylake/Zen3 single-thread,
  8%/7% multi). §1.4, §3.1–3.5.
- [PEP 734 – Multiple Interpreters in the Stdlib](https://peps.python.org/pep-0734/)
  — per-interpreter GIL, `concurrent.interpreters`, `InterpreterPoolExecutor`,
  sharable objects + `Queue` channel, isolation model. §1.1, §4-d.
- [Biased Reference Counting (Choi, Shull, Torrellas), PACT'18](https://iacoma.cs.uiuc.edu/iacoma-papers/pact18.pdf)
  — the owner-local-nonatomic / other-thread-atomic split; 7.3% throughput gain
  over naive atomic RC. §3.2.
- [Perceus: Garbage Free RC with Reuse (Reinking et al.), PLDI'21] (via design 27)
  — the borrow inference / Owned·Borrowed split that repays the atomic-RC tax. §3.3.
- [rayon-rs/rayon](https://github.com/rayon-rs/rayon) + crossbeam-deque (Chase-Lev
  work-stealing) — the `@par` work-stealing model (reimplemented on molt's in-tree
  crossbeam deque, not rayon code). §4-f.
- [Python 3.14 free-threading HOWTO](https://docs.python.org/3/howto/free-threading-python.html)
  + [What's New in 3.14](https://docs.python.org/3/whatsnew/3.14.html) — the
  current state of 3.13t/3.14t (specialization disabling, single-thread tax
  trajectory toward 5–10%). §1.4 (unleashed == 3.13t), §3.6.
- mimalloc (Microsoft, MIT) — the thread-scalable allocator + the restricted
  page-reuse the optimistic read depends on (dependency, P3; policy reimplemented
  from PEP 703's spec). §3.4.
- OpenMP `parallel for` — semantic reference for the `@par` disjointness contract.
  §4-f.
