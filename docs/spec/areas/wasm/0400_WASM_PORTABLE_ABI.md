# Molt Portable WASM ABI (Browser ↔ Server Symmetry)
**Spec ID:** 0400
**Status:** Draft (implementation-targeting)
**Priority:** P0/P1 (P0 for core “shared logic”, P1 for broader ecosystem)
**Audience:** runtime engineers, compiler engineers, client/runtime integrators
**Goal:** Make Molt-compiled WASM modules run in **both**:
- the browser (WASM32-unknown-unknown)
- the server (native host, optionally WASI)

…and enable a Molt server to talk to Molt WASM running in the browser with consistent data formats, error semantics, and security constraints.

---

## 0. Why This Exists

“Molt → WASM” is not just a compilation target. It is a strategy to:
- ship **shared business logic** to the browser (fast UX, consistent rules)
- run **sandboxed plugins** on the server
- create a single ecosystem for “Molt Packages” across platforms
- enable a clean interoperability story without CPython C extensions

This spec defines the minimal stable ABI and host surface needed for:
- **browser ↔ server** parity
- safe capability boundaries
- predictable performance and determinism

---

## 1. Scope and Non-Goals

### 1.1 In scope (v0.1)
- Pure compute modules that operate on bytes/strings/structured data
- Deterministic execution by default
- Host-provided I/O via explicit capabilities (opt-in)
- Efficient data exchange for:
  - MsgPack/CBOR (preferred), JSON (compatibility/debug)
  - Arrow IPC for tabular data (where supported)

### 1.2 Non-goals (v0.1)
- Full POSIX/WASI parity in the browser
- Direct filesystem/network access from WASM without explicit capability
- “Import arbitrary Python package into browser and it works”
- JIT in the browser (disallowed by policy in many environments)

---

## 2. Target Triples and Build Modes

### 2.1 Browser target
- `wasm32-unknown-unknown`
- No WASI assumptions
- All I/O must be via host imports (JS glue)

### 2.2 Server targets
- Native: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, etc.
- Optional WASI for plugins: `wasm32-wasip1` (or future component model) (TODO(wasm-host, owner:runtime, milestone:RT3, priority:P3, status:planned): component model target support).

### 2.3 “Portable Module” definition
A module is “portable” if it:
- does not require syscalls
- uses only the Portable Host API defined here
- avoids nondeterminism unless explicitly allowed

---

## 3. Memory Model and Data Passing

### 3.1 Linear memory
Portable WASM modules use linear memory for all buffers.

### 3.2 Canonical buffer representation
All buffers are represented as:
- `ptr: i32` (offset into linear memory)
- `len: i32` (byte length)

For strings:
- UTF-8 by default
- length in bytes

### 3.3 Ownership and lifetimes
- The module allocates buffers in its own linear memory via `molt_alloc` and
  resolves the returned handle with `molt_handle_resolve` (pointer registry lookup)
  before use.
- The host never assumes buffer validity beyond the call boundary unless explicitly copied
- Results returned to the host must be either:
  - copied out by the host, or
  - referenced via a “loan” with explicit release

v0.1 recommendation: **host copies results** for simplicity and safety.

---

## 4. Error Model (Required for Cross-Platform Consistency)

### 4.1 Result convention
All exported functions must use a uniform result encoding:

- return an `i32` status code:
  - `0` = OK
  - non-zero = error code enum

- and write outputs into provided out-parameters in linear memory.

Example signature style:
- `fn foo(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> i32`

Where:
- `out_ptr_ptr` points to an i32 where the module writes the result buffer pointer
- `out_len_ptr` points to an i32 where the module writes the result buffer length

### 4.2 Error payloads
On error, the module may write an error message buffer:
- `err_ptr_ptr`, `err_len_ptr`
- message is UTF-8
- host may log or surface it

### 4.3 Error code enum (v0.1)
Recommended minimal set:
- `1 = InvalidInput`
- `2 = DecodeError`
- `3 = EncodeError`
- `4 = Cancelled`
- `5 = Timeout`
- `6 = CapabilityDenied`
- `7 = InternalError`

---

## 5. Portable Host API (Imports)

Portable modules must not call syscalls directly. Instead they import a tiny host surface.
This surface is implemented by:
- JS host in the browser
- Rust host in the server

### 5.1 Required imports
- `molt_alloc(size: i64) -> i64`
- `molt_free(ptr: i32, len: i32) -> void`

### 5.2 Optional imports (capability-gated)
- `molt_log(level: i32, msg_ptr: i32, msg_len: i32) -> void`
- `molt_now_ms() -> i64` (only if nondeterminism allowed)
- `molt_random_bytes(out_ptr: i32, out_len: i32) -> i32` (capability required)

### 5.3 I/O imports (explicit capability)
Browser I/O is naturally host-mediated (fetch/websocket). Server I/O must also be capability-gated.

