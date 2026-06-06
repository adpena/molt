<!--
Foundation design 35 — Web Stack Frontier. The end-state design for molt's web stack:
HTTP/1.1 + HTTP/2, TLS, sockets, WebSockets, the ASGI server runtime (standalone server
binary), and the modernized HTTP client. Architect: read-only research-granted agent,
2026-06-06. DESIGN ONLY; no implementation landed. Doc number 35 reserved by the
supervisor in the doc-29 remapping note (29 §header: "its 'Doc 30 Web Stack' -> slot 35;
... 35 web after docs 26/28 land"). Do not renumber.

All file:line anchors verified against the live worktree at HEAD commit
fcf949af14c91479d13d95909ab4d2abf9d12a1c (branch main, 2026-06-06). The doc-29
SUBSYSTEM 4 audit was written at 951938075; this doc RE-AUDITS the web stack at HEAD and
flags SIX corrections inline (§0.1) — doc-29 is materially stale on the HTTP-client size,
the WebSocket wiring, the ASGI adapter, the event-loop/io-poller split, the SSL getter
surface, and the socketserver path. (Doc 37's agent found doc 29 stale on regex; the same
skepticism applied here surfaced an equal number of staleness items.)

Research provenance (RESEARCH GRANT, standing): the ASGI 3.0 spec
(asgi.readthedocs.io/en/latest/specs/{main,www,lifespan}.html — semantics oracle),
Granian's RSGI 1.5 protocol (github.com/emmett-framework/granian/blob/master/docs/spec/
RSGI.md — the closest prior art for a Rust-core + compiled-app dispatch; MIT/BSD, ideas +
shape), uvicorn HttpToolsProtocol + FlowControl backpressure model (encode/uvicorn, BSD —
study), hypercorn sans-io h11/h2/wsproto worker architecture (pgjones/hypercorn, MIT),
the Rust crates `h2` 0.4 (hyperium, MIT — Tokio-bound; passes h2spec), `hreq-h2`
(algesten, the Tokio-free `futures`-IO fork — MIT), `httparse` 1.x (seanmonstar, MIT/Apache),
`tungstenite` 0.29 (snapview, MIT/Apache — already in tree), `rustls` 0.23 (MIT/Apache/ISC —
already in tree; ALPN + Resumption + ServerConfig::ticketer APIs), `mio` 1.2 + `socket2`
0.6 (already in tree), the Autobahn|Testsuite (crossbario, Apache — ws conformance oracle,
fuzzingclient/fuzzingserver, 500+ cases), h2spec (summerwind/imcom — HTTP/2 conformance
oracle), TechEmpower FrameworkBenchmarks methodology (plaintext pipeline-depth-16 + JSON
keep-alive, wrk; archived 2026-03-24 but methodology stands), CPython http.client/
http.server/socketserver/wsgiref/socket/ssl (PSF — semantics reference only),
wasi-sockets preview2 (WebAssembly/wasi-sockets — 0.3.0 non-blocking, Feb 2026). Cited
inline. License discipline: study + reimplement; PSF + the MIT/Apache crates are directly
usable (tungstenite/rustls/mio/socket2/httparse/h2/hreq-h2); RSGI shape is a published
spec; no GPL code ingested.
-->

# Web Stack Frontier (Design 35)

**Document status:** Implementation-ready frontier design. **Web-stack root doc** (HTTP
client + server, TLS, sockets, WebSockets, ASGI server binary).
**Scope:** The complete web story for molt across all targets (native macOS/Linux,
WASM-browser, WASI, Luau) and all profiles. Defines the **standalone ASGI server binary**
(`molt build app.py` where `app.py` is a Starlette/FastAPI-style ASGI app produces a
native server with a Rust I/O core dispatching into the *compiled* Python app — the
structural edge over Granian, which dispatches into *interpreted* CPython), the
**parity-required** stdlib closure (http.client/http.server/socketserver/wsgiref/socket/
ssl byte-identical to CPython 3.12+), the **molt-native** server runtime (`molt.serve`) +
h2 + ws-server APIs, and the **unleashed** opt-ins. Three surface tiers with sharp
boundaries (the doc-33 §1.4 template). Dispatches through doc-26 (async generators) +
doc-28 (asyncio runtime) + doc-33 (workers); §10 states exactly which phases block on
those landing vs which proceed now.

> **Mandate alignment.** Performance contract: molt MUST beat every CPython-based stack
> (uvicorn/granian/hypercorn/gunicorn on CPython 3.12) on every web bench, every target,
> every profile (§9 names the multipliers). Parity contract: stdlib http/socket/ssl
> byte-identical to CPython 3.12+ except the four global carve-outs (no exec/eval/compile,
> no runtime monkeypatch, no unrestricted reflection — none of which touch this subsystem).
> Every UNLEASHED deviation names the exact guarantee it trades (§3).

---

## 0. The one-paragraph answer

