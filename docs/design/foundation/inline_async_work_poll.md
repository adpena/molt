# Inline asynchronous-work safepoints (HF-657)

Compiled call-return and loop-backedge observations read a runtime-scoped
pending-work word before entering the runtime. The word summarizes readiness;
the signal slots, GC scheduler, stopped-pool FIFO and C pending-call ring retain
their work and lifetime authority. No callback or Python object lives in the
summary. `state/pending_work.rs` owns its closed reason type and transitions.

## Layout and ownership

`ExecutionTls` remains the invocation's thread record. Its layout in
`molt-codegen-abi` lends the address of one aligned atomic u32 owned by the
attached `RuntimeState`. GC, signals and concurrent retirement hold the same
allocation through `Arc`; C pending-call publication uses the existing admitted,
signal-safe notification route. The runtime execution lease retains the origin
through callbacks, isolate switches, suspension and finalization. Backends take
the word address once per invocation. They never retain it across invocations
or turn a GIL token alone into a runtime-lifetime proof.
The generated-object ABI's revision 6 fingerprints the record prefix offsets
and work-word width and replaces both old link witnesses; no old-runtime alias
can satisfy a new object's field access.

The summary is shared by threads attached to that runtime. A worker can observe
main-thread-only work and take a harmless cold path; it cannot consume that
work or clear its bit. This avoids a thread registry and broadcast/lifetime
protocol solely for distributing hints. Main-thread-only signal and C pending
call execution keeps its existing custody checks. Isolates have distinct words;
the process signal/pending-call route belongs to the primary runtime.

## Publication and service

| Reason | Publication boundary | Readiness authority |
| --- | --- | --- |
| Signals | `SignalRuntimeState::record_delivery` | signal pending slots / tripped summary |
| GIL drop request | `concurrency::gil::take_gil`, after a waiter's switch interval (`signal_ext::notify_gil_drop_request`) | the bit itself (`PendingWork::take`) |
| GC | allocation threshold scheduling and resource-failure rearm | `GcRuntimeState.pending` |
| Pool retirement | `schedule_stopped_pool` | stopped FIFO / `stopped_pending` |
| C pending calls | `Py_AddPendingCall`'s existing notification hook, after ring publication | ready ring head |

Signal-route publication also adopts calls queued during runtime initialization.
Asyncio cancellation remains a task/scheduler transition at suspension points;
it is not an additional producer serviced by `service_async_work`.

The GIL drop request (CPython `_PY_GIL_DROP_REQUEST_BIT`) has no source to
recheck: a waiter that `try_lock_for(switch interval)` does not serve sets the
bit, and the holder's service clears it with `take`, then suspends and
reacquires the GIL; `unlock_fair` hands the mutex to the queued waiter. A
waiter that is still queued after the clear sets the bit again one interval
later. The request travels through the primary runtime's word, so a GIL holder
running inside an isolate does not see it (unverified cell).

Each producer publishes its source, then performs a release `fetch_or` of its
reason. Only after the service attempt does the consumer perform an acquire-release
`fetch_and` clearing that bit, then recheck the source and rearm if work remains.
The relaxed word read is only a hint: the cold service acquires the signal
slots, GC scheduler, stopped FIFO lock, or pending-call ring before using their
payload. Payload visibility never depends on treating the hint as an acquire.
The recheck must follow the RMW. A producer before the clear is acquired by that
RMW and its published source is visible to the subsequent acquire/source read.
A producer after the clear is either seen by the recheck or sets the bit itself.
A producer delayed between source publication and notification can only create
a redundant later notification. Concurrent/reentrant consumers can similarly
leave a redundant bit, but cannot discard an unserviced source. Whole-word
load/recompute/store would lose unrelated concurrent reasons and is forbidden.

The cold path retains signal, GC, pool retirement, then pending-call order. A
raising signal handler returns immediately; later source bits remain armed.
The consumer does not run handlers beneath an already propagating exception.

## Backend and exception obligations

LLVM emits a monotonic (relaxed), volatile atomic load. Volatile preserves the
authored repeated observations through optimization without extra machine
instructions. Cranelift currently exposes only sequentially consistent
`atomic_load`; it uses that stronger ordering, rather than a normal load that
GVN/LICM could reuse across an external request. WASM's admitted runtime is
single-threaded, so an aligned i32 load is sufficient. The runtime and ABI
already refuse threaded WASM; this change does not qualify it. Luau and Rust
already reject asynchronous-work observations without a runtime boundary.

The placement pass marks existing `CheckException` operations. It can fuse a
call's first exception observation or the last raising operation's observation
with the backedge service site. A pending exception can therefore reach the
*observation*, but must not reach the taken backedge. The empty-work edge still
reads the ordinary thread-record exception byte; pending exceptions never set
an asynchronous-work bit. `finally` observations call the passive payload
observer on that edge only when the exception byte is set; otherwise they emit
None directly. They retain their authored reconciliation path.
Their service branch returns the same payload as before, without an early
exception transfer. Dropping these observations would be a compiler defect,
not an optimization of an empty asynchronous poll.

## Prior art and alternatives

CPython 3.12's [CHECK_EVAL_BREAKER](https://github.com/python/cpython/blob/v3.12.0/Python/ceval_macros.h#L106)
is a relaxed load and branch. Its [ceval_gil.c](https://github.com/python/cpython/blob/v3.12.0/Python/ceval_gil.c)
contains `COMPUTE_EVAL_BREAKER`, producer notifications and `_Py_HandlePending`;
the handler clears scheduled GC before running it and recomputes the breaker.
Molt retains its existing service order and uses post-service clear/recheck
instead of adopting CPython's GIL-dependent whole-word recomputation. Newer
CPython's per-reason atomic bit operations motivate the disjoint RMWs.

HotSpot's [JEP 312](https://openjdk.org/jeps/312) and
[safepoint mechanism](https://github.com/openjdk/jdk/blob/master/src/hotspot/share/runtime/safepointMechanism.inline.hpp)
separate cheap thread-local polling from handshakes. Go's
[preempt.go](https://github.com/golang/go/blob/go1.23.0/src/runtime/preempt.go)
describes poisoning the stack bound so the existing prologue check enters the
preemption handler. Molt reuses its execution record, without a pinned register,
polling page, signal trap, or new scheduling protocol. These are design
references; no implementation text was copied.

## Qualification

The executable primitive ratchet and source-bound before/after receipts own cost
claims under [PERF_AUTHORITY](../../../tools/PERF_AUTHORITY.md). Native and WASM
execution controls exercise empty/notified work and existing pending exceptions;
runtime controls exercise actual producers, including a C publisher without the
GIL and a notification-suppression mutation control. Cross-platform, release
and free-threaded qualification require their own receipts; implementation and
local controls do not establish that entire matrix.

The source-built foreign C publisher releases/restores the main thread's GIL
around its blocking join and checks delivery after a taken Python backedge.
This is a shared CPython 3.12 oracle boundary, not a claim that CPython dispatches
at every C-call return. CPython 3.12's `SIGNAL_PENDING_CALLS` recomputes its
main-thread condition on the publisher; the fixture's paired GIL transition
does not depend on that foreign publisher arming the main thread's breaker.
Molt's runtime control separately suppresses just the producer notification,
proves that the inline gate starves real queued work, then restores the bit and
observes dispatch. Source-built extension loading needs its own passing receipt
in addition to the narrower runtime notification control.