Define a capability table:
- `cap_id: i32` identifies a granted capability instance
- modules cannot obtain new capabilities, only use those passed by host

Example:
- `molt_fetch(cap_id, req_ptr, req_len, resp_ptr_ptr, resp_len_ptr) -> i32`

**v0.1 recommendation:** keep I/O out of portable modules; focus on shared logic and compute.

### 5.4 Socket host interface (capability-gated)
WASM socket intrinsics mirror POSIX-style sockets and feed the runtime io_poller:
- `molt_socket_*` imports (see `wit/molt-runtime.wit` for full list)
- `molt_socket_poll_host(handle, events) -> i32` returns a bitmask of
  `IO_EVENT_READ|IO_EVENT_WRITE|IO_EVENT_ERROR`
- `molt_socket_wait_host(handle, events, timeout_ms) -> i32` blocks until readiness or timeout

Capability policy:
- `net.connect` (or `net`) required for outbound connects
- `net.listen`/`net.bind` (or `net`) required for bind/listen/accept

Browser mapping:
- `AF_INET`/`AF_INET6` + `SOCK_STREAM` map to WebSocket-backed streams
- `SOCK_DGRAM`, `listen`, `accept`, and server sockets return `EOPNOTSUPP`
- Readiness: `READ` when the receive queue has data or the socket is closed/error;
  `WRITE` when open/closed/error; `ERROR` when the socket is in error state

Server mapping:
- Node/WASI and wasmtime hosts use native sockets with io_poller-backed readiness.

### 5.5 DB host interface (capability-gated)
For wasm builds that need database access, the host must expose:
- `db_query(ptr, len, out, cancel_token) -> i32`
- `db_exec(ptr, len, out, cancel_token) -> i32`
- `db_host_poll() -> i32` (non-blocking pump for pending DB responses/cancellation)

These accept MsgPack-encoded requests (see `docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`)
and return a stream handle for response bytes. Access is gated by `db.read` and
`db.write` capabilities. `db_host_poll` lets the runtime explicitly ask the host
to deliver any queued responses while the wasm scheduler is running.

---

## 6. Exported Function Conventions

### 6.1 Export metadata
Portable modules must ship a manifest alongside the `.wasm`:
- `molt_wasm_exports.json`

Example:
```json
{
  "abi_version": "0.1",
  "module_name": "shared_rules",
  "exports": [
    {"name": "validate_order", "input": "msgpack", "output": "msgpack", "deterministic": true},
    {"name": "price_cart", "input": "cbor", "output": "cbor", "deterministic": true}
  ]
}
```

### 6.2 Encoding recommendations
- For small structured payloads: MsgPack/CBOR (fast, compact, typed)
- For interoperability/debug: JSON
- For tabular payloads: Arrow IPC (see §7)

---

## 7. Tabular Data (Arrow IPC) Strategy

### 7.1 Why Arrow
Arrow gives:
- columnar memory model
- zero/low-copy in many environments
- a natural bridge between server analytics and browser visualization

### 7.2 Browser reality
Browser support is improving, but:
- memory is limited
- large allocations can be slow
- JS/WASM boundary copying can dominate

Therefore, v0.1 policy:
- support Arrow IPC **primarily for server ↔ server**
- support Arrow IPC in browser for **moderate-size** tables and preview transforms
- use chunking/streaming to avoid giant monolithic buffers

### 7.3 Arrow IPC in the ABI
Arrow payloads are passed as bytes:
- input: Arrow IPC stream/file bytes
- output: Arrow IPC bytes

Optional: add a streaming interface later (component model / incremental frames).

---

## 8. Security and Trust Model

### 8.1 The browser is untrusted
Even if both ends run Molt:
- clients can be modified
- results can be forged
- the server must validate

Portable WASM should be used to:
- improve UX and reduce duplication
- not to move trust boundaries

### 8.2 Capability-based sandboxing
Modules only receive explicit capabilities:
- no ambient authority
- no filesystem/network unless granted
- deterministic mode by default

### 8.3 Resource limits
Hosts must enforce:
- max memory pages
- max execution time (timeouts)
- max output size
- cancellation handling

---

## 9. Integration Patterns

### 9.1 Shared logic (recommended)
Compile shared modules twice:
- WASM for browser
- native for server

Keep module “portable” (no I/O) so it works identically.

### 9.2 Server plugin model
Server runs WASM plugins to sandbox:
- tenant-specific rules
- user scripts
- untrusted transforms

### 9.3 Browser ↔ server protocol alignment
Even if transport is HTTP/WebSocket, standardize:
- message encodings (MsgPack/JSON)
- schema IDs / versioning
- error code mapping

---

## 10. Testing and Validation

### 10.1 Cross-target equivalence tests
For each portable export:
- run the same input on:
  - native build
  - wasm build (wasmtime)
  - wasm build (browser harness)
