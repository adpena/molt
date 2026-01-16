# `molt_accel` v0: Django Offload Client + Decorator
**Spec ID:** 0912
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** Python integrators, AI coding agents
**Goal:** Provide a minimal, reliable client library that makes offloading one endpoint trivial.
**Implementation status:** Initial stdio client + decorator scaffolding exists in `src/molt_accel` (framing + JSON/MsgPack payloads) with concurrent in-flight support in the shared client plus optional worker pooling via `MOLT_ACCEL_POOL_SIZE`. Timeouts send a best-effort cancel and mark the worker for restart after in-flight requests drain; metrics hooks/cancel checks are wired, but Django test-client coverage and richer retry policy remain pending (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): Django test-client coverage + retry policy). `molt_accel` ships as an optional dependency group (`pip install .[accel]`) with a packaged default exports manifest so the decorator can fall back to `molt-worker` in PATH when `MOLT_WORKER_CMD` is unset. A demo app scaffold lives in `demo/`.

---

## 0. Core responsibilities
- Start/attach to worker process
- Encode request, send over IPC, await response
- Enforce client-side timeout (in addition to worker)
- Propagate cancellation (client disconnect → cancel request)
- Handle worker restarts cleanly

---

## 1. Public API (minimum)
### 1.1 Low-level client
```python
client = MoltClient(worker_cmd=["./molt_worker", "--stdio", "--exports", "molt_exports.json"])
result = client.call("list_items", payload_obj, timeout_ms=250)
```
For higher concurrency, use `MoltClientPool` to round-robin across multiple worker processes.

### 1.2 Django decorator
```python
from molt_accel import molt_offload

@molt_offload(entry="list_items", codec="msgpack", timeout_ms=250)
def items_view(request):
    ...
```

See `docs/demo/django_offload_example.py` for a minimal example.

Decorator semantics:
- prepares payload from request (defined by demo contract)
- calls worker
- returns Django JsonResponse
- uses `MOLT_WORKER_CMD` when set; otherwise falls back to `molt-worker` in PATH plus the packaged default exports manifest

---

## 2. Failure behavior (must be polished)
- Worker unavailable → return 503 with a clear error code
- Worker Busy → return 429 or 503 (configurable)
- Timeout → 504
- InvalidInput → 400
- InternalError → 500 but with safe message

No stack traces leaked by default.

---

## 3. Cancellation
- If Django request is cancelled/aborted:
  - cancel in-flight worker request
  - do not continue work
  - release resources promptly
**Implementation note:** when `cancel_check` is not supplied, the decorator
auto-detects request helpers like `is_aborted()` or `is_disconnected()` and
polls them when available.

---

## 4. Retries and restart policy
- If worker dies:
- restart worker once (configurable)
- re-send request only if idempotent flag is set (default false)

**Implementation note:** `MoltClient.call(..., idempotent=True)` will retry once after restarting the worker. The decorator exposes `idempotent=` to opt in.

---

## 7. Decorator options and behaviors
- `entry`: the worker export name. Changing this routes the request to a different compiled handler. If the name is not present in the compiled manifest or built-in handlers, compilation fails (Static) or returns `InvalidInput`/`InternalError` at runtime (compiled path missing).
- `codec`: payload encoding for request/response (`msgpack` preferred; `json` as fallback). Must match the compiled export manifest (`codec_in`/`codec_out`).
- `timeout_ms`: client-side timeout; on timeout the client sends `__cancel__` and schedules a worker restart after outstanding requests drain.
- `client`: optional `MoltClient` instance; otherwise the decorator constructs one using `MOLT_WORKER_CMD` or `molt-worker` in PATH with packaged exports.
- `client_mode`: `shared` (default) reuses a single long-lived `MoltClient`; `per_request` spawns a client per request and closes it after the call. Defaults from `MOLT_ACCEL_CLIENT_MODE`.
- Pooling: set `MOLT_ACCEL_POOL_SIZE` to a value >1 to use a `MoltClientPool` when `client_mode=shared`.
- `payload_builder`: transforms the Django request into the payload sent to the worker. Set this to match your contract when using a different `entry`.
  Built-in helpers live in `molt_accel.contracts` (`build_list_items_payload`,
  `build_compute_payload`, `build_offload_table_payload`), including JSON-body
  parsing for the offload-table demo.
- `response_factory`: builds the HTTP response from the worker result (use `raw_json_response_factory` for JSON pass-through).
- `allow_fallback`: when True, failures call the original view instead of returning an error response.
- `decode_response`: when False, return raw payload bytes to the response factory (useful for JSON pass-through).
- Hooks: `before_send`, `after_recv`, `metrics_hook`, and `cancel_check(request)` provide observability and cancellation integration.
- `idempotent`: when True, the client will retry once after a worker restart.

## 5. Metrics hooks
Expose hooks:
- before_send / after_recv
- latency measurements
- optional Prometheus integration later (TODO(observability, owner:tooling, milestone:TL2, priority:P3, status:planned): Prometheus integration).
Metrics hooks include `client_ms` plus payload sizes (`payload_bytes`,
`response_bytes`) in addition to any worker-provided metrics.

---

## 6. Testing
- unit tests for client framing
- integration tests with a test worker stub
- Django test client tests for decorator behavior
