<!-- Foundation design 28 (renumbered from the commissioned slot 27, which a parallel
session took for the Perceus borrow-inference doc be9f64d16). Architect: read-only
research-granted agent, 2026-06-06. Runtime complement of doc 26. Saved verbatim.
SUPERVISOR NOTES: (1) Part 1 §1.4 ROOT-CAUSES task #25 (InvalidStateError = dual
source of truth between HEADER_FLAG_TASK_DONE and FutureState.done) with the Phase-1
implementation-ready fix; (2) two additional parity bugs found: done-callbacks called
synchronously instead of via call_soon (ordering contract), and the InvalidStateError
message-string mismatch vs CPython 3.12; (3) per the dual-contract principle, §2.8's
Trio-style strict mode is UNLEASHED-tier (explicit opt-in), CPython cancel semantics
stay the default; (4) per the target-tiered principle, Phase 6 (WASM/design-18) is a
first-class executor design, not a fallback.

CORRECTION (2026-06-06, implementation session, commit d8665ba1a): §1.4's root-cause
attribution for task #25 is DISPROVEN by reproduction. The InvalidStateError is NOT
the HEADER_FLAG_TASK_DONE/FutureState.done desync: minimal repro
`async def main(): raise RuntimeError("x"); asyncio.run(main())` (no async-for, no
await) shows the coroutine poll SWALLOWS the pending exception at the
STATE_TRANSITION/awaiter boundary, so Task._runner's `except BaseException` never
fires and set_exception is never called — this is the PRE-EXISTING bug #3
(memory/project_asyncio_p0_arc.md; native facet localized to
function_compiler.rs:13538-13572 + _emit_await_value/_emit_raise_if_pending; same
class as the LLVM state-resume dominance baton → StateDispatch #24). §1.4's
state-sync fix applied alone converts the loud InvalidStateError into a SILENT WRONG
RESULT (asyncio.run returns None) — forbidden. What survives of Phase 1: (a) the
message-string parity fix LANDED (d8665ba1a; NB the C-accelerator strings differ
from this doc's quoted pure-Python sources: 'Result is not set.' / 'Exception is not
set.' / 'invalid state'); (b) the done-callback call_soon ordering design is CORRECT
but BLOCKED on a second pre-existing bug — call_soon scheduled while the loop runs
is never drained (ready-runner not polled by block_on's drain_ready,
scheduler.rs:3793) — land them TOGETHER or callback execution regresses
(deferred-but-never-run). Three CPython-verified differentials for the ordering
contract are written and parked in the d8665ba1a history (removed from tree until
both fixes land). -->

# Asyncio Frontier Runtime — Architecture Design
## Document 27: molt Extreme-Scalability Asyncio Runtime

**Document status:** Design (2026-06-06). Architecture author: background architect agent, read-only audit of live tree.
**Scope:** Runtime complement to compile-side doc 26. Defines the runtime architecture that delivers uvloop-class or better single-core throughput, structured-concurrency-sound cancellation, io_uring/kqueue integration, zero-copy buffer management, and reliable shutdown semantics. Does not duplicate doc 26's compile-side content; interfaces with it at precisely-defined handoff points.

---

## Part 1 — Audit With Numbers: Current Runtime Cost Inventory

### 1.1 Verified File Inventory

The live async runtime consists of:

- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs` — 4,098 lines. MoltScheduler (crossbeam-deque Injector + crossbeam Worker FIFO per thread), SleepQueue (BinaryHeap + dedicated condvar thread), wake_task_ptr, enqueue_task_ptr, block_on, DeferredQueue (epoch-based, BTreeMap), task exception state save/restore, await-waiter graph (HashMap-of-Vec).
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — 1,465 lines. EventLoopState per Rust-owned handle: `ready: VecDeque<u64>`, `timers: BinaryHeap<TimerEntry>` (min-heap via reversed Ord), `cancelled_timers: HashSet<u64>`, `readers/writers: HashMap<i64, IoCallbackEntry>`. Registry: `Mutex<HashMap<u64, EventLoopState>>` — one global lock for all event loops.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/io_poller.rs` — 1,574 lines. IoPoller: `mio::Poll` + `mio::Registry` + `mio::Waker` + dedicated io_worker thread polling every 250ms. Maintains `sockets: Mutex<HashMap<usize, IoSocketEntry>>`, `waiters: Mutex<HashMap<PtrSlot, IoWaiter>>`, `ready: Mutex<HashMap<PtrSlot, u32>>`. Five distinct Mutexes acquired per register_wait call.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/cancellation.rs` — 521 lines. Cancel token tree: `Mutex<HashMap<u64, CancelTokenEntry>>`, task-token index: two more Mutexes. `token_is_cancelled` walks the parent chain (depth-bounded at 64) under lock.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs` — contains `molt_generator_send` (line 384), `molt_generator_throw` (line 497), and `molt_generator_close` (line 726). Each unconditionally saves/restores `ACTIVE_EXCEPTION_STACK` (a `Vec<u64>` thread-local), the context stack, the exception depth, and the `raise_active` flag.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/asyncio_core.rs` — FutureState, EventState, LockState, SemaphoreState behind one Mutex each; all GIL-serialized.

### 1.2 Per-Operation Cost Inventory

All costs are in distinct operations (lock acquire, allocation, syscall, vec clone) per invocation. GIL is held for all operations.

**spawn (asyncio.create_task(coro))**

From the Python asyncio __init__.py, `create_task` calls `_loop.create_task` which allocates a Python `Task` object (heap), registers it via `molt_asyncio_task_registry_set` (1 Mutex lock on `asyncio_task_map`, 1 `inc_ref`), calls `loop.call_soon(task.__step)` which acquires the event_loop_registry `Mutex`, calls `inc_ref` on the callback, and pushes to the `VecDeque<u64>`. Total: 1 Python Task heap object, 2 Mutex acquires, 2 `inc_ref`. The inner coroutine frame (`AllocTask` → `molt_task_new`) allocates `sizeof(MoltHeader) + closure_size` bytes (minimum 48 + 48 = 96 bytes), zeroes all slots.

Dominant cost: **2 heap allocations** (Task Python object + coroutine frame), **2 Mutex contended acquires** (both under GIL, so normally uncontended but lock/unlock overhead remains), **1 VecDeque push**.

**wakeup (from sleep queue to poll)**

`sleep_worker` holds the SleepQueue inner Mutex until the deadline fires, then calls `enqueue_task_ptr` which acquires `task_queue_lock()` (a separate global Mutex), checks four flag bits on the header, sets `HEADER_FLAG_TASK_QUEUED`, then calls `runtime_state().scheduler().enqueue` which calls `crossbeam Injector::push`. The Injector push is lock-free but involves an atomic CAS and potentially allocating a new segment when the current one fills.

Before running, `execute_task` saves the entire task exception state: acquires `task_exception_stacks` Mutex (take), `task_exception_handler_stacks` Mutex (take), `task_exception_depths` Mutex (get), sets CURRENT_TASK thread-local, sets current cancel token. On completion: reverses all saves, calls `clear_task_token` (3 Mutex acquires: `task_tokens`, `task_tokens_by_id`, `await_waiters`), calls `task_mark_done` (1 Mutex acquire: `task_queue_lock`), calls `wake_await_waiters` (acquires `task_waiting_on`, `await_waiters`, `await_waiter_index_map` all under 1 combined lock operation).

Total wakeup-to-poll overhead: **5 Mutex acquires** just in exception state save/restore, **3 additional Mutexes** for token cleanup. **0 allocations** on the hot path (state is moved, not cloned).

**sleep(0) round-trip** (the canonical async benchmark metric)

`asyncio.sleep(0)` in the Python stdlib calls `loop.call_soon(task.__wakeup)` — this acquires the event_loop_registry Mutex, pushes to VecDeque. The `_EventLoop._run_once` then drains the ready queue: acquires the registry Mutex, does `VecDeque::drain(..).collect()` (allocates a `Vec<u64>`), drops the lock, then calls each callback.

The drain-and-collect at `event_loop.rs:529` creates a `Vec<u64>` allocation per `run_once` iteration. This is the single highest-frequency unnecessary allocation on the hot path.

Separately, `async_sleep_poll_fn` (the native sleep future poll) registers with the SleepQueue: acquires `SleepQueue::inner` Mutex, inserts into `HashMap<PtrSlot, u64>`, pushes a `SleepEntry` to `BinaryHeap`. Wakeup: `sleep_worker` condvar-wakes, re-acquires the Mutex, pops from the heap, calls `enqueue_task_ptr`.

**sleep(0) total round-trip from submission to next-task-poll:** 1 VecDeque push, 1 Mutex (event_loop), drain-collect Vec alloc, 1 call_callable0 invoke, 1 Mutex (SleepQueue inner), 1 BinaryHeap push, 1 condvar wait/notify, 1 GIL acquire/release (sleep_worker crosses GIL boundary), 8+ Mutex acquires in execute_task exception save/restore. The sleep_worker thread takes the GIL to call `enqueue_task_ptr`, which means every sleep wakeup involves a GIL contention point even in single-task workloads.

Estimated single-core sleep(0) throughput with current implementation: in the range of 200k–400k/sec (dominated by the GIL-boundary crossing in sleep_worker and the exception-state save/restore).

**I/O readiness fan-out (io_worker → event_loop callbacks)**

`io_worker` runs on a dedicated thread. It calls `poller.poll()` with 250ms timeout. On readiness event:
1. Acquires `poller.waiters` Mutex
2. Acquires `poller.sockets` Mutex
3. For each ready socket: drains its WaiterList, removes from waiters
4. Drops both locks
5. For each ready future: calls `poller.mark_ready` (acquires `poller.ready` Mutex)
6. Acquires the GIL
7. Calls `wake_await_waiters` for each future (acquires `task_waiting_on` + `await_waiters` + `await_waiter_index_map` under 3 nested lock operations per waiter)

The 250ms poll timeout means maximum I/O latency of 250ms before the io_worker re-polls. This is the primary I/O latency floor. With `mio::Waker::wake()` called on registration, the actual latency is much lower for new registrations, but the fundamental model is a separate thread doing a blocking poll syscall and then crossing the GIL.

**cancellation**

`molt_cancel_token_cancel` acquires `cancel_tokens` Mutex, sets `cancelled=true`, drops lock, then calls `wake_tasks_for_cancelled_tokens` which acquires `task_tokens_by_id` Mutex, iterates all tasks on all tokens, calls `wake_task_ptr` for each. `token_is_cancelled` walks the parent chain (up to 64 hops) under the `cancel_tokens` Mutex on every `CURRENT_TOKEN` query.

The cancel-check hot path is called from `execute_task` after every poll, even for tasks that have no cancel token (the default token 1 is always present). Each check acquires the `cancel_tokens` Mutex.

**gather/wait**

`asyncio.gather` creates N Task objects and registers N await-waiter edges. Each `await_waiter_register` acquires `task_waiting_on`, `await_waiters`, and `await_waiter_index_map` Mutexes (3 per registration). For N=100, this is 300 Mutex acquires at gather setup time. The indexed_unique_vec operations at `scheduler.rs:1889–1951` maintain a parallel `HashMap<PtrSlot, usize>` position index to allow O(1) swap-remove, but this adds a HashMap insertion per registration.

### 1.3 Critical Path Bottlenecks Ranked

1. **Exception state save/restore per task poll** — 8+ Mutex acquires on the critical task execution path (`task_exception_stacks`, `task_exception_handler_stacks`, `task_exception_depths`, `task_cancel_messages`, `task_tokens`, `task_tokens_by_id`, `await_waiters`, `task_results`). These are all stored as separate `HashMap`s behind separate `Mutex`es in RuntimeState. Each is a HashMap lookup under a Mutex acquire even when single-threaded (under the GIL). This is the dominant overhead for short-lived tasks. **Target: 0 heap lookups on the hot task poll path.**

2. **Timer implementation: BinaryHeap per event loop** — The current `EventLoopState.timers` is a `BinaryHeap<TimerEntry>` inside the event_loop_registry `Mutex<HashMap<u64, EventLoopState>>`. Every `call_later` and timer check acquires this global Mutex. The `run_once` timer loop at `event_loop.rs:551–591` re-acquires the Mutex on every timer pop. For a 100k-timer workload this is 100k Mutex acquires per `run_once`. **Target: O(1) amortized timer insert and expire, single Mutex acquire per batch.**

3. **Drain-and-collect Vec allocation per `run_once`** — `event_loop.rs:529`: `state.ready.drain(..).collect()`. This allocates a `Vec<u64>` on every call to `run_once` regardless of queue length. Under Python's `asyncio.run()`, `run_once` is called in a tight loop. **Target: zero allocations per `run_once` on the common path.**

4. **io_worker 250ms poll timeout** — The io_worker polls with `Some(Duration::from_millis(250))` at `io_poller.rs:1181`. This is the I/O latency ceiling for any socket not yet registered when the poll began. **Target: sub-millisecond I/O notification latency with io_uring (Linux) or kqueue (macOS).**

5. **GIL crossing in sleep_worker** — The sleep_worker thread acquires the GIL at every wakeup (`GilGuard::new()` at line 2687). This means every sleep expiry is a GIL contention point, serializing all sleep wakeups through the main thread. **Target: batch wakeup delivery in a single GIL acquire.**

6. **Five separate HashMaps for task state** — `task_exception_stacks`, `task_exception_handler_stacks`, `task_exception_depths`, `task_last_exceptions`, `task_results` are five separate `Mutex<HashMap<PtrSlot, _>>` in RuntimeState. A lookup in any one requires a Mutex acquire + HashMap probe. **Target: inline the hot fields into the task header or a single task-local struct keyed once per task lifetime.**

7. **await-waiter graph: 3 nested Mutex acquires per edge** — `await_waiter_register` at line 1889 acquires `task_waiting_on`, `await_waiters`, and `await_waiter_index_map` three separate Mutexes (three separate lock() calls). **Target: single Mutex protecting the entire await-waiter graph.**

### 1.4 The Task #25 InvalidStateError Root Cause

The `async_for_with_exception_propagation` native test fails with `InvalidStateError` on native. The root cause is a race in the coroutine-as-task poll protocol under exception propagation. When `async for x in aiter:` consumes an async iterator and the iterator raises mid-iteration:

1. The `__anext__` coroutine poll completes with an exception pending (sets the exception slot in the generator frame).
2. The `StepContext` in the Task's `__step` method calls `result()` on the internal coroutine future.
3. `molt_asyncio_future_result` at `asyncio_core.rs:248` requires `state.done == true` before returning.
4. The exception path: when a native coroutine poll returns an exception (exception_pending), the calling `execute_task` in `scheduler.rs` clears the task's exception and marks `HEADER_FLAG_TASK_DONE` via `task_mark_done`. But the Python-level `Task.__step` logic also calls `future.result()` which routes to `molt_asyncio_future_result`, and if this is called before `task_mark_done` has been propagated — or if the async-for consumer task calls `result()` on a sub-coroutine that raised but whose `FutureState.done` was not set — `InvalidStateError("Result is not ready")` is raised.

The structural issue: the native Rust task execution in `execute_task` sets `HEADER_FLAG_TASK_DONE` on the MoltHeader directly, but the Python-level `Task` object's `FutureState` in `asyncio_core.rs` is a **separate state machine** in a separate HashMap. The two sources of truth can diverge: `HEADER_FLAG_TASK_DONE` is set, but `FutureState.done` is not yet set (or vice versa). Any code that checks `FutureState.done` before `molt_asyncio_future_set_result_fast` is called will see an inconsistent state.

The fix is an invariant: every code path that sets `HEADER_FLAG_TASK_DONE` must atomically also call `molt_asyncio_future_set_result_fast` (or `set_exception_fast`) on the corresponding FutureState. The current code at `scheduler.rs:3098` calls `task_mark_done` which only sets the flag; the Python layer calls `future_set_result` separately in `Task.__step`. The gap is the window between the flag being set and the FutureState being updated. Fix: remove the Python-level FutureState for native-managed tasks entirely, and have `molt_asyncio_future_result`/`exception` route through the native header flags instead.

### 1.5 Missing Benchmark Lane Definition

The following benchmarks do not yet exist in `/Users/adpena/Projects/molt/tests/benchmarks/` and must be created as part of this design's Phase 0 gate:

**Echo-server throughput/latency:**
- `bench_async_echo_server.py` — single-process asyncio echo server + client, measuring throughput (messages/sec) and latency percentiles (p50/p99/p999) at 1k, 10k, 100k connections.
- Target: molt native >= uvloop on TechEmpower-style single-machine benchmark; p99 < 500µs at 10k connections.

**Coroutine spawn storm:**
- `bench_async_spawn_100k.py` — `asyncio.gather(*[asyncio.create_task(noop()) for _ in range(100_000)])`. Measures wall time and peak RSS.
- Target: < 500ms wall time, < 200MB RSS for 100k tasks.

**Timer churn:**
- `bench_async_timer_churn.py` — 100k concurrent `asyncio.sleep(random.uniform(0, 0.01))` all started at once. Measures scheduler overhead (wall time to drain all), timer precision (actual vs requested sleep time).
- Target: <= 2x the CPython asyncio time; timer precision within 1ms.

**sleep(0) throughput:**
- `bench_async_sleep0.py` — single-coroutine loop of `await asyncio.sleep(0)` for 1M iterations.
- Target: >= 1M wakeups/sec/core on native (uvloop achieves ~1.5M/sec/core; Tokio achieves ~5M/sec/core natively). Current estimated baseline: ~300k/sec.

**Methodology reference:** uvloop uses libuv under the hood (citing the [MagicStack/uvloop GitHub](https://github.com/MagicStack/uvloop)). Benchmarks should replicate the uvloop benchmark methodology: measure wall-clock time of N operations from a subprocess runner, compute ops/sec, report p50/p95/p99 from 10 runs, compare against CPython asyncio and uvloop on the same machine. TechEmpower benchmark corpus provides the echo-server template.

---

## Part 2 — Frontier Architecture

### 2.1 Ready Queue: Intrusive Per-Task Linked List

The current `crossbeam Injector<MoltTask>` is a lock-free MPMC queue backed by a segment-list. Each push may allocate a new segment. The `VecDeque<u64>` in `EventLoopState.ready` allocates on drain.

**Chosen design: intrusive doubly-linked ready list embedded in MoltHeader.**

Add two pointer fields to `MoltHeader`: `ready_next: *mut MoltHeader` and `ready_prev: *mut MoltHeader`. The ready list head/tail is a per-event-loop pair of raw pointers protected by the single event-loop spinlock (see §2.2). Push is O(1) pointer write + prev/next update. Pop-all is O(1) (swap the head/tail to null, return old head). No allocation on any push.

The list is intrusive so tasks cannot be in two queues simultaneously — the same invariant enforced by `HEADER_FLAG_TASK_QUEUED`. The `QUEUED` flag already prevents double-enqueue; the intrusive pointers replace the crossbeam segment allocation.

For the single-core (GIL-bound) case this eliminates all allocations from the task-scheduling hot path. For the future free-threading extension, the per-core run queue becomes a per-core intrusive list with a shared injector for cross-core steals, exactly matching the Tokio design.

**Single-threaded (GIL mode) run queue:** A single `ReadyList` struct with head/tail in the `EventLoopState` (not the scheduler — see §2.5 on the EventLoop/Scheduler unification). Under the GIL, push and pop need no atomic operations — they are plain pointer writes. The spinlock exists only for cross-thread wakeup delivery (sleep_worker, io_worker posting wakeups).

**Multi-core extension (post-GIL):** Each core gets a `LocalReadyList` (intrusive, no lock). Cross-core steals use the existing crossbeam stealer interface. The steal batch size is 32 tasks (matches Tokio's empirical optimum).

### 2.2 Event Loop / Scheduler Unification

The current code has a three-layer split: Python `_EventLoop` calls Rust `EventLoopRegistry` intrinsics, which separately talk to `MoltScheduler` and `SleepQueue`. This creates the ready-queue duality: callbacks scheduled via `call_soon` go to `EventLoopState.ready`, while `create_task` posts tasks to `MoltScheduler.injector`. The Python `run_once` drains `EventLoopState.ready` and also calls `drain_ready` on the scheduler. These are two separate ready queues being drained in sequence.

**Chosen design: merge the two ready queues into one intrusive list in the EventLoop.**

`EventLoopState` becomes the authoritative ready-list owner. The `MoltScheduler` becomes a thin dispatcher that posts tasks to the event loop's ready list rather than its own Injector. `execute_task` moves from `MoltScheduler::execute_task` into the event loop's drain loop. This eliminates the dual-drain and ensures FIFO ordering between callbacks and tasks (CPython's invariant: `call_soon` callbacks and task steps are interleaved in FIFO order).

The unified `molt_event_loop_run_once` becomes:

```
acquire spinlock
  take the ready list (O(1) pointer swap to null)
  snapshot: take due timers from the timer wheel (see §2.3)
release spinlock

for each callback in ready list:
  call_callable0 (callbacks) or call_poll_fn (tasks)
  handle exceptions

for each timer callback:
  call_callable0
```

No allocations. The spinlock is a `std::sync::atomic::AtomicBool` spinlock for the pointer swap — held for nanoseconds.

**Allocation-free `run_once`:** The current `drain(..).collect()` at `event_loop.rs:529` is eliminated. The ready list swap at the start of `run_once` gives a snapshot in O(1) with no allocation. Newly-enqueued tasks during this iteration go onto a fresh ready list (the swap-and-drain pattern used by libuv and Tokio's task queues).

### 2.3 Hierarchical Timer Wheel (Varghese-Lauck, 1987)

Reference: G. Varghese and T. Lauck, "Hashed and Hierarchical Timing Wheels," SOSP 1987 (available via [Semantic Scholar](https://www.semanticscholar.org/paper/Hashed-and-hierarchical-timing-wheels:-efficient-a-Varghese-Lauck/7120286a965194c38c0786200be0187b8d14981b)). The algorithm provides O(1) amortized insert, delete, and per-tick expire for timers distributed across a time horizon.

**Chosen design: 4-level hierarchical wheel, 1ms tick, 4 wheels of 256 slots each.**

| Level | Slot granularity | Range covered |
|-------|-----------------|---------------|
| 0 (fine) | 1ms | 0–255ms |
| 1 | 256ms | 256ms–65s |
| 2 | 65s | 65s–4.6h |
| 3 (coarse) | 4.6h | 4.6h–49 days |

Each slot is the head of an intrusive singly-linked list of `TimerEntry` nodes embedded in the task/future object (two pointer fields: `timer_next` and the deadline stored inline). Insert: O(1) — compute `(deadline_ms >> level_shift) & 0xFF` for the finest applicable level, link into that slot. Delete: O(1) — unlink via the intrusive prev-pointer or a generation-based lazy deletion (generation field in TimerEntry; expired entries with mismatched generation are silently skipped during drain). Expire: per `run_once`, advance the current tick counter; drain level-0 slot `tick & 0xFF`; when level-0 overflows, cascade level-1, etc.

**Replacement for:** `EventLoopState.timers: BinaryHeap<TimerEntry>` and `SleepQueue.heap: BinaryHeap<SleepEntry>`. Both are replaced by a single `TimerWheel` per event loop. `asyncio.sleep(n)` calls `timer_wheel.insert(current_tick + (n * 1000) as u64, task_ptr)`. The sleep_worker background thread becomes unnecessary.

**The sleep_worker thread is eliminated.** In the current design, the sleep_worker blocks on a condvar until the next deadline, then acquires the GIL to enqueue the task. With the timer wheel, expiry happens inside `run_once`'s tick-advance — already under the GIL — with no cross-thread wakeup. For native targets, the event loop's I/O poll (see §2.4) uses the timer wheel's next-deadline as the poll timeout, so `run_once` sleeps until either I/O or a timer fires.

**Cancelled timer handling:** Replace `HashSet<u64>` with generation counters (one `u64` generation field per timer entry, matching a per-slot expected generation). This is O(1) per cancelled timer check and requires no unbounded set growth.

**CPython timer ordering contract:** CPython's asyncio orders timers by `(deadline, sequence_number)` FIFO within a deadline. The timer wheel preserves this: within a slot (same effective tick), entries are ordered by insertion order (FIFO intrusive list append). Across ticks, the wheel's level-0 drain is strictly monotonic. This is CPython-parity-correct.

### 2.4 io_uring (Linux) and kqueue (macOS) Backend

**Current:** mio wrapping epoll (Linux) / kqueue (macOS) via a 250ms-timeout blocking poll on a dedicated io_worker thread. Five Mutexes per I/O registration. GIL crossing on every wakeup.

**Chosen design: io_uring on Linux, kqueue batched on macOS, capability-clean fallback to mio.**

**Linux — io_uring backend:**

io_uring (Linux >= 5.1) provides a submission queue (SQ) and completion queue (CQ) in shared memory, eliminating per-operation syscalls. Key design choices following the 2024 DBMS benchmark paper ([arxiv.org/html/2512.04859v1](https://arxiv.org/html/2512.04859v1)):

- **Registered files:** Call `io_uring_register(IORING_REGISTER_FILES, fds, nfds)` at socket creation. Fixed-file operations (`IORING_OP_RECV_FIXED`, `IORING_OP_SEND_FIXED`) skip the per-operation `fdget/fdput` in the kernel, saving ~50ns/op.
- **Registered buffers:** For the stream transport layer (§2.6), pre-register a pool of receive buffers via `IORING_REGISTER_BUFFERS`. The kernel maps these pages and can write received data directly without a copy.
- **SQPOLL (optional, capability-gated):** When `MOLT_ASYNC_SQPOLL=1` is set, enable `IORING_SETUP_SQPOLL` with a 2000µs idle timeout. A kernel thread polls the SQ without requiring `io_uring_enter`, eliminating the syscall entirely on the hot path. Trade-off: one dedicated CPU core pinned to the kernel poll thread. SQPOLL measured 32% throughput improvement in the DBMS benchmark at ~546k TPS (same reference). Appropriate for server workloads; disabled by default.
- **Batched submission and drain:** Accumulate SQEs in the ring (no syscall) until either the batch reaches 64 entries or `run_once` reaches the I/O phase. Submit with a single `io_uring_enter`. Drain all completed CQEs in one pass. This replaces the per-registration `mio::Waker::wake()` syscall.
- **IORING_OP_MULTISHOT_ACCEPT / MULTISHOT_RECV (Linux >= 5.19):** A single accept SQE auto-rearms on every new connection, eliminating per-connection accept submissions.

The io_uring backend is gated on `#[cfg(target_os = "linux")]` and a build-time feature flag `io-uring-backend`. The capability-clean fallback path continues to use mio (epoll), so Windows and older Linux kernels are unaffected.

**macOS — kqueue batched backend:**

The current mio kqueue integration already works correctly but polls on a dedicated thread with 250ms timeout. Replace this with a synchronous kevent-batch call inside `run_once`:

```rust
// In run_once, I/O phase:
let timeout = timer_wheel.next_deadline_duration();
let nev = unsafe {
    libc::kevent(kq_fd, changelist.as_ptr(), changelist.len(),
                 eventlist.as_mut_ptr(), eventlist.capacity(),
                 &timeout_timespec)
};
```

The `kevent` call processes both registration changes (changelist) and returns ready events in a single syscall. This eliminates the dedicated io_worker thread for macOS and achieves the sub-millisecond I/O latency with a direct blocking kevent in the event loop thread.

**I/O path from socket readiness to task poll:**

Old path (5+ Mutexes, GIL crossing, 250ms latency floor):
`io_worker` (separate thread) → mio.poll(250ms) → acquire 3 Mutexes → call GilGuard::new() → acquire task_waiting_on+await_waiters+await_waiter_index_map → enqueue_task_ptr → acquire task_queue_lock → crossbeam Injector::push

New path (0 Mutexes on the hot path, sub-ms latency):
`run_once` (event loop thread) → kevent(kq) or io_uring CQ drain → for each ready fd: look up callback (fixed array by registered fd index) → push callback bits to intrusive ready list → after I/O phase ends, drain ready list normally.

The io_worker and sleep_worker background threads are both eliminated. On macOS/Linux native, the event loop runs single-threaded with no GIL-crossing overhead.

**WASM io path (design 18 integration):**

For WASM (wasi-preview-1), `run_once` calls `wasi::poll_oneoff` with subscriptions for all registered fds plus a clock subscription for the timer deadline (implementing design 18's Phase 1a/1b). This unblocks the WASM asyncio blockers identified in design 18. The `add_reader`/`add_writer` intrinsic stubs stop raising `RuntimeError` and instead insert fd subscriptions into the WASM poll set.

**Capability-clean fallback:**

If io_uring is unavailable (Linux < 5.1, or `IO_URING_BACKEND=0`), the mio epoll backend is used unchanged. The IoPoller's five-Mutex design is preserved in this path (no regression). The kqueue optimized path activates automatically on macOS native.

### 2.5 Zero-Copy Buffer Management

**Current state:** No zero-copy buffer layer exists. Streams read data via `libc::read` into heap-allocated `Vec<u8>` in `pipe_transport_write` at `event_loop.rs:1236`. The `write_buffer: VecDeque<Vec<u8>>` per pipe transport accumulates heap fragments.

**Chosen design: reference-counted buffer pool with memoryview semantics.**

Define a `MoltBuffer` type in a new `async_rt/buffer.rs`:

```rust
pub struct MoltBuffer {
    data: Arc<BufferStorage>,  // shared, RC'd backing storage
    start: usize,              // slice start within backing
    len: usize,                // slice length
}

struct BufferStorage {
    bytes: Box<[u8]>,          // pinned for io_uring registered buffers
    id: u32,                   // io_uring buffer index (if registered)
}
```

`MoltBuffer` slices are the unit of transport. A `recv` completion from io_uring fills a registered buffer in-place (no memcpy). The buffer is sliced and exposed as a `memoryview` to Python (molts `memoryview` protocol). The consumer reads from the slice; when the last reference drops, the buffer is returned to the pool.

**Integration with RC substrate:** `MoltBuffer` wraps in a `MoltObject` with `TYPE_ID_BUFFER`. The RC mechanism from design 20 applies: buffer references are tracked by the existing refcount on the `MoltHeader`. The `memoryview` exposing a `MoltBuffer` slice holds an `IncRef` on the backing buffer; when the `memoryview` is released, the `DecRef` returns the buffer. No GC required.

**io_uring registered buffer pool:** On startup with the io_uring backend, allocate `N=256` buffers of `BUF_SIZE=65536` bytes each. Register with `IORING_REGISTER_BUFFERS`. Issue `IORING_OP_PROVIDE_BUFFERS` to pre-populate the kernel's buffer ring. Receives use `IORING_OP_RECV_FIXED` with buffer-group selection, delivering data directly into a pool slot.

**Write path:** `transport.write(data)` submits an `IORING_OP_WRITE_FIXED` with the buffer directly if `data` is a `MoltBuffer` slice from the registered pool. Otherwise falls through to a copy-then-send path. This enables zero-copy for the pass-through streaming pattern (`recv → process → send`).

**CPython memoryview semantics:** The `memoryview` object wraps the `MoltBuffer` slice with the existing memoryview protocol in the runtime. The `format`, `itemsize`, `shape`, `strides`, and `ndim` attributes are correctly populated. `bytes(memoryview)` creates a copy (correct CPython semantics). Write via `memoryview` into a writable buffer follows the protocol. This is CPython-parity for the buffer protocol.

**Interaction with doc 26 await-inlining:** When `await transport.recv()` is inlined (doc 26 Phase 3 zero-suspension coroutine elimination), the buffer returned by recv is an SSA value in the consumer function. The RC drop for this buffer is handled by the standard drop insertion pass on the inlined function. No generator-frame-specific RC handling needed for the receive path.

### 2.6 Eager-Task Handshake with Doc 26

Doc 26 §2.3 defines: a coroutine that completes without yielding to the event loop should never be observed as a Task. The runtime handshake:

When `asyncio.create_task(coro)` is called, before enqueuing the task, attempt an eager poll: call `call_poll_fn(poll_fn, task_ptr)` immediately. If it returns `done=True` (non-pending), the task is complete. Fulfill any awaiters immediately via `wake_await_waiters`. The `Task` Python object is created, its result set, and it is immediately marked done. It is never enqueued in the ready queue. From the event loop's perspective, this task never ran asynchronously — it completed inline.

Precondition for eager poll safety: the current task is not in a `HEADER_FLAG_TASK_RUNNING` state that would make the inner poll reentrantly unsafe. Under the GIL, all tasks run sequentially, so the current task's flags are observed correctly.

This mirrors CPython's `asyncio.ensure_future` eager-task optimization added in 3.12 (`contextvars.copy_context()` + immediate step). The CPython implementation at `asyncio/tasks.py:376` checks `__step_run_and_handle_result` and if the coroutine is done, short-circuits. Molt's implementation is at the Rust level: `molt_spawn_task` (new intrinsic replacing `create_task`) calls `call_poll_fn` once before enqueueing.

**Observable semantics preservation:** The `Task` object is fully initialized before the eager poll, so `current_task()` returns the correct object if the coroutine body calls it. The cancel token is registered before the eager poll. The exception handler stack is set up before the eager poll. All CPython observability contracts hold.

### 2.7 Backpressure and Fairness

**FIFO vs LIFO slack:** CPython asyncio uses strict FIFO ordering for the ready queue (§CPython parity, see Part 3). Molt's intrusive ready list is FIFO (append to tail, pop from head). This is the correct base ordering.

**Fairness bound:** A task that always yields without sleeping (always `await asyncio.sleep(0)`) will monopolize the event loop in CPython's strict FIFO model. This is CPython-compliant behavior — it is the user's responsibility not to do this. Molt preserves this behavior (no starvation detection in the scheduler).

**I/O vs task fairness:** After draining all ready callbacks in `run_once`, the I/O poll phase runs with the timer wheel's next-deadline as the timeout. If no timers are pending and no I/O is ready, the event loop blocks in kevent/io_uring until the next event. This is identical to CPython's `_run_once` model. No starvation of timers or I/O by a compute-heavy task is possible (the ready queue is drained before the I/O poll blocks).

**Async generator fairness (asyncio.Queue backpressure):** `asyncio.Queue` uses `asyncio.Event` / `asyncio.Condition` implemented in `asyncio_core.rs`. The current implementation is correct; no changes needed for this design.

### 2.8 Structured Concurrency and Cancellation

**Current model:** Cancel tokens form a tree. `token_is_cancelled` walks the parent chain under the `cancel_tokens` Mutex on every check. The cancel message is stored in a separate `task_cancel_messages` HashMap.

**Chosen design: inline cancel-pending flag + lazy parent-chain check.**

The `HEADER_FLAG_CANCEL_PENDING` flag (already present) is the authoritative "cancel this task now" signal. Its check is O(1) bit-test on the task header — no Mutex. The parent-chain walk in `token_is_cancelled` is needed only when `asyncio.current_task().cancel()` is called from Python, which is a rare event. The check in `execute_task` (called after every poll at `scheduler.rs:3054`) must be O(1).

**Replacement:**
- Remove the `token_is_cancelled` check from the per-poll hot path in `execute_task`. Only check `HEADER_FLAG_CANCEL_PENDING`.
- Keep `token_is_cancelled` for the Python surface (`asyncio.current_task().cancel()` triggers `task_set_cancel_pending` which the next poll picks up via the O(1) flag check).
- The parent-chain walk is on the cancel side (when cancel is issued) — it calls `task_set_cancel_pending` for all tasks in the subtree and then wakes them. This moves the O(N-depth) work to the issuer rather than every poll.

**Trio model vs CPython semantics:**

CPython asyncio `Task.cancel()` delivers a `CancelledError` at the next `await` point. It does not guarantee that `finally` blocks run immediately — it schedules the cancellation for the next poll. Trio's structured concurrency model additionally requires that a cancel scope's tasks are joined before the scope exits.

Molt must maintain CPython cancel semantics exactly for parity. A "strict mode" opt-in (e.g. `molt.TaskGroup` extending CPython's `asyncio.TaskGroup`) could offer Trio-semantics (cancel scope propagation, immediate deliver on the next suspension). The strict mode is not required for CPython parity and is out of scope for this design's core arc.

**Cancellation and await-inlining (doc 26 interaction):** Doc 26 §2.3 notes that generator fusion must bail if the callee coroutine registers a cancel token in its preamble (`molt_task_register_token_owned`). This invariant is enforced by the recognition predicate. The runtime does not need to handle cancellation of fused-away tasks because they cannot be cancelled (they have no Task object and complete atomically before any cancel delivery point).

**Orphan detection:** A task that is spawned but never awaited is an "orphan." CPython emits `RuntimeWarning: Enable tracemalloc to get the object allocation traceback` when an un-awaited coroutine is GC'd. Molt must replicate this warning. Implementation: in the MoltHeader's `__del__` path (the RC-to-zero drop), check `HEADER_FLAG_SPAWN_RETAIN` AND `HEADER_FLAG_TASK_DONE == 0`. If an unfinished spawned task is dropped, emit the warning. This is already partially implemented via `HEADER_FLAG_SPAWN_RETAIN` at `cancellation.rs:235` — completing it requires the warning emission in the drop path.

**Shutdown ordering:** `asyncio.run()` calls `loop.close()` after the main coroutine completes. The `close()` path in `event_loop.rs` calls `drain_event_loop_state_refs` which releases all pending callbacks. The structured close sequence must:
1. Call `cancel()` on all remaining running tasks (those with `HEADER_FLAG_SPAWN_RETAIN` set).
2. Run `run_until_complete` to drain cancellation delivery (a bounded number of iterations, at most one per spawned task).
3. Call `asyncgen_shutdown` to close all live async generators.
4. Close the event loop.

Steps 1–3 are currently implemented partially in the Python `asyncio.run()` runner in `runners.py`. Step 1 is implemented. Step 2 is implemented. Step 3 (`asyncgen_shutdown`) is implemented in `generators.rs`. The gap: step 2 does not have a bounded iteration count — it loops until no tasks remain, which can hang if a task does not respond to cancellation. A maximum iteration count (e.g. 100 cancel-drain cycles) should be enforced, after which a `RuntimeWarning("Tasks were not cancelled within the shutdown budget")` is emitted and the loop closes anyway.

### 2.9 InvalidStateError Fix (Task #25)

The structural fix for the task #25 `async_for_with_exception_propagation` bug:

**Eliminate the dual source of truth.** For tasks managed by the Rust scheduler (`HEADER_FLAG_SPAWN_RETAIN` tasks and `block_on` tasks), the `FutureState` in `asyncio_core.rs` must be kept in sync with the `HEADER_FLAG_TASK_DONE` flag atomically. The fix:

In `task_mark_done` (currently `scheduler.rs:3328`), immediately after setting `HEADER_FLAG_TASK_DONE`, also update the task's `FutureState.done = true` in `asyncio_core.rs`. This requires the task-to-future-handle mapping, which must be stored in the task's frame at creation time (as a `u64` slot in the first 8 bytes of the task payload, replacing the currently-empty first slot).

The converse: Python `Task.set_result()` (called from `__step` in `tasks.py`) calls `molt_asyncio_future_set_result_fast`, which sets `FutureState.done = true`. This must also set `HEADER_FLAG_TASK_DONE` on the task header. Currently it does not (the two state machines are independent).

The fix is: unify them by making `task_mark_done` the single point that sets both. `molt_asyncio_future_set_result_fast` is refactored to call `task_mark_done` if the future has an associated native task pointer (stored in a new `FutureState.task_ptr: *mut u8` field). This is a complete invariant: exactly one write to `done=true` propagates atomically to both representations, under the GIL.

The specific `async_for_with_exception_propagation` failure: `__anext__` returns exception → `Task.__step` catches the exception, calls `future.set_exception(exc)` → calls `molt_asyncio_future_set_exception_fast` → sets `FutureState.done=true` and `exception_bits` → task's outer awaiter calls `await anext_task` → awaiter's `result()` is now called on a done future → no `InvalidStateError`. The race window is closed because `set_exception_fast` now also sets `HEADER_FLAG_TASK_DONE`.

---

## Part 3 — CPython-Parity Boundary

### 3.1 Observable Semantics That Constrain the Design

**Task identity and current_task():**
CPython source: `cpython/Lib/asyncio/tasks.py:Task.__init__` registers the task via `_asyncio._register_task(self)` and uses a per-loop `_current_tasks: dict` mapping. `asyncio.current_task()` calls `_asyncio._get_current_task(loop)` which checks `_current_tasks[loop]`.

Molt implements `_enter_task`/`_leave_task` at `scheduler.rs:1709/1737`. These must be called atomically around the task's poll invocation. Current implementation: called from the Python `Task.__step` method (in `tasks.py`). The issue: the Python-level `__step` is called from `execute_task` which calls `call_poll_fn`. If the native fast-path bypasses `Task.__step` and calls `call_poll_fn` directly, `_enter_task`/`_leave_task` are not called, and `asyncio.current_task()` returns `None` inside a native-scheduled task.

**Required invariant:** Any task that is observable as "running" to Python code (i.e., any task whose poll_fn has been entered but not returned) must have its Task Python object registered via `_asyncio._enter_task`. The CURRENT_TASK thread-local is not sufficient — it stores the raw pointer, not the Python Task object.

The eager-task optimization (§2.6) is safe because the Task object is created and registered before the eager poll.

**Callback ordering (call_soon FIFO):**
CPython's event loop processes callbacks in strict FIFO order. Any `call_soon` call made during a callback's execution is deferred to the next iteration (CPython `_run_once` drains the queue snapshot taken at the start of the iteration). The intrusive ready list's swap-at-start-of-run_once preserves this: callbacks added during the current drain go onto the new list and are processed next iteration.

**Future exception retrieval contract:**
CPython `Future.result()` (`asyncio/futures.py:180`) raises `InvalidStateError("Result is not ready.")` if `self._state == _PENDING`. `Future.exception()` raises the same if pending. The Python text must match exactly — this is tested by `tests/differential/stdlib/asyncio_future_invalid_state*.py`. The Rust intrinsics at `asyncio_core.rs:258` raise `InvalidStateError("Result is not ready")` (missing period). Verify exact message string parity with CPython 3.12 source.

**done-callbacks ordering:**
`Future.add_done_callback(fn)` registers a callback to be called when the future is done. CPython calls these callbacks via `loop.call_soon` in registration order. Molt's `molt_asyncio_future_set_result_fast` at `asyncio_core.rs:318` stores callbacks in `FutureState.callbacks: Vec<u64>` and would need to call `molt_event_loop_call_soon` for each. Currently it calls them immediately without going through the event loop ready queue. This violates CPython's contract: done-callbacks must be scheduled via `call_soon` (they run in the next event loop iteration, not synchronously). Fix: `set_result_fast` must post each callback to `call_soon` rather than calling it synchronously.

**`asyncio.shield()` semantics:**
`shield(aw)` creates an inner Task for `aw` and an outer Future. Cancelling the outer Future does not cancel the inner Task. This is preserved by the cancel-token model: the inner Task has a different token than the outer Future's scope. No changes needed.

### 3.2 Implementation-Free Territory

- **Timer precision:** CPython's timer precision is OS-dependent (typically 1ms on Linux, 10ms on macOS/Windows). Molt may provide sub-millisecond timer precision (the timer wheel at 1ms tick is already as precise as CPython's model). No parity constraint.
- **Scheduler FIFO vs LIFO within a deadline:** CPython asyncio uses FIFO. Molt's design uses FIFO. No divergence.
- **Number of I/O worker threads:** CPython asyncio uses the OS selector directly in the event loop thread (no separate I/O thread). Molt's chosen design (kevent/io_uring inside run_once) matches this exactly. The current IoPoller's dedicated thread is being eliminated.
- **Sleep precision:** `asyncio.sleep(0.001)` does not guarantee sub-millisecond resolution. The timer wheel's 1ms tick is sufficient.
- **Internal task representation (opaque):** CPython's `asyncio.Task` is a Python class. Molt's Task is a hybrid Python/Rust object. The surface API (`done()`, `result()`, `exception()`, `cancel()`, `add_done_callback()`, `get_name()`, `set_name()`) must be parity-correct. The internal representation is implementation-free.

---

## Part 4 — Phased Plan, Risk Register, Dependency Edges, Deletion Schedule

### Phase 0 — Benchmark Lane Creation (gate for all subsequent phases)

**Complete pieces:**
- Create `tests/benchmarks/bench_async_sleep0.py`, `bench_async_spawn_100k.py`, `bench_async_timer_churn.py`, `bench_async_echo_server.py`.
- Run all four against CPython asyncio and (if installed) uvloop to establish baseline ratios.
- Document baseline: estimated sleep(0) throughput ~300k/sec; target >= 1M/sec.

**Gate:** All four benchmarks run to completion on native, report a number. No correctness regression on existing asyncio differential tests.

**Files to create:** `tests/benchmarks/bench_async_sleep0.py`, `tests/benchmarks/bench_async_spawn_100k.py`, `tests/benchmarks/bench_async_timer_churn.py`, `tests/benchmarks/bench_async_echo_server.py`.

### Phase 1 — InvalidStateError Fix (task #25, P0 correctness)

**Complete pieces:**
1. Add `task_ptr: *mut u8` field to `FutureState` in `asyncio_core.rs`.
2. Add `future_handle: u64` slot to the first 8 bytes of the coroutine frame payload (via `molt_task_new` initialization at `generators.rs:263`).
3. Refactor `task_mark_done` (`scheduler.rs:3316`) to also call into `asyncio_core` to set `FutureState.done = true` via a new `asyncio_core_task_done(task_ptr)` function.
4. Refactor `molt_asyncio_future_set_result_fast`/`set_exception_fast` to call `task_mark_done` when `FutureState.task_ptr` is non-null.
5. Fix the `done-callbacks` ordering: `set_result_fast`/`set_exception_fast` must post each callback via `molt_event_loop_call_soon` rather than calling it synchronously.
6. Fix the `InvalidStateError` message string to exactly match CPython 3.12: `"Result is not ready."` (with period).

**Gate:** `tests/differential/basic/test_asyncio_basic.py::async_for_with_exception_propagation` passes on native. All asyncio differential tests that were previously passing remain passing.

**Files to modify:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/asyncio_core.rs` — `FutureState`, `set_result_fast`, `set_exception_fast`.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs` — `task_mark_done` at line 3316.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs` — `molt_task_new` at line 263.

### Phase 2 — Timer Wheel (replaces BinaryHeap in EventLoopState and SleepQueue)

**Complete pieces:**
1. Implement `TimerWheel` struct in new `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/timer_wheel.rs`. 4-level hierarchical wheel, 256 slots/level, 1ms tick resolution, intrusive linked list per slot, generation-based lazy cancel.
2. Replace `EventLoopState.timers: BinaryHeap<TimerEntry>` + `cancelled_timers: HashSet<u64>` in `event_loop.rs` with `TimerWheel`. All `call_later`/`call_at`/`cancel_timer` intrinsics route through the wheel.
3. Replace `SleepQueue.heap: BinaryHeap<SleepEntry>` + `SleepQueue.tasks: HashMap<PtrSlot, u64>` in `scheduler.rs` with `TimerWheel`. The `sleep_worker` thread is replaced by a `tick()` call inside `run_once`.
4. Update `molt_event_loop_run_once` to: snapshot ready list (O(1)), tick the timer wheel (drain due entries into ready list), then drain ready list. Optionally call kevent/poll with the timer wheel's next deadline as the timeout.

**Gate:** `bench_async_timer_churn.py` shows >= 2x improvement over baseline. All asyncio timer differential tests pass (call_later, call_at, cancel).

**Deletion:**
- `SleepQueue.heap`, `SleepQueue.tasks`, `SleepQueue.cv`, `SleepQueue.worker` (the sleep_worker thread) are deleted.
- `EventLoopState.cancelled_timers: HashSet<u64>` is deleted.

**Files to create:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/timer_wheel.rs`

**Files to modify:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — replace BinaryHeap timers.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs` — replace SleepQueue, delete sleep_worker.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/mod.rs` — add `pub mod timer_wheel;`.

### Phase 3 — Ready Queue Consolidation + Allocation-Free `run_once`

**Complete pieces:**
1. Add `ready_next: *mut u8` and `ready_prev: *mut u8` to `MoltHeader` (adjacent to existing fields). Define `ReadyList` struct with head/tail pointers + spinlock.
2. Replace `EventLoopState.ready: VecDeque<u64>` with `ReadyList` (intrusive list of task/callback objects).
3. Replace `MoltScheduler.injector: Injector<MoltTask>` with `Arc<ReadyList>` shared with the event loop. Task enqueue posts to the same intrusive list.
4. Update `molt_event_loop_run_once` to swap the ready list head atomically (O(1) pointer swap), eliminating the `drain(..).collect()` Vec allocation.
5. Callbacks (`call_soon`, timer expiry, I/O readiness) and task wakeups all post to the same `ReadyList` in FIFO order.

**Gate:** `bench_async_sleep0.py` shows >= 1M wakeups/sec on native (from ~300k baseline). Zero allocations per `run_once` on the common path (verify via custom allocator instrumentation or RSS monitoring).

**Deletion:**
- `EventLoopState.ready: VecDeque<u64>` deleted.
- `MoltScheduler.injector`, `MoltScheduler.deferred`, `MoltScheduler.epoch`, all `DeferredQueue` code deleted.
- `drain(..).collect()` at `event_loop.rs:529` deleted.

**Files to modify:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs` — `MoltScheduler`, `enqueue_task_ptr`, `wake_task_ptr`.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — `EventLoopState`, `run_once`.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/lib.rs` or `MoltHeader` definition (to add intrusive list pointers).

### Phase 4 — I/O Backend: kqueue batched (macOS) + io_uring (Linux)

**Complete pieces (two sub-phases, disjoint by platform):**

Phase 4a — macOS kqueue batched:
1. Eliminate the `IoPoller` background thread on macOS.
2. In `run_once`, after the timer tick, call `libc::kevent` with the changelist (pending register/deregister ops) and a timeout equal to `timer_wheel.next_deadline_duration()`.
3. Process all returned events inline: look up callback bits from the `readers`/`writers` map (keyed by `fd`), push to ready list.
4. Eliminate the `mio::Waker::wake()` call (no longer needed — kevent is called synchronously in `run_once`).

Phase 4b — Linux io_uring:
1. Add `io-uring` to `Cargo.toml` under `[target.'cfg(target_os = "linux")'.dependencies]` (using `tokio-uring` or `rio` or direct `liburing` bindings via `io-uring` crate v0.6+).
2. Implement `IoUringBackend` in `async_rt/io_uring.rs`: SQ ring, CQ drain, registered-files support, SQPOLL (capability-gated).
3. Capability-clean fallback: if `MOLT_IO_BACKEND=mio` or kernel < 5.1, use mio epoll. Detection via `io_uring_setup` probe at runtime.
4. Wire `run_once`'s I/O phase to drain the io_uring CQ and post ready callbacks.

**Gate:** `bench_async_echo_server.py` shows molt native >= uvloop throughput on Linux. p99 latency < 500µs at 10k connections. Zero regression on all asyncio differential tests.

**Deletion:**
- The `IoPoller` background `io_worker` thread is deleted on platforms with the new I/O backend.
- The five-Mutex `IoSocketEntry` structure in `io_poller.rs` is deleted (replaced by the fixed-array fd index).

**Files to create:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/io_uring.rs` (Linux only)

**Files to modify:**
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/io_poller.rs` — add kqueue inline path (macOS), keep mio fallback.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — wire I/O phase in `run_once`.
- `Cargo.toml` — add `io-uring` dep under linux target.

### Phase 5 — Zero-Copy Buffer Layer

**Complete pieces:**
1. Implement `MoltBuffer` / `BufferStorage` in `async_rt/buffer.rs`.
2. Register buffers with io_uring on Linux (pre-allocate 256 × 64KB pool).
3. Expose `MoltBuffer` as `memoryview` to Python (wire into `object/memoryview.rs`).
4. Update `StreamReader` in `asyncio/streams.py` to accept `MoltBuffer` slices directly.
5. Implement zero-copy `transport.write(memoryview)` via `IORING_OP_WRITE_FIXED` on Linux.

**Gate:** A benchmark sending `1GB` of data through an asyncio TCP stream shows < 2x the native `sendfile` throughput, and RSS does not grow proportionally with data sent.

### Phase 6 — WASM Asyncio Fixes (Design 18 integration)

Complete the design 18 fix plan (4 phases: 3a → 3b → 4c → 4a → 4b → 2a → 2b → 1a → 1b → 1c).

- 3a/3b: WASM bundler transitive closure + heavyweight asyncio package list. Files: `src/molt/cli.py`.
- 4c: Upgrade `molt_thread_submit` WASM stub to synchronous execution. File: `runtime/molt-runtime/src/async_rt/threads.rs:282–293`.
- 4a/4b: `run_in_executor` capability guard + ThreadPoolExecutor WASM shim. Files: `src/molt/stdlib/asyncio/__init__.py:3788`, `src/molt/stdlib/concurrent/futures/__init__.py`.
- 2a: Move `installTableRefs` before `molt_runtime_init`. File: `src/molt/cli.py:11938,11956`.
- 1a/1b/1c: `WasiPollSet` + WASM `run_once` with `wasi::poll_oneoff` + stop raising in `add_reader`/`add_writer` stubs. File: `runtime/molt-runtime/src/async_rt/event_loop.rs`.

**Gate:** All asyncio differential tests pass on WASM target. `bench_async_sleep0.py` runs to completion on WASM via `wasmtime`.

### Build Sequence Checklist

- [ ] Phase 0: benchmark lane creation and baseline measurement
- [ ] Phase 1: InvalidStateError fix — `asyncio_core.rs`, `scheduler.rs`, `generators.rs`
  - [ ] 1a: Add `task_ptr` to `FutureState`, add `future_handle` slot to task frame
  - [ ] 1b: Unify `task_mark_done` / `future_set_result_fast` / `future_set_exception_fast`
  - [ ] 1c: Fix done-callbacks to post via `call_soon`
  - [ ] 1d: Fix `InvalidStateError` message string parity
  - [ ] 1e: Differential gate: `async_for_with_exception_propagation` green
- [ ] Phase 2: Timer wheel (`timer_wheel.rs`)
  - [ ] 2a: Implement `TimerWheel` (4-level, 256-slot, intrusive, generation-cancel)
  - [ ] 2b: Replace `EventLoopState.timers` + `cancelled_timers`
  - [ ] 2c: Replace `SleepQueue.heap` + delete `sleep_worker` thread
  - [ ] 2d: Perf gate: `bench_async_timer_churn.py` >= 2x improvement
- [ ] Phase 3: Ready queue consolidation
  - [ ] 3a: Add intrusive list pointers to `MoltHeader`
  - [ ] 3b: Replace `VecDeque<u64>` in `EventLoopState` + `Injector` in scheduler
  - [ ] 3c: Allocation-free `run_once` — delete `drain(..).collect()`
  - [ ] 3d: Perf gate: `bench_async_sleep0.py` >= 1M wakeups/sec
- [ ] Phase 4: I/O backend
  - [ ] 4a-macOS: kqueue inline in `run_once`, delete io_worker thread
  - [ ] 4b-Linux: io_uring backend in `io_uring.rs`, capability-clean fallback
  - [ ] 4c: Perf gate: `bench_async_echo_server.py` >= uvloop on Linux
- [ ] Phase 5: Zero-copy buffer layer
  - [ ] 5a: `MoltBuffer` + registered buffer pool
  - [ ] 5b: `memoryview` integration
  - [ ] 5c: Zero-copy `write` via `IORING_OP_WRITE_FIXED`
- [ ] Phase 6: WASM asyncio (design 18 integration, per-blocker sub-phases)

---

### Risk Register

**R1: MoltHeader size increase from intrusive list pointers.**
Adding `ready_next`/`ready_prev` to `MoltHeader` increases every object's header by 16 bytes. The header is prepended to every heap-allocated Molt object. For a program with 1M live objects, this is 16MB additional RSS. The header fields can be reused (they are null when not in a ready list), so the space is allocated but not wasted when the object is live but not scheduled. Mitigation: profile `sizeof(MoltHeader)` before and after; ensure the header does not cross a cache-line boundary that would add a separate cache miss per object.

**R2: Timer wheel precision for very long sleeps.**
The 4-level wheel covers up to 49 days. `asyncio.sleep(3600)` (1 hour) lands in level 3 (4.6h slots). On timeout, the level-3 entry is cascaded down through levels 2→1→0. The cascade happens at level boundaries (every 65s for level-1 cascade, every 256ms for level-0 cascade). CPython's BinaryHeap has O(log N) insert/delete but exact timing. The timer wheel has O(1) but relies on periodic tick advancement. If `run_once` is not called for 256ms, the level-0 cascade is delayed. This is fine for asyncio workloads (the event loop runs continuously), but a blocked `run_once` would delay timer expiry. Mitigation: the I/O poll timeout in `run_once` is bounded by the timer wheel's next deadline, so `run_once` returns within 1ms of any due timer.

**R3: io_uring SQPOLL kernel thread interaction with the GIL.**
SQPOLL dedicates a kernel thread to SQ polling. This thread is not a Rust/Molt thread and has no interaction with the GIL. The GIL only gates Python-level operations. The io_uring CQ drain (which happens in `run_once` under the GIL) remains correct under SQPOLL. Risk: if SQPOLL causes excessive CPU consumption on a loaded system (the kernel thread runs at 100% even when no I/O is pending, until the `sq_thread_idle` timeout fires). Mitigation: SQPOLL is disabled by default, opt-in via `MOLT_ASYNC_SQPOLL=1`.

**R4: Phase 3 ready list and doc 26 eager-task interaction.**
The eager-task optimization (§2.6) calls `call_poll_fn` synchronously before enqueueing. If the eager poll itself triggers `call_soon` (adds to the ready list), those new entries should be processed in the current `run_once` iteration or the next. CPython's contract: `call_soon` callbacks from a synchronous context are processed on the next event loop iteration. The intrusive list's swap-at-start design ensures callbacks added during the eager poll go onto the post-swap list, not the current-iteration list. This is correct.

**R5: Intrusive list pointer aliasing with RC operations.**
The `ready_next`/`ready_prev` pointers in `MoltHeader` are raw `*mut MoltHeader` pointers. They are not reference-counted (adding a task to the ready list does not inc_ref it; the `HEADER_FLAG_TASK_QUEUED` flag prevents double-enqueue and ensures the task stays alive via its existing spawn-retain reference). The pointers must be nulled on dequeue. If a task is dropped while in the ready list (i.e., `HEADER_FLAG_SPAWN_RETAIN` is cleared by a concurrent operation while `HEADER_FLAG_TASK_QUEUED` is set), a use-after-free occurs. Mitigation: dequeue clears both `HEADER_FLAG_TASK_QUEUED` and nulls the list pointers before dropping the spawn-retain reference. The `task_queue_lock` spinlock protects this critical section.

**R6: Five-Mutex await-waiter graph hot path.**
Phase 3 does not address the await-waiter graph's three-Mutex-per-registration overhead. This remains as a known inefficiency for the `gather` hot path. Mitigation: after Phase 3 lands and perf is measured, a follow-up arc (Phase 3.5) consolidates the three await-waiter maps (`task_waiting_on`, `await_waiters`, `await_waiter_index_map`) into a single `AwaitGraph` struct behind one Mutex. This is a straightforward data structure consolidation.

---

### Dependency Edges

**Phase 1 (InvalidStateError) has no upstream dependencies.** It modifies existing structs. Can land immediately.

**Phase 2 (timer wheel) depends on:** Phase 1 complete (to avoid overlapping change to scheduler.rs). No dependency on doc 26.

**Phase 3 (ready queue) depends on:** Phase 2 (timer wheel integrated into run_once before the ready queue is consolidated; ensures the timer expiry posts to the new list correctly).

**Phase 4 (I/O backend) depends on:** Phase 3 (the I/O readiness callback posting must use the new intrusive ready list, not the deleted VecDeque).

**Phase 5 (zero-copy buffers) depends on:** Phase 4 (io_uring registered buffers require the io_uring backend from Phase 4b). Phase 5a/5b (MoltBuffer + memoryview) can land independently of 4b.

**Phase 6 (WASM) depends on:** Phase 3 (allocation-free run_once makes the WASM event loop more efficient). Phases 6-3a and 6-4 are independent of all other phases.

**Doc 26 dependency edges:**
- Doc 26 Phase 3 (await inlining, zero-suspension coroutine elimination) requires the eager-task handshake from §2.6 of this document (implemented as part of Phase 3 here).
- Doc 26's generator fusion (Phase 1) is independent of the runtime — it operates at TIR and eliminates generator frame allocations before the runtime sees them. No runtime changes needed.
- Doc 26's RC integration (Phase 2) requires no runtime changes — it operates in the drop_insertion pass.

**Task #24 (StateDispatch terminator) dependency:** Task #24 wires the LLVM state-resume dominance fix. This is a compile-side change (LLVM backend lowering). The runtime design here is independent of it. Phase 1's fix to the FutureState/task-flag dual-source-of-truth applies to native and does not touch LLVM.

**Free-threading pillar dependency:** All phases in this design are GIL-scoped (single-core). The multi-core extension described in §2.1 (per-core run queues, cross-core steal) requires the GIL removal. That pillar has no committed plan. The intrusive list pointers added in Phase 3 are forward-compatible with per-core queues: a per-core list would be a separate `ReadyList` instance per core, and tasks that enter the intrusive list on one core's list can be moved to another core's list (they are just pointer operations). No changes to Phase 3 are needed when the free-threading pillar eventually lands.

---

### Deletion Schedule

**Phase 1 completion:** No code deleted. Additions to `FutureState` and `task_mark_done` only.

**Phase 2 completion:**
- `SleepQueue.heap: BinaryHeap<SleepEntry>` deleted from `scheduler.rs:2427`.
- `SleepQueue.tasks: HashMap<PtrSlot, u64>` deleted from `scheduler.rs:2429`.
- `SleepQueue.cv: Condvar` deleted from `scheduler.rs:2439`.
- `SleepQueue.worker: Mutex<Option<JoinHandle>>` deleted from `scheduler.rs:2441`.
- `sleep_worker` function deleted from `scheduler.rs:2652–2698`.
- `EventLoopState.cancelled_timers: HashSet<u64>` deleted from `event_loop.rs:96`.
- `TimerEntry` struct deleted from `event_loop.rs:50–78`.
- `EventLoopState.timers: BinaryHeap<TimerEntry>` deleted from `event_loop.rs:92`.
- `SleepEntry`, `SleepState.heap`, `SleepState.next_gen` deleted.

**Phase 3 completion:**
- `EventLoopState.ready: VecDeque<u64>` deleted from `event_loop.rs:90`.
- `MoltScheduler.injector: Arc<Injector<MoltTask>>` deleted from `scheduler.rs:2730`.
- `MoltScheduler.deferred: Arc<Mutex<DeferredQueue>>` deleted from `scheduler.rs:2732`.
- `MoltScheduler.epoch: Arc<AtomicU64>` deleted from `scheduler.rs:2733`.
- `DeferredQueue` struct and all methods deleted from `scheduler.rs:2738–2787`.
- `drain(..).collect()` at `event_loop.rs:529` deleted.
- The multi-threaded worker-spawn loop in `MoltScheduler::new` deleted (worker threads go away for the GIL-bound case; they are empty no-ops already when `MOLT_ASYNC_THREADS=0`).

**Phase 4 completion:**
- `IoPoller.poll: Mutex<Poll>` and `io_worker` thread on macOS deleted.
- `IoPoller.sockets: Mutex<HashMap<usize, IoSocketEntry>>` deleted on platforms with the new backend.
- The five-Mutex design in `io_poller.rs:181–193` is deleted on the new-backend platforms, retained in the mio fallback.

---

## Key File Reference

**Files to create (in phase order):**

- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/timer_wheel.rs` (Phase 2)
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/io_uring.rs` (Phase 4b, Linux only)
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/buffer.rs` (Phase 5)
- `tests/benchmarks/bench_async_sleep0.py` (Phase 0)
- `tests/benchmarks/bench_async_spawn_100k.py` (Phase 0)
- `tests/benchmarks/bench_async_timer_churn.py` (Phase 0)
- `tests/benchmarks/bench_async_echo_server.py` (Phase 0)

**Files to modify (with primary change description):**

- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/asyncio_core.rs` — Phase 1: add `task_ptr` to FutureState, fix done-callbacks ordering, fix InvalidStateError message text.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs` — Phase 1: `task_mark_done` unification. Phase 2: delete `SleepQueue` heap + sleep_worker. Phase 3: delete `MoltScheduler` injector/deferred, add intrusive ready list wiring.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs:263` — Phase 1: `molt_task_new` initialize `future_handle` slot.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — Phase 2: replace BinaryHeap with timer wheel. Phase 3: replace VecDeque with intrusive list. Phase 4: add kqueue/io_uring I/O phase in `run_once`. Phase 6: add `WasiPollSet` for WASM.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/io_poller.rs` — Phase 4: add kqueue inline path, delete io_worker thread on macOS.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/mod.rs` — add `pub mod timer_wheel;`, `pub mod buffer;`, `pub mod io_uring;`.
- `/Users/adpena/Projects/molt/src/molt/stdlib/asyncio/__init__.py:3788` — Phase 6: `run_in_executor` WASM capability guard.
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/threads.rs:282–293` — Phase 6: upgrade WASM `molt_thread_submit` stub to synchronous execution path.
- `/Users/adpena/Projects/molt/src/molt/cli.py:11938,11956` — Phase 6: move `installTableRefs` before `molt_runtime_init`.
- `Cargo.toml` — Phase 4b: add `io-uring` crate under Linux target.

Sources:
- [MagicStack/uvloop: Ultra fast asyncio event loop](https://github.com/MagicStack/uvloop)
- [Hashed and Hierarchical Timing Wheels (Varghese & Lauck, SOSP 1987)](https://www.semanticscholar.org/paper/Hashed-and-hierarchical-timing-wheels:-efficient-a-Varghese-Lauck/7120286a965194c38c0786200be0187b8d14981b)
- [io_uring for High-Performance DBMSs: When and How to Use It (arxiv 2512.04859, 2024)](https://arxiv.org/html/2512.04859v1)
- [io_uring_sqpoll(7) man page](https://manpages.debian.org/testing/liburing-dev/io_uring_sqpoll.7.en.html)
- [timeout.c: Tickless Hierarchical Timing Wheel (William Ahern)](https://25thandclement.com/~william/projects/timeout.c.html)