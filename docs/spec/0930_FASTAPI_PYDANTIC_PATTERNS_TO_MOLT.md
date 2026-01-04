# FastAPI + Pydantic Patterns → Molt Equivalents
**Spec ID:** 0930
**Status:** Guidance (migration + design)
**Audience:** FastAPI users, Molt framework/runtime implementers, AI coding agents
**Goal:** Provide a concrete mapping from common FastAPI + Pydantic v2 patterns to Molt-native equivalents so users can migrate incrementally and Molt can design the right primitives.

---

## 0. Principle: Molt should copy *the winning ergonomics*, not the whole framework
FastAPI’s adoption comes from:
- minimal ceremony (decorators)
- automatic request parsing + validation
- great error messages
- excellent docs via OpenAPI generation
- async-first handler model

Molt should preserve these benefits, but compile the boundary work and enforce explicit contracts.

---

## 1. “Define request/response models” → “Compile boundary contracts”
### FastAPI
```python
from pydantic import BaseModel
from fastapi import FastAPI

app = FastAPI()

class In(BaseModel):
    user_id: int
    limit: int = 50

class Out(BaseModel):
    items: list[dict]

@app.get("/items", response_model=Out)
async def items(inp: In):
    ...
```

### Molt target shape
```python
from molt_web import App
from molt_schema import model

app = App()

@model
class In:
    user_id: int
    limit: int = 50

@model
class Out:
    items: list[Item]

@app.get("/items")
async def items(ctx, inp: In) -> Out:
    ...
```

**Molt behavior**
- `In`/`Out` are compiled into:
  - fast decoders/encoders (JSON/MsgPack)
  - validators
  - internal struct layouts (strict tier)
- handler receives a typed object (not a dict)

---

## 2. Dependency injection (DI) → explicit context + app state
### FastAPI pattern
```python
from fastapi import Depends

def get_db():
    ...

@app.get("/x")
async def x(db=Depends(get_db)):
    ...
```

### Molt mapping (recommended)
- Keep DI minimal; prefer:
  - `ctx.state` (app-scoped singletons)
  - `ctx.request` (request-scoped)
  - `ctx.with_db()` helper for DB sessions/transactions

```python
@app.get("/x")
async def x(ctx):
    async with ctx.db.transaction() as tx:
        ...
```

**Why:** DI can explode complexity, hurts compile-time reasoning, and encourages hidden global state.

---

## 3. Background tasks → structured concurrency
### FastAPI
FastAPI has “BackgroundTasks” but it’s not a full job system.

### Molt mapping
- Provide `ctx.spawn()` for bounded lifetime tasks
- Provide job registry for durable jobs (ties to “no Celery” demo)

```python
task = ctx.spawn(send_email(user_id), timeout_ms=5000)
```

**Rule:** tasks inherit cancellation and deadlines unless explicitly detached.

---

## 4. Validation errors → structured, stable error model
### FastAPI
Returns 422 with a structured error body for validation failures.

### Molt mapping
- Same spirit, stricter contract:
  - 400/422 for validation
  - stable error codes
  - field path + reason
  - never leak stack traces by default

---

## 5. OpenAPI generation → schema IR export
### FastAPI
Uses model metadata to emit OpenAPI.

### Molt mapping
- SIR (Schema IR) should support OpenAPI export as a tool:
  - `molt schema export openapi`

This is a major adoption feature; people love “docs for free.”

---

## 6. Routing and middleware → keep it small, compile-friendly
### FastAPI/Starlette
Starlette provides routing and middleware chain.

### Molt mapping
- Router supports:
  - static paths + path params
  - method dispatch
  - grouped routers
- Middleware supports:
  - request/response wrapping
  - timing, auth, tracing hooks
- Avoid dynamic middleware injection at runtime in strict tier.

---

## 7. Streaming responses (SSE / WebSockets) → first-class, cancellation-aware streams
FastAPI can stream, but cancellation + backpressure are not always “obvious.”

Molt should:
- expose streaming bodies as async iterators
- enforce bounded buffering
- cancel cleanly on disconnect

---

## 8. Migration strategy for existing FastAPI apps
### Phase A: Keep FastAPI, move hotspots to Molt worker
- Offload endpoints/job handlers through `molt_accel`
- Use Pydantic models as the contract authoring format (0921)

### Phase B: Introduce Molt-native services for new components
- Greenfield services use `molt_web` + `molt_schema`
- Keep interop with existing services via HTTP/MsgPack/Arrow

### Phase C: Optional adapter layer
- Allow Molt handlers to be mounted behind an ASGI adapter (only if needed)
- Do **not** design around ASGI as the core runtime model.

---

## 9. Checklist: “FastAPI parity” that matters (and what doesn’t)
### Matters
- painless schema-driven validation
- great error messages
- async handlers
- OpenAPI export
- performance + tail latency

### Doesn’t matter (early)
- every Starlette extension
- every DI trick
- plugin ecosystems

Molt should win by being small, fast, and predictable.
