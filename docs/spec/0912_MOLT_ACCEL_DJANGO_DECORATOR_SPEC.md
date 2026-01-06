# `molt_accel` v0: Django Offload Client + Decorator
**Spec ID:** 0912
**Status:** Draft (implementation-targeting)
**Priority:** P0
**Audience:** Python integrators, AI coding agents
**Goal:** Provide a minimal, reliable client library that makes offloading one endpoint trivial.
**Implementation status:** Initial stdio client + decorator scaffolding exists in `src/molt_accel` (framing + JSON/MsgPack payloads). Timeouts send a best-effort cancel and restart the worker; retries/metrics and Django test-client coverage are still pending.

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

---

## 4. Retries and restart policy
- If worker dies:
  - restart worker once (configurable)
  - re-send request only if idempotent flag is set (default false)

---

## 5. Metrics hooks
Expose hooks:
- before_send / after_recv
- latency measurements
- optional Prometheus integration later

---

## 6. Testing
- unit tests for client framing
- integration tests with a test worker stub
- Django test client tests for decorator behavior