The Year-5 shape is a **Rust I/O core that owns accept/parse/TLS/h2/ws-framing and
dispatches the compiled Python ASGI app through a molt-native `protocol` object backed by
the doc-28 `MoltBuffer` pool — never a fresh Python dict per request.** This is RSGI's
structural insight (Granian's `protocol` object is "a lightweight Python object backed
directly by Rust memory" — RSGI spec) made *more* potent because molt's app is compiled
native code, not interpreted bytecode: there is no interpreter-dispatch tax on either side
of the boundary. Crates that win: **`httparse` replaces the hand-rolled HTTP/1.1 parser**
(7338-line `functions_http.rs` — §0.1-A), **`hreq-h2` (Tokio-free h2)** for HTTP/2 over
molt's own mio/io_uring loop (NOT `h2` proper, which hard-binds Tokio — §6.3), **`rustls`
0.23 ALPN + Resumption + ticketer** (already in tree; three missing config lines — §4),
**`tungstenite` 0.29 server-side** (already in tree, currently client-only — §0.1-B, §5),
**`mio` 1.2 + `socket2` 0.6 + `SO_REUSEPORT`** (already in tree; the constant exists in
`platform_env_ffi.rs:330,537,634` — §7) on the **doc-28 io_uring(Linux)/kqueue(macOS)**
backend with **doc-33 spawn-isolate SO_REUSEPORT workers** for multi-core. No `hyper`, no
`tokio` (the <2MB mandate and the GIL-serialized single-loop model both reject the Tokio
multi-thread runtime — §6.3). The server binary is **MOLT-NATIVE NEW SURFACE** (additive,
like `molt.gpu`); the stdlib closure is **PARITY-REQUIRED**; zero-copy bodies / header
interning / aggressive connection reuse are **UNLEASHED** opt-ins (§3).

---

## 0.1 Corrections to the doc-29 SUBSYSTEM 4 audit (re-audited at HEAD)

Doc 29 SUBSYSTEM 4 (`docs/design/foundation/29_domain-critical-portfolio.md:127-157`) was
written at `951938075`. Re-auditing at HEAD `fcf949af1` surfaces six material corrections.
None invalidate doc-29's *verdict* (NEEDS-FRONTIER-DOC), but they change the *scope* and
the *starting line*.

- **A — "manual HTTP/1.1 parser, ~100 lines" understates by 70×.** `functions_http.rs`
  (the live copy is the `molt-runtime-http` crate version,
  `runtime/molt-runtime-http/src/functions_http.rs`, **7338 lines** — `wc -l`) is a *large*
  hand-rolled stack: urllib split/quote/encode (`urllib_urlsplit_impl:328`,
  `urllib_quote_impl:386`), a full http.client connection model (`MoltHttpClientConnection:205`,
  `MoltHttpClientConnectionRuntime:222`), http.server response building (`HTTP_SERVER_HTTP11:896`,
  `:1358`), socketserver intrinsics (`molt_socketserver_serve_forever:4604`), and the actual
  request executor (`urllib_http_send_request:3562`). Doc-29's "no h2/keep-alive/pipelining"
  is **CORRECT and confirmed** (§0.1-C), but the deletion/replacement surface is 7338 lines,
  not 100. **Correction: the httparse swap (§6.1) is a large, bounded refactor, not a
  drop-in.** Note also the **dual copy**: `runtime/molt-runtime/src/builtins/functions_http.rs`
  is compiled only `#[cfg(not(feature = "stdlib_http"))]` (`builtins/mod.rs:59-60`); the
  `molt-runtime-http` crate copy is live when `stdlib_http` is on. Any parser change must
  touch both or the split must first be collapsed.

- **B — WebSocket is wired, but CLIENT-ONLY.** Doc-29 says "tungstenite ... Python-facing
  API wiring needs to be audited." Audited: it **exists** and is **substantive** —
  `runtime/molt-runtime/src/async_rt/channels.rs:63-65` imports `tungstenite::{Message,
  WebSocket, connect, stream::MaybeTlsStream}`; `molt_ws_connect:2564`, `molt_ws_send:3335`,
  `molt_ws_recv:3385`, `molt_ws_wait_new:2814`, `molt_ws_close:3422` are real, integrated
  with the io-poller (`molt_ws_wait` registered at `io_poller.rs:309`). WASM delegates to
  host imports (`molt_ws_connect_host`/`molt_ws_poll_host`/`molt_ws_send_host`/
  `molt_ws_recv_host`, `lib.rs:1110-1124`). **BUT there is no `tungstenite::accept` / server
  handshake anywhere** — grep for `tungstenite::accept`/`Role::Server`/`accept_hdr` returns
  zero. **Correction: the ws-CLIENT is built; the ws-SERVER (accept side, the ASGI
  `websocket` scope) is the actual gap (§5).** A `net.listen` capability string exists
  (`channels.rs:2173,2424,2515`) but only the connect path uses it.

- **C — HTTP client confirmed one-shot, no keep-alive (worse than doc-29 implies).** The
  client struct `MoltHttpClientConnection` (`functions_http.rs:205-220`) holds **no socket
  field** — it buffers `headers`/`body` and the executor `urllib_http_send_request:3562`
  does `TcpStream::connect((host, port))` per request (`:3571`), force-writes
  `Connection: close` (`:3422`), then `read_to_end` (`:3592`) and drops the socket. **There
  is no persistent connection, no pool, no pipelining, and the path is fully BLOCKING
  (`std::net::TcpStream`, not the mio socket).** `http.client.HTTPConnection` semantics
  (persistent connection object reused across `.request()`/`.getresponse()`) are therefore
  *not* honored — each call reconnects. This is a **parity gap**, not just a perf gap (§4.3,
  §6.2).

- **D — the event loop and the I/O poller are SEPARATE, on different threads.** Doc-29 cites
  `event_loop.rs:1-100` as "the correct foundation ... no receive/send abstraction." Correct,
  but the mio `Poll` does **not** live in `event_loop.rs` — it lives in
  `runtime/molt-runtime/src/async_rt/io_poller.rs` (**1573 lines**), which owns
  `Poll::new()` (`:423`), a `Waker` (`:425`), and runs a **dedicated OS thread** `io_worker`
  (`:1171`) that polls with a 250ms timeout (`:1181`) and marks futures ready for the
  GIL-held loop to drain. `event_loop.rs` is the ReadyQueue/TimerHeap + `add_reader`/
  `add_writer` registration façade (`:365,394`). **Correction: the server's accept loop and
  receive/send plumb into `io_poller.rs` (the real demultiplexer), not `event_loop.rs`. This
  is the doc-28 §2.4 io_uring/kqueue replacement target — the server inherits whatever
  backend doc-28 lands.**

- **E — the SSL getter surface is broad; the gaps are precise.** Doc-29 lists "missing
  recv/send MSG_* flags, session resumption, ALPN completeness, client-cert auth." Audited:
  `ssl.rs` (1294 lines) **has** `do_handshake:834`, `read:920`, `write:975`,
  `getpeercert:1047`, `cipher:1103`, `version:1160`, `unwrap:1195`, mTLS cert-chain load
  (`:213`, applied client-side at `:720+`), verify-mode get/set (`:426,447`). The **precise**
  missing items are: (1) **ALPN — zero references** in `ssl.rs` (grep `alpn` = 0); the
  `ClientConfig`/`ServerConfig` builders (`:716+`, `:656+`) never call `.alpn_protocols`,
  and there is no `selected_alpn_protocol()` getter (grep = 0). (2) **Session resumption —
  not configured** (no `.resumption(...)` on client, no `ServerConfig::ticketer`/
  `session_storage`). (3) **`recv`/`send` MSG_* flags** raise `NotImplementedError` at the
  **Python** layer (`src/molt/stdlib/ssl.py:121,126,138,143`), not in Rust. (4)
  **`wrap_bio`** (MemoryBIO) raises `NotImplementedError` (`ssl.py:303`) — a 5th gap doc-29
  did not list, and the one that **blocks the async-TLS server** because rustls is driven via
  the blocking `StreamOwned<Conn, std::net::TcpStream>` (`ssl.rs` `SslSocketInner`), which
  cannot integrate with the non-blocking mio loop. **Correction: the server-critical SSL gap
  is not "ALPN completeness" — it is the absence of a buffer-driven TLS state machine
  (rustls `Connection::{read_tls,process_new_packets,write_tls}` over `MoltBuffer`), of which
  `wrap_bio` is the stdlib face (§4.2).**

- **F — socketserver/wsgiref already route through Rust intrinsics, via POLLING not events.**
  `src/molt/stdlib/socketserver.py:18-35` binds `molt_socketserver_serve_forever`,
  `_handle_request`, `_dispatch_begin/poll/cancel`, `_get_request_poll` — real intrinsics
  (`functions_http.rs:4453-4710`). But `molt_socketserver_serve_forever:4604` is a
  **busy-poll loop** with `std::thread::sleep(poll_interval.min(0.05))` (`:4649`) — not
  event-driven. `wsgiref.simple_server.WSGIServer` subclasses `socketserver.TCPServer`
  (`wsgiref/simple_server.py:179`). **Correction: the parity server path exists and works,
  but its 50ms poll granularity is a latency floor; §8-P1 re-bases it onto the io-poller's
  edge-triggered readiness (parity-preserving, removes the sleep).**

**Net:** doc-29's verdict stands; the *work* is (i) replace a 7338-line hand-parser with
httparse, (ii) build the ws-SERVER accept side (client is done), (iii) give rustls a
buffer-driven (sans-io) TLS path so TLS works on the async loop, (iv) add 3 rustls config
lines for ALPN + resumption, (v) make the http-client persistent + async, (vi) build the
NEW ASGI server runtime on top. Items (iii)+(iv)+(v) are independent of docs 26/28/33 and
**proceed now**; (vi) blocks on 26+28 (§10).

---

## 1. END-STATE: the standalone ASGI server binary

### 1.1 The compile-time shape

```
$ molt build app.py --emit server            # app.py exposes `app` (ASGI 3 callable)
  → ./app                                     # standalone native server binary
$ ./app --bind 0.0.0.0:8000 --workers 4 --http auto --ssl-certfile cert.pem
```

`molt build` already produces standalone binaries (the whole project thesis). The new
`--emit server` flag (or auto-detection: the module exposes a top-level name bound to an
object that is `async-callable` with arity 2 — ASGI `(scope, receive, send)` — or arity 1
returning such; mirrors uvicorn's `--factory`) links the **server runtime** (`molt.serve`,
§1.5) as the binary's `main`, with the compiled app as the dispatch target. The app's
`async def app(scope, receive, send)` is **compiled to native code** via the existing
async-codegen pipeline (doc-26 `_poll` heap frames); the server calls it directly through
`call_callable3` (the path doc-29 §145 already identified). No CPython, no interpreter — the
ASGI callable boundary is two native function pointers exchanging NaN-boxed handles.

**Why this beats Granian structurally.** Granian's Rust core is excellent, but the app it
dispatches into is *interpreted CPython* — every `await receive()`, every header access,
every `await send(...)` crosses the C-API boundary and runs bytecode. molt's app is native:
`await receive()` lowers to a `_poll` on a heap frame that resumes inside the Rust runtime
(doc-26 §StateSwitch, `runtime/molt-runtime/src/async_rt/scheduler.rs`); the buffer it
yields is a `MoltBuffer` slice (doc-28 §2.5), not a freshly-allocated Python `bytes` marshalled
across pyo3. The per-request Python-side cost is *compiled field loads*, not interpreter
dispatch. This is the molt edge; §9 quantifies it.

### 1.2 The dual interface: ASGI (parity) + RSGI-class native (unleashed)

molt offers **two** application interfaces, exactly as Granian offers `asgi` and `rsgi`:

1. **ASGI 3.0 (default, MOLT-NATIVE NEW SURFACE, spec-faithful).** `async def app(scope,
   receive, send)`. `scope` is a real `dict` (parity with the ecosystem: Starlette/FastAPI/
   Django-ASGI/Litestar consume `scope` as a dict — `scope["headers"]`, `scope["path"]`).
   `receive`/`send` are molt-native awaitables (§2). This is what runs FastAPI unmodified.
   The dict is the compatibility tax; §3-U2 makes it interned/lazy under unleashed.

2. **MSGI — Molt Server Gateway Interface (UNLEASHED opt-in, RSGI-shaped).** For molt-native
   apps that opt into `@molt.serve.msgi`, the app receives `(scope, protocol)` where `scope`
   is a **lazy Rust-backed view** (field access reads `MoltBuffer` slices directly, no dict
   materialization) and `protocol` is the receive/send transport object (RSGI's `protocol()`
   → body bytes, `protocol.response_bytes(...)`, `async for chunk in protocol`, and for ws
   `await protocol.accept()` → transport with `receive()`/`send_bytes()`/`send_str()` — RSGI
   spec method names mirrored). This trades ASGI-dict-identity for zero-dict-allocation; named
   in §3-U2. **MSGI is molt's own name** (not "RSGI") because RSGI is Granian's spec and we do
   not claim wire-compat with it; we mirror its *shape* (provenance honored), and our
   `protocol` is backed by molt's `MoltBuffer`, not Granian's Rust types.

The ASGI path is the contract; MSGI is the superpower. A FastAPI app gets the ASGI path for
free; a throughput-maximal molt service opts into MSGI and pays no dict tax.

### 1.3 Connection lifecycle manager (Rust core)

Per accepted connection, the Rust core (`molt-runtime-net`, the crate that already gates
`mio`+`socket2`+`rustls`+`tungstenite` — `Cargo.toml:22`) runs a state machine. This is
the uvicorn `HttpToolsProtocol`/`FlowControl` model (encode/uvicorn) reimplemented in Rust
over the io-poller:

```
Accept (SO_REUSEPORT listener, §7)
  → [TLS?] rustls sans-io handshake over MoltBuffer (§4.2); ALPN selects h2|http/1.1
  → protocol demux:
      h2  → hreq-h2 server connection (§6.3): per-stream → one ASGI scope, concurrent
      h11 → httparse incremental parse (§6.1): one request → one ASGI scope; keep-alive loop
      ws-upgrade (Connection: Upgrade, h11) → tungstenite server handshake (§5) → ws scope
  → construct scope (§2.1) → spawn an eager molt Task (doc-28 §eager tasks) running the
    compiled app(scope, receive, send) → drive receive/send (§2.2) with backpressure (§2.3)
  → on app return / disconnect / error: graceful teardown, return MoltBuffers to pool,
    keep-alive → loop to next request on the same connection (h11) or close.
```

The state machine is **one `enum ConnState` per connection** held in an io-poller-registered
slot (`io_poller.rs` socket entry, `:1196`), advanced on readiness edges. No thread per
connection; N connections multiplex on the loop thread, exactly the asyncio model.

### 1.4 Lifespan protocol (ASGI lifespan sub-spec)

On server start, before accepting, the core invokes the app **once** with
`scope = {"type": "lifespan", "asgi": {...}, "state": {}}` and drives the lifespan handshake
(ASGI lifespan spec): send `{"type": "lifespan.startup"}`, await
`lifespan.startup.complete` | `lifespan.startup.failed` (on failed → log + exit non-zero,
parity with uvicorn). The `lifespan.shutdown` half runs on SIGTERM/SIGINT (§1.6). The
existing `moltlib/asgi.py:89-97` adapter already implements the *app-side* lifespan loop;
the server provides the *driver* side. `scope["state"]` (the ASGI `state` extension for
sharing startup-initialized objects with request scopes — e.g. a DB pool) is threaded
through as a shared `dict` referenced by every subsequent request scope (spec: "a copy of
... state ... must be passed into every connection scope").

### 1.5 `molt.serve` — the MOLT-NATIVE server module

New stdlib-adjacent module `src/molt/serve/__init__.py` (NOT a CPython module — additive,
like `molt.gpu`). Surface:

```python
import molt.serve
molt.serve.run(app, bind="0.0.0.0:8000", workers=4, http="auto",
               ssl_certfile=None, ssl_keyfile=None, ssl_alpn=("h2","http/1.1"),
               lifespan="auto", backlog=2048, limit_concurrency=None,
               h11_max_incomplete=16384, ws="auto", ws_max_size=16*1024*1024)
# or the programmatic Server object for embedding:
server = molt.serve.Server(config); await server.serve()
```

`run` is the entry `--emit server` wires to `main`. The bulk of `molt.serve` is a thin
Python orchestration shell over Rust intrinsics (`molt_serve_*`); the hot path never returns
to Python except to call the app. This mirrors how `socketserver.py` is already a shell over
`molt_socketserver_*` (§0.1-F) — the proven pattern.

### 1.6 Graceful shutdown

SIGTERM/SIGINT → stop accepting (close listeners), set a draining flag; in-flight requests
finish within `timeout_graceful_shutdown` (default 30s, uvicorn parity); idle keep-alive
connections receive `Connection: close` on their next response or are closed immediately;
lifespan.shutdown is driven; then exit 0. WebSocket connections receive a close frame
(code 1001 "going away"). This is the uvicorn shutdown sequence; signal handling routes
through molt's existing main-thread signal path (doc-33 §B6 confirms signals are
main-thread-only, between bytecodes — the draining flag is set by the handler, observed by
the loop).

---

## 2. receive / send / scope — the molt-native awaitable transport (doc-28 buffer pool)

### 2.1 Scope construction (zero-copy where possible)

The Rust core builds the ASGI HTTP scope per the WWW spec
(asgi.readthedocs.io/.../www.html). Field sources:

| ASGI scope key | Source | Cost |
|---|---|---|
| `type` | constant `"http"`/`"websocket"`/`"lifespan"` | interned static |
| `asgi` | `{"version":"3.0","spec_version":"2.4"}` | one shared dict |
| `http_version` | httparse / hreq-h2 (`"1.0"`/`"1.1"`/`"2"`) | interned static str |
| `method` | httparse `req.method` | interned for the 8 standard verbs |
| `scheme` | `"http"`/`"https"` (TLS present?) | interned static |
| `path`, `raw_path` | httparse `req.path` (percent-decoded vs raw) | `MoltBuffer` slice → str |
| `query_string` | bytes after `?` | `MoltBuffer` slice (bytes) |
| `headers` | `[(name_lower_bytes, value_bytes), ...]` | list of 2-tuples over slices |
| `client`/`server` | peer/local `SocketAddr` | `(host_str, port_int)` |
| `state` | shared lifespan state dict (§1.4) | shared ref |

Under **ASGI default**, `headers` is a real Python `list[tuple[bytes,bytes]]` (ecosystem
contract). The header *bytes* are `MoltBuffer` slice views into the parse buffer (no copy),
which is sound only while the buffer outlives the scope — guaranteed because the scope's
lifetime is the request and the parse buffer is held by `ConnState` for the request
(refcounted, doc-28 §2.5). Under **MSGI unleashed**, the whole scope is a lazy view object
(§3-U2).

### 2.2 receive / send as `MoltBuffer`-backed awaitables

`receive` and `send` are **molt-native awaitable callables** (not Python closures over a
queue — that is uvicorn's model and it costs an interpreter frame per call). They are
intrinsic-backed awaitables (`molt_serve_receive`/`molt_serve_send`) that, when awaited,
either complete immediately (data already buffered) or suspend the calling Task on the
io-poller (doc-28 intrusive ready list), exactly like `asyncio.StreamReader.read`.

- **`await receive()` (http)** → returns one ASGI event dict: `{"type":"http.request",
  "body": <MoltBuffer-slice bytes>, "more_body": bool}` as body chunks arrive, then
  `{"type":"http.disconnect"}` on EOF/reset. The `body` value is a `bytes` object whose
  storage is a `MoltBuffer` slice (doc-28 §2.5 memoryview semantics) — under ASGI default it
  is a *copy into an immutable `bytes`* (bytes-immutability parity, §3); under unleashed
  zero-copy-bodies (§3-U1) it is a view.
- **`await send(event)` (http)** → `http.response.start` (status + headers → serialize
  status line + headers into a `MoltBuffer`, queue for write) and `http.response.body`
  (body bytes → queue; `more_body=False` finalizes, applies chunked TE or Content-Length).
  Writes go through the io-poller writable-readiness path; a full socket buffer suspends the
  sender (backpressure, §2.3).
- **WebSocket** events (`websocket.connect`/`accept`/`receive`/`send`/`disconnect`/`close`)
  map to tungstenite frames (§5).

The event *dicts* are the ASGI contract (parity). The *bytes inside them* ride the buffer
pool. This is the seam where molt is faster without breaking the spec.

### 2.3 Backpressure / flow control (uvicorn FlowControl, in Rust)

Reimplement uvicorn's `FlowControl` (encode/uvicorn — high/low watermark) in the Rust
`ConnState`:

- **Request-body high-water:** if the app is slow to `await receive()` and the kernel buffer
  fills, the core stops issuing `mio` read-interest for that socket (pauses reading) until
  the app drains; resumes read-interest when the app awaits again. Prevents unbounded
  in-memory body buffering.
- **Response-write high-water:** if the app `send`s faster than the socket drains,
  `await send(...)` suspends the sender Task once the queued `MoltBuffer` bytes exceed the
  high-water mark (default 64KiB), resuming when the write buffer drains below low-water.
  This makes `send` honestly awaitable (an app that ignores backpressure cannot OOM the
  process — directly serves the CLAUDE.md "never OOM the host" rule).
- **h2 flow control:** hreq-h2 surfaces per-stream and connection `WINDOW_UPDATE`; the core
  maps stream-window exhaustion to sender suspension (h2 has its own flow control on top of
  TCP; both are honored).

### 2.4 Concurrency: one eager Task per request

Each request spawns an **eager Task** (doc-28 eager-task optimization: the app runs
synchronously until its first real suspension, so request handlers that never await — the
plaintext bench — never touch the scheduler queue). Tasks are GIL-serialized (default tier);
under the doc-33 unleashed free-threading tier, request Tasks on a free-threaded worker run
on real cores (§7.3). The connection lifecycle (accept/parse/write) is core-side Rust and
runs without spawning a Task — only the app invocation is a Task.

---

## 3. Surface classification (the three tiers — sharp boundaries)

Following the doc-33 §1.4 template: DEFAULT tier surrenders nothing vs CPython; UNLEASHED
deviations each name the exact traded guarantee.

### 3.1 PARITY-REQUIRED (stdlib, byte-identical CPython 3.12+)

`http.client`, `http.server`, `socketserver`, `wsgiref`, `socket`, `ssl`, `urllib.request`/
`urllib.parse`, `http.cookies`/`http.cookiejar`. These must match CPython byte-for-byte
including error messages and edge cases. **Enumerated gaps to close** (verified at HEAD):

| # | Gap | Site (HEAD anchor) | Fix (phase) |
|---|---|---|---|
| G1 | `ssl.SSLSocket.recv/recv_into/send/sendall` raise on any `flags` arg | `ssl.py:121,126,138,143` | §4.1 — plumb MSG_* through rustls read/write (only MSG_PEEK/WAITALL meaningful over TLS; others map to socket-level) |
| G2 | `ssl` ALPN absent (no `set_alpn_protocols`, no `selected_alpn_protocol`) | `ssl.rs` (0 `alpn` refs) | §4.1 — `ClientConfig/ServerConfig.alpn_protocols` + getter |
| G3 | `ssl` session resumption not configured | `ssl.rs` (no `.resumption`/ticketer) | §4.1 — client `Resumption`, server `ticketer` |
| G4 | `ssl.SSLContext.wrap_bio` (MemoryBIO) raises | `ssl.py:303` | §4.2 — sans-io rustls path (also unblocks async-TLS server) |
| G5 | `http.client.HTTPConnection` does not persist the socket (reconnects per request, forces `Connection: close`) | `functions_http.rs:3422,3562-3592` | §6.2 — persistent socket on the connection object |
| G6 | `socket.socket.recv/send` MSG_* flag coverage | `socket.py` (audit §4.3) | §4.3 — socket2 supports flags; surface them |
| G7 | `socketserver.serve_forever` 50ms busy-poll latency floor | `functions_http.rs:4649` | §8-P1 — re-base on io-poller readiness (parity-preserving) |
| G8 | client-cert *verification* server-side (`with_no_client_auth` always) | `ssl.rs:656` | §4.1 — `WebPkiClientVerifier` when `verify_mode==CERT_REQUIRED` server-side |

Closing G1–G8 makes the stdlib tier byte-identical. No new surface; pure parity work.

### 3.2 MOLT-NATIVE NEW SURFACE (additive, not parity-trading)

`molt.serve` (the ASGI server runtime, §1.5), the h2 server/client (§6.3), the WebSocket
**server** API (§5), MSGI (§1.2). These are new — CPython has no ASGI server in the stdlib —
so they trade *nothing* from parity; they are the `molt.gpu`-class additive superpower. They
must, however, conform to the **ASGI 3.0 spec** (the ecosystem contract) and pass the
ecosystem test suites (Starlette/FastAPI app run unmodified — §8 conformance).

### 3.3 UNLEASHED (explicit opt-in; each trades a named guarantee)

| # | Unleashed feature | Opt-in | DEFAULT guarantee | What it trades |
|---|---|---|---|---|
| U1 | **Zero-copy request bodies as buffer views** | `@molt.serve.zerocopy_body` or `--unleashed` | `receive()` body is an immutable `bytes` (independent copy; mutating the socket buffer cannot affect it; `id()`/buffer-protocol report a private object) | the `body` is a `memoryview` over the live `MoltBuffer` parse slice. Trades **bytes-immutability *observability*** — the bytes are still logically read-only, but they are not a private copy: the buffer is recycled after the request, so holding the view past the request handler is UB (documented). Saves one memcpy per body chunk. |
| U2 | **MSGI lazy scope (no dict)** | `@molt.serve.msgi` | `scope` is a real `dict`; `scope["headers"]` is a `list`; identity/iteration are dict semantics | `scope` is a Rust-backed lazy view; field reads hit `MoltBuffer` slices. Trades **scope-is-a-dict identity** (`isinstance(scope, dict)` is False; `scope.copy()`/mutation differ). Apps written to ASGI-dict assumptions break; molt-native apps gain zero scope allocation. (This is exactly RSGI vs ASGI in Granian.) |
| U3 | **Header-name interning / shared header dict** | `--unleashed` | each request's headers are fresh objects | header *names* are interned to shared immortal `bytes`; a frequently-seen full header set may be served from an interning cache. Trades **header-object freshness** (`a_req_header is b_req_header` may be True for equal names). No semantic change to values; only identity. |
| U4 | **Connection-reuse aggressiveness beyond http.client defaults** | `molt.serve`/client `keepalive=aggressive` | client `http.client` opens/closes per CPython usage; pool size = explicit | the client keep-alive pool reuses connections opportunistically (HTTP/2 multiplexing, h11 keep-alive with larger idle pools than CPython's none). Trades **connection-count observability** (a server sees fewer TCP connections than a CPython client would make; FD/connection accounting differs). Pure perf; no wire-semantic change. |
| U5 | **`response_file` / sendfile zero-copy response** | MSGI `protocol.response_file` | response body bytes pass through Python `send` | the core `sendfile(2)`s a file directly to the socket (or io_uring `IORING_OP_SENDFILE`/splice), never copying into Python. Trades nothing semantically; requires the doc-36 `os.sendfile` intrinsic (dependency, §10). RSGI `response_file`/`response_file_range` shape. |

**The contract in one line:** the DEFAULT/ASGI tier surrenders nothing — a FastAPI app runs
with byte-identical ASGI semantics; UNLEASHED surrenders exactly {U1 body-copy-privacy, U2
scope-dict-identity, U3 header-identity, U4 connection-count-observability} — each a perf
trade with no value-level semantic change, opt-in per build or per decorator.

---

## 4. TLS / SSL — closing the parity gaps + the async-TLS state machine

### 4.1 The three config-line gaps (G2/G3/G8) + flags (G1)

These are **independent of docs 26/28/33** and proceed now (§10). Against `rustls` 0.23
(already in tree):

- **ALPN (G2):** add `config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()]`
  (or the user's list) in both `ClientConfig` (`ssl.rs:716+`) and `ServerConfig`
  (`ssl.rs:656+`) builders, fed by a new `molt_ssl_context_set_alpn_protocols` intrinsic
  (backing `SSLContext.set_alpn_protocols`, currently absent). Add
  `molt_ssl_socket_selected_alpn` reading `conn.alpn_protocol()` post-handshake (backing
  `SSLSocket.selected_alpn_protocol()`). **This is what lets the server negotiate h2 vs
  http/1.1** — the keystone for §6.3.
- **Session resumption (G3):** client — keep a process-wide `Arc<ClientSessionMemoryCache>`
  (or the default `Resumption`), shared across `ClientConfig`s (rustls docs: shared
  `Resumption` improves rates). Server — install a `ServerConfig::ticketer`
  (`rustls::crypto::ring::Ticketer` or aws-lc-rs) for stateless TLS1.3 tickets +
  `send_tls13_tickets`. Note the rustls-known ALPN/resumption storage-key interaction
  (rustls#2196 — storage keyed on SNI alone); molt keys its client cache on `(sni,
  alpn_set)` to avoid the cross-protocol confusion.
- **Client-cert verification (G8):** server-side, when `verify_mode == CERT_REQUIRED`, build
  `ServerConfig::builder().with_client_cert_verifier(WebPkiClientVerifier::builder(roots)
  .build()?)` instead of `with_no_client_auth()` (`ssl.rs:656`). mTLS cert *presentation*
  already exists client-side (`ssl.rs:720+`); this adds the verification half.
- **MSG_* flags (G1/G6):** over TLS, only `MSG_PEEK`/`MSG_WAITALL` are meaningful and map to
  buffering behavior on the plaintext side of rustls; `MSG_DONTWAIT` maps to the socket's
  non-blocking mode; `MSG_OOB` is unsupported over TLS (raise `ssl.SSLError`, parity:
  CPython's OpenSSL also rejects OOB over TLS). Plain-socket MSG_* (G6) is `socket2`-native.

### 4.2 The async-TLS state machine (G4 — the server-critical one)

Today `ssl.rs` wraps `StreamOwned<Connection, std::net::TcpStream>` — **blocking**, and
unusable on the non-blocking mio loop. The server needs **sans-io rustls**: drive
`rustls::ServerConnection`/`ClientConnection` as a pure state machine over `MoltBuffer`s:

```
readable edge  → conn.read_tls(&mut io_buf)         // feed ciphertext from MoltBuffer
               → conn.process_new_packets()          // advance handshake / decrypt
               → conn.reader().read(plaintext_buf)    // pull plaintext → app receive()
writable edge  → conn.writer().write(app_send_bytes)  // push plaintext
               → conn.write_tls(&mut out_buf)         // pull ciphertext → socket write
```

This is exactly what `SSLContext.wrap_bio(incoming, outgoing)` (G4) exposes at the Python
layer — `MemoryBIO` *is* the sans-io interface. So **fixing G4 (wrap_bio) and building the
async-TLS server are the same work**: implement the BIO-driven `SslSocketInner::MemoryBio`
variant, and the server's TLS handler is `wrap_bio` driven by the io-poller. `asyncio`'s
existing `sslproto.py` (`src/molt/stdlib/asyncio/sslproto.py`, 1.5K) is the parity consumer.
**This is the riskiest single piece of the TLS work** (handshake state across many readiness
edges, renegotiation, close_notify ordering) — flagged.

### 4.3 socket.py MSG_* audit

`src/molt/stdlib/socket.py` (32.1K) over `socket2` (which exposes `recv_with_flags`/
`send_with_flags`). Audit each `recv`/`recvfrom`/`recv_into`/`send`/`sendto` for `flags`
handling; `socket2` supports MSG_* natively, so G6 is a surfacing task, not a new mechanism.

---

## 5. WebSockets — building the SERVER side (client is done)

§0.1-B established: the ws **client** is built (`channels.rs` tungstenite connect/send/recv,
io-poller-integrated; WASM host-delegated). The **server** (accept side) is the gap.

**Design (native):** on an h11 connection with `Upgrade: websocket` + `Connection: Upgrade`
+ `Sec-WebSocket-Key`, the core runs `tungstenite::accept_hdr` (server handshake, computing
`Sec-WebSocket-Accept`, negotiating subprotocol from `Sec-WebSocket-Protocol` per the ASGI
`websocket.accept` `subprotocol` field). The resulting `WebSocket<TlsOr Plain Stream>` is
held in `ConnState::Ws`, registered on the io-poller for frame readiness. The ASGI websocket
scope (`type:"websocket"`, `subprotocols:[...]`, plus the http-like fields) is constructed;
the app is invoked; the message loop maps:

| ASGI ws message | tungstenite |
|---|---|
| app receives `websocket.connect` | synthesized on accept |
| app sends `websocket.accept` (+subprotocol) | complete handshake, send 101 |
| app sends `websocket.close` (+code) | `ws.close(CloseFrame{code,reason})` |
| app sends `websocket.send` {bytes|text} | `ws.send(Message::Binary|Text)` |
| app receives `websocket.receive` | `ws.read()` → Binary/Text → event |
| app receives `websocket.disconnect` (+code) | Close frame / EOF |

Ping/pong/fragmentation/UTF-8-validation/close-handshake are tungstenite's job (it is
RFC-6455 complete; that is why it is in the tree). permessage-deflate: tungstenite supports
it behind a feature; gate it (Autobahn §9–§13 cover it). The MSGI ws path (§1.2) exposes the
RSGI `protocol.accept()` → transport `receive()`/`send_bytes()`/`send_str()` shape directly.

**WASM-browser:** ws **server** is N/A (browsers cannot listen); the ws **client** uses the
host `WebSocket` API (`molt_ws_connect_host`, `lib.rs:1110`) — already built. State honestly:
no ws server in the browser target.

---

## 6. HTTP/1.1 + HTTP/2 — parser swap, persistent client, h2

### 6.1 Replace the hand-rolled HTTP/1.1 parser with `httparse` (DELETION)

`httparse` (1.x, MIT/Apache, already a dev-profile package in `Cargo.toml:95,214` — promote
to a real dep under the net feature) is a SIMD-capable, push-based, allocation-free
HTTP/1.x parser (the same one uvicorn's httptools and hyper use the spirit of). **Named
deletion:** the request-line/header/body framing in `functions_http.rs`
(`urllib_http_send_request`'s manual parse, the http.server request parsing, the chunked/
content-length handling at `:3343,3400-3425`) is **replaced** by httparse + a small
molt-owned chunked-TE codec. This is the §0.1-A 7338-line surface: urllib parse/quote/encode
*stays* (it is URL logic, not HTTP framing); the HTTP *wire* parsing is deleted in favor of
httparse. The dual-copy (§0.1-A) is collapsed first (one parser, in `molt-runtime-net` or
`molt-runtime-http`).

**Why httparse over rolling our own:** the hand parser is exactly the "regex/manual where
structural parsing belongs" smell the zero-workarounds policy rejects; httparse is the
structurally-correct, fuzzed, spec-complete parser. It also feeds the server core (one parser
for client and server).

### 6.2 Persistent + async HTTP client (G5 + keep-alive pool + pipelining)

Rebuild `MoltHttpClientConnection` (§0.1-C) to **hold its socket**: `HTTPConnection.connect()`
opens and *keeps* a `mio`-backed (async) or `socket2`-backed (sync-parity) stream;
`.request()`/`.getresponse()` reuse it across calls (CPython `http.client` semantics — G5
parity). Add a **keep-alive connection pool** keyed on `(scheme, host, port)` for
`urllib.request`/`requests`-style reuse (parity-preserving perf: CPython opens per request,
molt reuses — observable only as fewer TCP connections, the U4 unleashed trade for
*aggressive* reuse; conservative reuse honoring `Connection: keep-alive`/`close` headers is
default-tier safe). HTTP/1.1 **pipelining** (depth-N, the TechEmpower plaintext requirement,
§9) on the keep-alive socket. This is independent of docs 26/28/33 (it can use the existing
blocking path or the io-poller) and **proceeds now**.

### 6.3 HTTP/2 via `hreq-h2` (NOT `h2`, NOT `hyper`) — the dependency decision

**The constraint:** `h2` (hyperium, passes h2spec) is excellent but **hard-binds Tokio**
(its `AsyncRead`/`AsyncWrite` are Tokio's; it is "built on Tokio" — h2 README). molt's loop
is a **single GIL-serialized mio/io_uring loop**, NOT a Tokio multi-thread runtime; pulling
Tokio in would (a) duplicate the executor (two event loops), (b) violate the single-loop
asyncio model, (c) bloat the binary far past <2MB (Tokio + its deps). **`hyper` is worse** —
it is the full client+server+conn-management stack on Tokio; its non-goal-free surface is
megabytes and it owns connection management we already own.

**Decision: `hreq-h2`** (algesten/hreq-h2 — "the h2 crate with modifications to remove
dependencies on tokio, replacing tokio's AsyncWrite/AsyncRead with standard variants from the
`futures` crate"). It is the h2 *protocol* state machine (HPACK, framing, flow control,
multiplexing — h2spec-conformant by inheritance) with **no executor dependency**, driven over
molt's own buffers exactly like the sans-io rustls path (§4.2). The h2 frames are read/written
through `MoltBuffer`s on the io-poller; per-stream → one ASGI scope, concurrent on one
connection. This satisfies the <2MB mandate (protocol-only, no runtime) and the single-loop
model. **Provenance/risk:** hreq-h2 tracks h2 with a lag; if it falls too far behind, the
fallback is to vendor the Tokio-decoupling patch against current `h2` ourselves (the patch is
mechanical — swap the IO traits — and h2's protocol core is what we want). Flagged as the
**second-riskiest** piece (dependency freshness + h2 flow-control correctness on our loop).

**Binary-size estimate (feature-trimmed, from crate metadata):** `httparse` ~tens of KB,
`hreq-h2` (h2 core + `hpack`/`fnv`/`bytes`/`futures-core`) low-hundreds of KB, `rustls` 0.23
+ `aws-lc-rs`/`ring` is the dominant TLS cost (already paid — in tree), `tungstenite`
~low-hundreds KB (in tree). Net new for h2 ≈ a few hundred KB — within the <2MB envelope when
the server binary opts into the net feature (the server is necessarily larger than an empty
binary; the <2MB target is for the *default* non-server binary, which strips all of this via
the `stdlib_net`/`net`/`stdlib_http` feature gates — `Cargo.toml:96,22,15`).

### 6.4 Per-protocol selection

`--http auto` (default): ALPN selects h2 when the client offers it over TLS (§4.1);
cleartext defaults to h11 (h2c prior-knowledge optional, off by default — parity with
uvicorn). `--http 11`/`--http 2` force.

---

## 7. Per-target matrix (first-class designs, honest gaps)

| Target | Sockets | TLS | h11/h2 | ws server | ws client | Server (accept) | I/O backend |
|---|---|---|---|---|---|---|---|
| **native Linux** | mio+socket2 (built) | rustls + sans-io (§4.2) | httparse + hreq-h2 | tungstenite accept (§5) | built | **yes** | **io_uring** (doc-28 §2.4) + `SO_REUSEPORT` (§7.1) |
| **native macOS** | mio+socket2 (built) | same | same | same | built | **yes** | **kqueue** (doc-28 §2.4) + `SO_REUSEPORT` |
| **native Windows** | mio (IOCP) | same | same | same | built | yes (IOCP) | mio IOCP (no io_uring) |
| **WASM-browser** | **none** (no raw sockets — browser security) | host (TLS terminated by browser) | via host `fetch()` | **N/A** (cannot listen) | host `WebSocket` (built, `lib.rs:1110`) | **N/A** | host event loop |
| **WASI (preview2)** | wasi-sockets 0.3.0 (non-blocking, Feb 2026) | rustls (sans-io, no platform certs) | httparse + hreq-h2 (sans-io ⇒ portable) | tungstenite (sans-io core) | sans-io | **yes** (wasi-sockets `tcp.listen`/`accept`) | wasi-poll |
| **Luau** | host HTTP only | host | host | host | host | host-dependent | host |

**Honest statements:**
- **WASM-browser:** the server does not exist (browsers cannot accept TCP — confirmed:
  "no raw socket access in browser"). The **client** substrate is `fetch()` (for
  http.client/urllib → a host-import shim) and the host `WebSocket` API (built). A
  service-worker "edge server" pattern (the SW intercepts `fetch` events and dispatches them
  to the compiled ASGI app as synthetic scopes) is a **real and attractive** option — it
  turns a Cloudflare-Worker/SW into an ASGI host — and is called out as a **deferred
  exploration** (§8-P5 optional), not a Year-5 commitment, because it depends on the host
  `FetchEvent` shape, not on raw sockets.
- **WASI:** wasi-sockets reached non-blocking I/O in 0.3.0 (Feb 2026). Because httparse +
  hreq-h2 + tungstenite-core + rustls are all **sans-io** in molt's design (§4.2, §6.1,
  §6.3), the *same* protocol code runs on WASI by swapping the socket layer for wasi-sockets
  — this is the dividend of the sans-io decision. Platform cert roots are unavailable on WASI
  (bundle webpki-roots, already used — `ssl.rs:702`).
- **Luau:** no socket layer; HTTP is whatever the host exposes. State plainly: no native
  server.

### 7.1 Multi-core: `SO_REUSEPORT` workers via doc-33 spawn-isolate

`SO_REUSEPORT` already has its constant (`platform_env_ffi.rs:330,537,634`) and `asyncio`
already plumbs `reuse_port` to `setsockopt` (`asyncio/__init__.py:4264,5346`). The server's
`--workers N` (doc-33 spawn-isolate model): N worker processes (spawn, re-exec the binary
with a worker entry — doc-33 §multiprocessing-spawn), each opening its **own** listener on the
same `(addr, port)` with `SO_REUSEPORT`; the kernel load-balances accepts across workers.
This is the standard gunicorn/uvicorn `--workers` model and the doc-33 superpower (isolates
for multi-core). Each worker runs one GIL + one event loop (the per-interpreter-GIL spine,
doc-33 §1). **This blocks on doc-33's spawn protocol landing** (§10).

### 7.2 Single-worker concurrency

Within one worker, N connections multiplex on the one loop (§1.3) — this is the asyncio model
and needs only docs 26+28. Multi-worker (§7.1) is the doc-33 dependency. So **single-worker
high-concurrency serving ships before multi-core**, which is the right phasing (most of the
RPS win is single-core; cores multiply it).

### 7.3 Unleashed: free-threaded workers (doc-33 rung-e)

Under `molt build --unleashed` (doc-33 §3), a worker can run request Tasks on real threads
within one interpreter (free-threading). This is the doc-33 deep rung; the server inherits it
for free once doc-33 lands it (the request Task is already the unit of work, §2.4). Trades the
doc-33 §1.4 guarantees (compound-op atomicity etc.) — named there, not re-traded here.

---

## 8. Phased build plan (complete pieces, riskiest flagged)

Each phase is independently shippable and ends green on its named gate. LoC are Rust+Python
estimates for the phase's *new/changed* code.

**P0 — sans-io rustls + ALPN + resumption + cert-verify (G1–G4, G8).** ~900 LoC.
Independent of docs 26/28/33. Delivers: TLS works on the async loop (the `wrap_bio`/MemoryBIO
sans-io path), ALPN negotiation (unblocks h2), session resumption, mTLS verification, MSG_*
flags. **Riskiest sub-piece: the handshake state machine across readiness edges (§4.2).**
Gate: CPython `ssl` differential (handshake, getpeercert, cipher, version,
selected_alpn_protocol, resumption rate) + a rustls-loopback async-TLS echo test.

**P1 — httparse swap + persistent/pooled async HTTP client (G5, G7).** ~1200 LoC, **net
deletion** of the hand-rolled HTTP/1.x framing (§6.1). Independent of docs 26/28/33. Collapse
the dual `functions_http.rs` (§0.1-A) → one parser. Persistent `HTTPConnection` socket +
keep-alive pool + pipelining. Re-base `socketserver.serve_forever` off the 50ms poll onto
io-poller readiness (G7). Gate: CPython `http.client`/`urllib.request`/`http.server`
differential (status parsing, chunked TE, keep-alive reuse, header folding) + a fuzz oracle
on httparse inputs (extend doc-31's `fuzz_compiler.py` lane with an HTTP-request corpus).

**P2 — h2 (hreq-h2) client + server connection over the loop.** ~1500 LoC. **Second-riskiest
phase** (§6.3 — dependency freshness + flow-control correctness). Depends on P0 (ALPN) for
protocol selection; otherwise independent of docs 26/28/33 at the *protocol* layer (the
server *dispatch* layer is P4). Gate: **h2spec** (the HTTP/2 conformance oracle —
summerwind/h2spec, run against a molt h2 echo server) must pass; client interop against a
known h2 server (nghttp2).

**P3 — WebSocket SERVER accept side (§5).** ~700 LoC. Depends on P1 (h11 upgrade detection).
Independent of docs 26/28/33 for the framing; the ASGI ws *scope dispatch* is P4. Gate:
**Autobahn|Testsuite fuzzingclient** (tests our ws *server*) — §1–§13 (framing, fragmentation,
UTF-8, close, ping/pong, and §12–§13 permessage-deflate if enabled) must pass; the existing ws
*client* re-runs Autobahn fuzzingserver to guard against regression.

**P4 — the ASGI server runtime `molt.serve` (THE keystone).** ~2000 LoC. **Depends on doc-26
(async generators / coroutine `_poll` frames — the app's `async def` must be compilable and
resumable) AND doc-28 (the asyncio runtime — eager Tasks, intrusive ready lists, `MoltBuffer`
pool, io_uring/kqueue backend).** This is the connection lifecycle manager (§1.3), scope
construction (§2.1), receive/send awaitables (§2.2), backpressure (§2.3), lifespan (§1.4),
graceful shutdown (§1.6). Integrates P0/P1/P2/P3 as the protocol layers under it. Gate:
**run Starlette + FastAPI test apps unmodified** (the ASGI ecosystem conformance — the real
oracle); the ASGI-spec lifespan/http/ws message sequences; a soak test (10k connections, no
FD/buffer leak — ties to doc-28 RC).

**P5 — multi-core SO_REUSEPORT workers (§7.1) + MSGI (§1.2) + unleashed tier (§3.3).**
~900 LoC. **Depends on doc-33 (spawn protocol) for workers.** MSGI and the unleashed body/
scope/header trades layer on P4. Gate: TechEmpower-style RPS scaling across workers (linear to
core count); MSGI vs ASGI throughput delta measured; unleashed flags differential-tested to
confirm they change *only* the named observable (§3.3) and nothing else.

**(P6, optional/deferred) — WASI server + WASM service-worker edge pattern (§7).** Not a
Year-5 commitment; gated on wasi-sockets maturity + host `FetchEvent` design. Stated for
completeness.

**Dependency graph:** P0, P1 → now (parallel). P2 → after P0. P3 → after P1. **P4 → after
docs 26+28 land AND P0–P3.** P5 → after doc-33 + P4.

---

## 9. Benchmark lane (molt MUST beat every CPython stack)

TechEmpower methodology (plaintext at **pipeline depth 16**, JSON with **keep-alive**, wrk;
concurrency 256/1024/4096/16384 — the archived-but-canonical TFB spec). Targets vs
**CPython 3.12** stacks. Bench files to create (under `benchmarks/web/`, mirroring the
existing `bench_*.py` convention — there is currently **no web bench**; grep found only
`drivers/falcon/browser_webgpu/bench_hostfed.py`):

| Bench file | What | Baseline | molt target |
|---|---|---|---|
| `bench_serve_plaintext.py` | TFB plaintext, pipeline-16, RPS | uvicorn / granian / gunicorn (CPy 3.12) | **≥ granian, ≥ 3× uvicorn** (granian's Rust core is the bar; molt's compiled app should match or beat it because the app side is native too) |
| `bench_serve_json.py` | TFB JSON serialization, keep-alive, RPS | uvicorn / granian | **≥ granian, ≥ 3× uvicorn**; JSON via the doc-29 §3 serde path |
| `bench_serve_latency.py` | p50/p99 latency under load (256→16384 conc) | uvicorn / granian | **lower p99** (io_uring sub-ms notification, doc-28 §2.4 vs the 250ms poll floor / uvicorn selector) |
| `bench_http_client_throughput.py` | client GET/POST throughput w/ keep-alive + h2 | requests / httpx (CPy 3.12) | **≥ 2× requests** (persistent pool + h2 multiplexing vs CPython per-request connect) |
| `bench_tls_handshakes.py` | TLS handshakes/sec (full + resumed) | CPython ssl + uvicorn TLS | **≥ 2×** full; **≥ 5×** resumed (session resumption, §4.1) |
| `bench_ws_echo.py` | ws echo msgs/sec + p99 | uvicorn[websockets] / websockets lib | **≥ 3×** (Rust framing vs Python websockets lib) |
| `bench_serve_workers_scaling.py` | RPS vs `--workers` 1→N | gunicorn -w N uvicorn | **linear to cores**, beating gunicorn at every N |

Per the performance contract, **every** bench must beat the CPython baseline on **every**
target/profile (native macOS+Linux, release-fast/dev-fast/debug-with-asserts). The plaintext
bench is the hardest "beat granian" case (both are Rust-core); the JSON/latency/client/ws
benches are where molt's compiled-app + io_uring + zero-copy edge compounds.

---

## 10. Dependency edges — what proceeds NOW vs blocks on 26/28/33

| Phase | Blocks on docs 26/28/33? | Why |
|---|---|---|
| **P0** TLS sans-io + ALPN + resumption | **NO — proceeds now** | rustls config + state machine; no coroutine/loop dependency (the sans-io BIO path is driven by whatever loop, incl. the existing io-poller) |
| **P1** httparse + persistent client | **NO — proceeds now** | parser swap + socket lifecycle; can use the existing blocking path or io-poller |
| **P2** h2 (hreq-h2) | **NO** (needs P0 for ALPN) | protocol state machine over buffers; no coroutine dependency |
| **P3** ws server | **NO** (needs P1) | tungstenite accept; framing is sans-io |
| **P4** `molt.serve` ASGI runtime | **YES — doc-26 + doc-28** | the app is `async def` (doc-26 `_poll` frames must compile/resume); receive/send/Tasks/buffer-pool/io_uring are doc-28 |
| **P5** workers + MSGI + unleashed | **YES — doc-33** (workers); P4 (MSGI/unleashed) | SO_REUSEPORT multi-process is doc-33's spawn protocol |

**The actionable conclusion:** P0, P1, P2, P3 (TLS gap closure, httparse swap, h2, ws-server)
are **all independent of the in-flight async work and start immediately** — they harden the
parity surface and build the protocol layers. Only **P4 (the server keystone) and P5
(multi-core/unleashed)** wait on docs 26/28/33 implementation landing. This matches doc-29's
"HTTP/2 is independent and can proceed in parallel" while correcting its implication that the
*whole* subsystem is blocked: three of five phases are not.

---

## 11. Scores (house IMPORTANCE × GAP scale, 1–3 each)

| Piece | Importance | Gap | Product | Note |
|---|---|---|---|---|
| ASGI server runtime (P4) | 3 (web) | 3 (nonexistent) | **9** | the keystone; the `molt build app.py → server` thesis |
| async-TLS sans-io + ALPN (P0) | 3 | 3 (blocking-only, no ALPN) | **9** | blocks h2 + async server; parity gaps G2/G3/G4/G8 |
| h2 (P2) | 3 | 3 (absent) | **9** | modern web table-stakes; hreq-h2 decision |
| httparse swap + async client (P1) | 3 | 2 (works but hand-rolled/one-shot) | **6** | parity G5 + the structural parser fix |
| ws server (P3) | 2 | 3 (client-only) | **6** | the accept side is the gap |
| multi-core workers (P5) | 3 | 2 (SO_REUSEPORT primitive exists) | **6** | doc-33 dependency |
| MSGI + unleashed (P5) | 2 | 3 (new) | **6** | the throughput superpower |

**Subsystem weighted gap (web/engineering): the three 9s (server, async-TLS, h2) are the
critical mass.** P0+P1 are the no-dependency down-payment; P4 is the headline.

---

## 12. Explicit refusals (what this design does NOT do, and why)

- **No `hyper`, no `tokio` in the binary.** Refused: both violate the single-GIL-serialized-
  loop model (two executors) and the <2MB mandate (megabytes of runtime). hreq-h2 (protocol
  only) + httparse + the existing mio/io_uring loop is the structurally correct substitution.
  Taking hyper because "it's the obvious HTTP crate" is exactly the comfort-shortcut the
  top-priority policy rejects.
- **No claiming RSGI wire-compat.** Refused: RSGI is Granian's spec; molt's MSGI mirrors its
  *shape* (cited) but is backed by `MoltBuffer`, not Granian's types. Calling it "RSGI" would
  be a false-compat claim.
- **No fake HTTP/2 / fake h2spec pass.** Refused: P2's gate is real h2spec conformance. A
  partial frame parser labeled "HTTP/2" is the partial-implementation trap.
- **No WASM-browser server pretense.** Refused: browsers cannot accept TCP. The doc states the
  server is N/A there and offers the SW-edge pattern only as a *deferred, host-API-gated*
  exploration, not a Year-5 commitment.
- **No shipping P4 before docs 26/28 land.** Refused: an ASGI server whose app cannot suspend
  correctly (no real async generators / no asyncio runtime) is a demo, not the end state. P0–
  P3 deliver real value meanwhile; P4 waits for its foundation. (This is the "complete
  structural change is the unit of work" rule — P4 is gated on its prerequisites, not faked.)
- **No keeping the hand-rolled HTTP/1.x framing "because it works."** Refused: it is the
  manual-parsing-where-structural-parsing-belongs smell; httparse replaces it (a named
  deletion, not a parallel second parser).

---

## 13. One-screen summary for the implementer

1. **Now, no dependencies (P0):** give rustls a sans-io (MemoryBIO/`wrap_bio`) path so TLS
   runs on the async loop; add `alpn_protocols` (3 lines) + `selected_alpn_protocol` getter +
   client `Resumption`/server `ticketer` + server `with_client_cert_verifier`. Fix
   `ssl.py:121/126/138/143/303` and the MSG_* flags. Risk: handshake-across-edges state
   machine. — closes G1–G4, G8.
2. **Now, no dependencies (P1):** swap the 7338-line hand HTTP/1.x framing for `httparse`
   (collapse the dual copy first); make `http.client` persist its socket + add a keep-alive
   pool + pipelining; re-base `socketserver.serve_forever` off the 50ms poll. — closes G5, G7.
3. **After P0 (P2):** `hreq-h2` (Tokio-free h2) over `MoltBuffer`s; gate on h2spec.
4. **After P1 (P3):** tungstenite **server** accept (the client is done); gate on Autobahn
   fuzzingclient.
5. **After docs 26+28 + P0–P3 (P4 — keystone):** `molt.serve` — connection lifecycle manager,
   scope/receive/send over the buffer pool, backpressure, lifespan, graceful shutdown; the
   compiled app dispatched via `call_callable3`; ASGI default + MSGI unleashed. Gate: FastAPI/
   Starlette run unmodified.
6. **After doc-33 (P5):** SO_REUSEPORT spawn-isolate workers (multi-core); MSGI lazy scope;
   the U1–U5 unleashed trades. Gate: linear RPS scaling; beat granian on plaintext, ≥3×
   uvicorn on JSON, ≥3× on ws echo, ≥2×/≥5× on TLS full/resumed.

Crates that win: **httparse** (parser), **hreq-h2** (h2, Tokio-free), **rustls 0.23**
(TLS+ALPN+resumption, in tree), **tungstenite 0.29** (ws, in tree, server side to build),
**mio 1.2 + socket2 0.6 + SO_REUSEPORT** (sockets, in tree). No hyper, no tokio.
