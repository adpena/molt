# Molt Worker v0: IPC Execution Shell (stdio-first)
**Spec ID:** 0911
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** runtime engineers, AI coding agents
**Goal:** Define the minimal worker that can execute exported entrypoints safely and predictably.
**Implementation status:** Initial Rust stdio shell exists in `runtime/molt-worker` with framing, export allowlist, and a deterministic `list_items` demo entrypoint. Cancellation, compiled entrypoint dispatch, and metrics are pending.

---

## 0. Modes
### 0.1 Stdio mode (required v0)
- Worker reads framed messages from stdin, writes framed messages to stdout
- Enables easiest integration, testing, and portability

### 0.2 Unix socket mode (optional v0.1)
- Local domain socket: `/tmp/molt.sock`
- Better for production later, but not required for first win

---

## 1. Entrypoints
Worker loads a manifest at startup, e.g. `molt_exports.json`:
```json
{
  "abi_version": "0.1",
  "exports": [
    {"name": "list_items", "codec_in": "msgpack", "codec_out": "msgpack"}
  ]
}
```

Entrypoints are invoked by name with a payload.

---

## 2. IPC framing (minimal, robust)
- length-prefixed frames (u32 LE length + bytes)
- message encoding: MsgPack (or a tiny custom binary header + bytes)

### 2.1 Request fields (minimum)
- `request_id: u64`
- `entry: string`
- `timeout_ms: u32`
- `codec: enum` (msgpack/json/arrow_ipc reserved)
- `payload: bytes`

### 2.2 Response fields (minimum)
- `request_id: u64`
- `status: enum` (Ok, InvalidInput, Busy, Timeout, Cancelled, InternalError)
- `payload: bytes` (result if Ok)
- `error: string?` (present if not Ok)
- `metrics: map?` (optional v0)

---

## 3. Execution model
- Worker maintains:
  - bounded request queue
  - small fixed threadpool OR async runtime (implementation choice)
- Each request receives:
  - deadline
  - cancellation token (triggerable by client cancel frame or disconnect)

---

## 4. Timeouts and cancellation
- Worker enforces `timeout_ms` strictly
- On timeout/cancel:
  - return status
  - drop/rollback resources
- Do not leak memory, tasks, or DB connections

**Implementation note:** current worker accepts `__cancel__` requests carrying a `request_id` payload; cancellation is best-effort and only affects the built-in demo handler. Full propagation into compiled entrypoints/DB tasks is still pending.

---

## 5. Safety hardening
- Validate entry name exists
- Validate payload size limits
- Reject oversized frames early
- Never panic on malformed input (return InvalidInput)

---

## 6. Observability (minimal v0)
- Log per-request:
  - start/end timestamp
  - queue time
  - exec time
  - status
- Optional: emit a JSON line per request for easy parsing

---

## 7. Testing (must)
- unit tests for framing
- fuzz tests for decoding/invalid frames
- “kill worker mid-request” recovery test via `molt_accel` harness