- verify identical outputs/errors

### 10.2 Determinism tests
If `deterministic=true`:
- repeated runs must match bit-for-bit outputs

### 10.3 Fuzz tests
- fuzz decode/encode boundaries
- fuzz invalid inputs (must not crash)
- property tests for invariants

---

## 11. Acceptance Criteria (v0.1)

A v0.1 Portable ABI is successful when:
1) A Molt module can be compiled to WASM and run in browser and server hosts.
2) Exports follow the ABI consistently and are versioned.
3) Shared business logic modules behave identically across targets.
4) Capability boundaries are enforced and documented.
5) Integration examples exist:
   - browser module validating/formatting payloads
   - server module enforcing same rules

---

## 12. Roadmap (WASM evolution)
- v0.1: single-shot calls, buffer in/out, deterministic-by-default
- v0.2: streaming frames (for Arrow IPC chunking)
- v0.3: component model/WIT integration (see §13 migration plan)
- v1.0: stable ABI with backwards compatibility guarantees

---

## 13. Component Model Migration Plan

### 13.1 Current State (Phase 1 — Complete)

Molt's WASM target currently uses **raw wasmtime imports** via `Linker::func_wrap()`
calls in `molt-wasm-host`. The intrinsic interface is defined in WIT format at
`wit/molt-runtime.wit` (622+ intrinsic functions), but this WIT file serves as
documentation — the actual binding is manual.

**Current wasmtime version:** 41.0.3

### 13.2 Phase 2: Component Model Migration (Planned)

**Target:** wasmtime 42+ (when Component Model API stabilizes)

Migrate from raw `Linker::func_wrap()` imports to Component Model linking:

1. **Split WIT into capability-scoped interfaces:**
   - `molt:runtime/core` — lifecycle, object model, arithmetic, type system
   - `molt:runtime/io` — file, socket, process, stream operations
   - `molt:runtime/codec` — JSON, MsgPack, CBOR, Arrow IPC serialization
   - `molt:runtime/db` — database query/exec operations
   - `molt:runtime/async` — futures, promises, tasks, channels
   - `molt:runtime/collections` — list, dict, set, tuple, heapq

2. **Define a Component Model world:**
   ```wit
   world molt-app {
       import molt:runtime/core;
       import molt:runtime/collections;
       // Capability-gated imports (only linked if declared):
       import molt:runtime/io;
       import molt:runtime/codec;
       import molt:runtime/db;
       import molt:runtime/async;
       export run: func() -> s32;
   }
   ```

3. **Benefits:**
   - Capability enforcement at link time (not just runtime)
   - Stable ABI versioning via WIT semantic versioning
   - Interop with other Component Model toolchains (e.g., `wasm-tools compose`)
   - Automatic binding generation via `wit-bindgen`

4. **Migration steps:**
   - [ ] Split `wit/molt-runtime.wit` into per-capability `.wit` files
   - [ ] Define `molt-app` world in `wit/world.wit`
   - [ ] Replace `Linker::func_wrap()` calls with `wasmtime::component::Linker`
   - [ ] Update `molt-backend/src/wasm.rs` to emit Component Model modules
   - [ ] Validate with `wasm-tools validate --features component-model`

### 13.3 Phase 3: WASI Preview 2 Alignment (Future)

**Target:** After Component Model migration is stable

Replace custom I/O intrinsics with WASI Preview 2 (WASI 0.2) interfaces where
semantics align:

| Current Molt Intrinsic | WASI P2 Replacement |
|------------------------|---------------------|
| `molt_file_read` / `molt_file_write` | `wasi:filesystem/types` |
| `molt_socket_connect` / `molt_socket_send` | `wasi:sockets/tcp` |
| `molt_time_now` | `wasi:clocks/wall-clock` |
| `molt_random_bytes` | `wasi:random/random` |
| `molt_env_get` | `wasi:cli/environment` |
| `molt_process_spawn` | `wasi:cli/run` (limited) |

**Molt-specific intrinsics** (no WASI equivalent):
- Object model operations (NaN-boxing, refcount, GC)
- Collection operations (list, dict, set internals)
- Codec operations (MsgPack, CBOR, Arrow)
- Database operations
- Async runtime primitives

These will remain as custom `molt:runtime/*` imports alongside WASI interfaces.

### 13.4 Prerequisites

- [ ] wasmtime Component Model API stable (42+)
- [ ] WASI Preview 2 filesystem/network support mature
- [ ] Molt's WIT interface stabilized (no breaking changes in 3+ months)
- [ ] Performance validation: Component Model overhead < 2% vs raw imports

### 13.5 References

- [Bytecode Alliance Component Model docs](https://component-model.bytecodealliance.org/)
- [WASI Preview 2 specification](https://github.com/WebAssembly/WASI)
- [wasmtime Component Model guide](https://docs.wasmtime.dev/api/wasmtime/component/)
