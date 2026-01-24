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
from molt import net

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
