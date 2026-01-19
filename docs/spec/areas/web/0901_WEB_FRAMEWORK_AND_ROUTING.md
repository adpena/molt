# Molt Web Framework (molt_web): Routing, Middleware, and “Django-Friendly” Ergonomics
**Spec ID:** 0901
**Status:** Draft (product + API)
**Priority:** P1 (after HTTP runtime + demos)
**Audience:** framework authors, backend engineers
**Goal:** Provide a minimal, fast, expressive web framework that showcases Molt’s strengths without recreating Django.

---

## 0. Philosophy
- **Do not clone Django.** Django already exists and will remain valuable.
- Provide a **modern, small, fast** framework like “FastAPI in spirit” but designed around Molt’s:
  - tasks/channels
  - cancellation
  - static compilation constraints
  - small binary deployment

The first wedge is “accelerate Django,” not “replace Django.”
But Molt still needs a native framework for greenfield services.

---

## 1. Core primitives
### 1.1 Router
- path parameters: `/users/{id}`
- method routing: GET/POST/etc
- grouped routes and nesting

### 1.2 Middleware
- chain of middlewares around handler
- explicit ordering
- cancellation-aware
- no magic globals

### 1.3 Request/Response
- request body as stream
- response body as stream
- typed helpers for JSON/MsgPack
- safe defaults for headers

---

## 2. Handler model
Handlers are `async` by default:
```python
from molt_web import App

app = App()

@app.get("/health")
async def health(req):
    return {"ok": True}
```

### 2.1 Dependency injection (careful)
DI can explode complexity. Keep minimal:
- explicit `ctx` object
- app-scoped state container
- request-scoped context
- `ctx.cancelled()` reflects the request token; spawned work inherits unless overridden

---

## 3. Validation and schemas (phase-in)
- optional schema validation for inputs/outputs
- prefer messagepack/arrow compatibility for internal services
- Pydantic-compat is nice but must not dominate design

---

## 4. Background tasks integration
First-class integration with the “no Celery” story:
- `ctx.spawn(...)` to run background tasks with bounded lifetime
- job registry for durable tasks (phase-in)

---

## 5. Database integration (Molt-native)
- official integration points with `molt_db`
- per-request session and transaction helpers
- strict timeouts by default

---

## 6. Compatibility and migration
### 6.1 Django coexistence
- provide reverse proxy / gateway patterns
- allow Django to route some paths to Molt services
- shared auth via JWT/session bridging (phase-in)

### 6.2 WSGI/ASGI
- do not target WSGI (wrong concurrency model)
- optional ASGI adapter if it reduces friction, but avoid designing around it

---

## 7. Testing and tooling
- built-in test client for handlers
- golden tests for routing
- fuzz tests for path params and decoding

---

## 8. Acceptance criteria
- can build a small service binary with:
  - router
  - middleware
  - JSON response
  - one DB query via `molt_db`
- can deploy as a single binary and handle high concurrency
