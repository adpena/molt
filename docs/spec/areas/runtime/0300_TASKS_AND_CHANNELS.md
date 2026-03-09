# Molt Tasks, Channels, and Native Concurrency
**Status:** Active Runtime Contract
**Audience:** Compiler engineers, runtime engineers

## Executive Summary
Molt's RT2 async-runtime milestone is implemented as a Rust-backed `asyncio`
core. The production contract today is a deterministic single-loop scheduler
with a native I/O poller, cancellation propagation across timers and readiness
waiters, and a Python stdlib surface exposed through intrinsic-backed asyncio
shims.

This document is the canonical runtime-level contract for the current async
execution model. It does not promise a general M:N task runtime, channel-first
execution model, or no-GIL multicore scheduler. Those remain future design
space and must not be inferred from this spec.

## Current RT2 Contract
- Default async execution is powered by
  `runtime/molt-runtime/src/async_rt/event_loop.rs`.
- Native readiness polling is powered by
  `runtime/molt-runtime/src/async_rt/io_poller.rs`.
- Cancellation state is tracked and propagated by
  `runtime/molt-runtime/src/async_rt/cancellation.rs`.
- Scheduler coordination and runtime-facing queues live in
  `runtime/molt-runtime/src/async_rt/scheduler.rs`.
- Python-facing `asyncio` integration is exposed through
  `src/molt/stdlib/_asyncio.py` and the surrounding stdlib asyncio modules.

## Goals
- Deterministic scheduling within a loop turn
- Rust-owned timer and readiness queues
- Structured cancellation propagation
- Asyncio-compatible semantics for supported Python 3.12+ surfaces
- No host-Python fallback in compiled binaries
- Native and wasm behavior that stay aligned behind explicit capability gates

## Runtime Model
- The runtime event loop is single-loop and deterministic by design.
- The ready queue drains in insertion order for a given loop turn.
- Timers use a deadline heap with a FIFO tiebreak sequence for equal deadlines.
- The I/O poller preserves waiter ordering and removes cancelled waiters from
  poll queues before wake-up.
- Cancellation must propagate through timer handles, waiting tasks, and socket
  readiness registrations without leaving stale wakeups behind.
- Runtime mutation remains serialized by the GIL-like runtime lock; the async
  runtime is not a no-lock design today.

## Determinism Guarantees
The RT2 acceptance bar requires deterministic behavior for the cases that most
often regress async runtimes:

- Equal-deadline `call_later` / `call_at` callbacks resume in FIFO order.
- Surviving I/O waiters preserve registration order after neighbouring waiters
  are cancelled.
- Cancelled timers and readiness waiters do not produce spurious wakeups in a
  later turn.
- Task cancellation propagation is observable through the `asyncio` surface
  rather than being swallowed inside runtime queues.

These guarantees are covered by differential tests including
`asyncio_call_later_fifo_tiebreak.py`,
`asyncio_call_at_fifo_tiebreak.py`,
`asyncio_sock_recv_cancel_deterministic.py`,
`asyncio_sock_recv_cancel_survivor_order.py`, and
`asyncio_wait_for_cancel_propagation.py`.

## Python Surface
- The user-facing async contract is the stdlib `asyncio` surface, not a custom
  channel-first API.
- `src/molt/stdlib/_asyncio.py` is an intrinsic-backed bridge for loop/task
  state and must remain Rust-lowered in compiled binaries.
- Unsupported async capabilities must fail explicitly via capability gating or
  missing-intrinsic errors; they must not silently route through host Python.

## Channels And Broader Concurrency
Molt may grow richer task/channel primitives over time, but they are not the
current RT2 production contract. Any future channel or multicore task design
must integrate with the deterministic async scheduler instead of bypassing it,
and must be specified separately with:

- lock ordering and memory model
- cancellation semantics
- fairness guarantees
- native/wasm parity story
- differential and performance validation

## Non-Goals For This Contract
- Promising a general-purpose M:N scheduler
- Claiming linear multicore scaling for Python async code
- Removing the runtime GIL from the current async implementation
- Reintroducing CPython fallback/background-loop behavior to satisfy APIs

## Verification Expectations
Changes to the async runtime must ship with deterministic verification, not
just passing smoke tests. Minimum evidence includes:

- targeted differential coverage for timer ordering, cancellation propagation,
  and I/O waiter ordering
- runtime tests for queue cleanup and wake-up behavior
- documentation updates when scheduler or cancellation semantics move

See also `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md` for the runtime
locking contract and `docs/spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md`
for subsystem ownership.
