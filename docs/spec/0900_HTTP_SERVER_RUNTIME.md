# Molt HTTP Server Runtime (molt_http): Fast, Small, Correct
**Spec ID:** 0900
**Status:** Draft (implementation-targeting)
**Priority:** P0 for “web is the wedge”
**Audience:** runtime engineers, networking engineers, AI coding agents
**Goal:** Define a production-grade HTTP server runtime for Molt that enables Go-class concurrency, excellent tail latency, and a small deployable footprint.

---

## 0. Positioning
Molt’s HTTP server should:
- be **boring and correct** (HTTP/1.1 first, HTTP/2 next)
- integrate tightly with Molt tasks/channels and cancellation
- be small enough to ship inside a single binary
- offer primitives needed for a web framework (routing, middleware, streaming)

This is not “yet another Python web framework.”
It is the foundation that makes Molt credible for web services.

---

## 1. Requirements
### 1.1 Performance
- handle high concurrency with stable P99/P999
- low allocation rate per request
- avoid per-connection OS threads

### 1.2 Correctness
- RFC-compliant enough for production
- robust to malformed inputs (never crash)
- clear limits to prevent abuse (header sizes, body sizes)

### 1.3 Operability
- structured logs
- metrics (latency, bytes, connections)
- graceful shutdown
- readiness/liveness hooks

### 1.4 Portability
- macOS + Linux first
- Windows later (explicit)
- support embedding into `molt_worker` as a mode (optional)

---

## 2. Architecture
### 2.1 Core loop
- nonblocking sockets (epoll/kqueue)
- accept loop produces connection tasks
- each connection drives an HTTP parser state machine
- requests are surfaced as events to the application handler

### 2.2 Concurrency model
- “connection task” owns socket I/O and parsing
- “request task” runs handler logic
- request tasks inherit a cancellation token tied to:
  - client disconnect
  - server timeout
  - shutdown signal

### 2.3 Backpressure
Backpressure is mandatory:
- bounded queues between accept → parse → handler → write
- if handler is slow, server reduces read pressure to avoid memory blowups
- configurable max in-flight requests per connection and per server

---

## 3. HTTP features (phased)
### 3.1 v0.1 (must ship)
- HTTP/1.1 keep-alive
- request/response headers
- chunked transfer decoding and encoding
- streaming bodies (request + response)
- request timeouts
- max header size, max body size

### 3.2 v0.2
- HTTP/2 (server side)
- header compression (HPACK)
- stream multiplexing with fairness

### 3.3 v0.3
- WebSockets (or earlier if needed for demos)
- SSE (Server-Sent Events) (easy win)

---

## 4. API shape (Molt-level)
Minimal server API:
```python
from molt_http import serve

async def app(req):
    return Response.json({"ok": True})

serve(app, host="0.0.0.0", port=8000)
```

Key types:
- `Request`: method, path, query, headers, body stream
- `Response`: status, headers, body stream
- `Context`: cancellation token, deadline, peer info, trace ids

---

## 5. Cancellation and deadlines
- each request has a deadline (default configurable)
- client disconnect cancels request token
- handler code can await `ctx.cancelled()` or check token
- handler code may override the current token for sub-work (task-scoped override)
- on cancel, server must stop reading body and abort writes safely

---

## 6. Observability
### 6.1 Metrics
- active connections
- active requests
- request duration histogram
- bytes in/out
- status code counts
- queue depth (backpressure indicators)

### 6.2 Logs/tracing
- structured access logs
- per-request trace id hook
- compatibility with OpenTelemetry (phase-in)

---

## 7. Security and hardening defaults
- conservative header and body size limits
- timeouts on read and write
- optional TLS termination (phase-in; can rely on reverse proxy early)
- request parsing fuzz tests

---

## 8. Testing
- golden tests for HTTP parsing edge cases
- fuzz tests on parser
- soak tests for keep-alive and slowloris-like behavior
- benchmark suite for concurrency and tail latency

---

## 9. Acceptance criteria
- can run the Django-offload demo with Molt hosting the endpoint (optional mode)
- can sustain high concurrency on a single process with stable P99
- no crashes on malformed inputs under fuzzing
