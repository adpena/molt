# Molt Tasks, Channels, and Native Concurrency
**Status:** Priority / Core Feature
**Audience:** Compiler engineers, runtime engineers

## Executive Summary
Native concurrency is foundational to Molt. This document defines a goroutine-class task model with channels, designed for Python ergonomics and Go-like performance.

## Goals
- Millions of cheap tasks
- No GIL
- M:N scheduling
- Structured cancellation
- Typed channels
- Native streaming I/O (HTTP streaming, WebSockets)
- Production-grade semantics

## User Model
```python
from molt import Channel, channel, spawn

async def worker(jobs, results):
    while True:
        value = jobs.recv()
        results.send(value)

def main():
    jobs: Channel[int] = channel()
    results: Channel[int] = channel()
    spawn(worker())
```

### Current API Surface (CPython fallback)
- `molt.channel()` and `molt.Channel` wrap `molt_chan_*` intrinsics via shims.
- `molt.spawn()` dispatches to `molt_spawn` or falls back to a background event loop.
- Async-friendly helpers: `Channel.send_async()` / `Channel.recv_async()` for event-loop usage.

### Cancellation Tokens (Structured)
- Each task runs with a **current cancellation token**; by default tasks inherit
  the token from their parent (request-scoped default).
- Tokens can be overridden inside a task (task-scoped override) by setting a new
  current token; the new token becomes the inherited token for any spawned work.
- Cancellation is **cooperative**: handlers should check `molt.cancelled()` or
  call `token.cancelled()` at safe points and abort work promptly.

## Runtime
- Work-stealing scheduler (multicore scaling)
- OS threads
- Cooperative yields at channel and I/O ops

## Safety
- Immutable sharing allowed
- Mutable state via channels only (Tier 0)
- Compile-time rejection of unsafe sharing

## I/O
- epoll / kqueue integration
- One task per connection/request
- WebSocket streams map to bounded channels with backpressure

See `docs/spec/areas/web/0600_STREAMING_AND_WEBSOCKETS.md` for the streaming/WebSocket API and capability model.

## Partner Library
`molt_accel` provides CPython integration via a Molt worker binary and IPC.

## Acceptance
- 10× CPython throughput for I/O services
- Linear core scaling
- No global lock
