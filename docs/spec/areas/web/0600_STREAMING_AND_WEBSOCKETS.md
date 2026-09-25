# Streaming I/O and WebSockets
**Spec ID:** 0600
**Status:** Draft (implementation-targeting)
**Audience:** runtime engineers, compiler engineers, package authors
**Goal:** Define native streaming and WebSocket APIs with capability gating, consistent with tasks/channels and deterministic execution.

---

## 1. Scope
- Streaming HTTP request/response bodies.
- WebSocket connections (client + server).
- Backpressure-aware integration with Molt tasks/channels.
- Capability-based access control for all network I/O.

Non-goals (v0.1): HTTP/2, QUIC, browser-native sockets.

---

## 2. Capability Model
All network I/O is explicit and capability-gated:
```
[molt.packages.molt_net]
capabilities = ["net", "websocket.connect", "websocket.listen"]
```
Rules:
- Capabilities are granted by config only (no ambient network access).
- Each capability produces a `cap_id` passed into runtime calls.
- Deterministic builds forbid unlisted capabilities.

---

## 3. Core API Surface (Python)
```python
from moltlib import net


async def handler(req):
    async for chunk in req.body:
        ...
    return net.Response(body=net.stream(iter_chunks()))


async def ws_handler(ws):
    async for msg in ws.recv():
        await ws.send(msg)
```

Semantics:
- `req.body` and `ws.recv()` return async iterators.
- `net.stream()` wraps a channel/iterator into a streaming body.
- All streams are backpressure-aware and bounded.

### File streams and namespace ownership

`moltlib.io.stream(file, mode="rb", chunk_size=65536, **open_kwargs)` returns a
single-pass `FileStream`, usable with either `for` or `async for`. File streaming
is a Molt-specific extension, not part of CPython's `io` surface. The former
`molt.stdlib.io.stream` extension is removed; use `moltlib.io.stream` instead.

The adapter consumes public `io.open/read/close` behavior. In compiled programs,
the normal runtime I/O authority enforces capabilities; there is no second
permission check or dependency on networking or the compiler package. Ordinary
CPython execution uses CPython's I/O and is not a Molt capability proof.
The file is opened eagerly, after validating a positive integer chunk size.
Each pull reads at most that many bytes (binary mode) or characters (text mode).
Async iteration preserves pull-based backpressure but does not make the file
read nonblocking. EOF and read errors close the owned file. Use `with` or
`async with`, or explicit `close`/`aclose`, when stopping early. Cleanup failures
propagate, retaining the read exception as their context when applicable.

The file stream implements the iterable protocols accepted by `net.Stream`; it
does not need a networking wrapper or networking intrinsics for file access.
Reference-Python adapter tests and source-closure tests do not certify compiled
native/WASM or cross-platform behavior; those require target execution receipts.

---

## 4. Runtime Primitives
### 4.1 Stream channels
- `stream<T>` is a thin wrapper over `chan<T>` with a fixed-size buffer.
- Producers block/yield on full buffer.
- Consumers block/yield on empty buffer.

### 4.2 WebSocket runtime
- WebSocket frames are normalized to `bytes` or `str`.
- Control frames (ping/pong/close) are handled by the runtime.
- The runtime exposes:
  - `ws_send(cap_id, conn_id, ptr, len) -> status`
  - `ws_recv(cap_id, conn_id, out_ptr, out_len_ptr) -> status`
  - `ws_connect(...)` gated by `websocket.connect` and delegated via a host hook.

---

## 5. Backpressure and Scheduling
- All streaming operations yield to the scheduler on backpressure.
- Bounded buffers are mandatory for WebSocket send/recv loops.
- Scheduler fairness targets: no single connection can starve others.

---

## 6. WASM and Host Interop
- WASM modules cannot open sockets directly.
- Hosts provide `ws_connect`/`ws_listen` imports gated by capability tokens.
- Payloads are passed as `(ptr, len)` byte buffers.

---

## 7. Acceptance Criteria
- 1M+ concurrent idle connections with bounded memory.
- Linear scaling across cores for active connections.
- Deterministic behavior in tests with fixed input streams.
- No network access without explicit capability.
