# FastAPI: How It Works and How It Compares
**Guide ID:** 0933
**Audience:** Python web developers evaluating architecture choices, Molt framework designers
**Goal:** Explain FastAPI’s design, how requests flow through it, and how it compares to popular alternatives (Django, Flask, Starlette, Sanic, aiohttp, Falcon).

---

## 0. What FastAPI is (the essence)
FastAPI is an **ASGI** web framework built on **Starlette** (web) and **Pydantic** (data validation/serialization).
Its “killer feature” is that it turns Python type hints and Pydantic models into:
- automatic request parsing and validation
- automatic JSON serialization
- automatic OpenAPI documentation
- async-first handler ergonomics

It became popular because it is:
- ergonomic like Flask
- async-friendly like modern frameworks
- documented like an enterprise system (docs “for free”)

---

## 1. How FastAPI works internally (conceptual request flow)
A simplified request lifecycle:

1) **ASGI server receives connection**
   - Typically `uvicorn` (or `hypercorn`)
   - The server speaks ASGI and calls your app with a `scope` + `receive` + `send`

2) **Starlette routing**
   - Matches path + method
   - Builds middleware chain

3) **Dependency injection / parameter extraction**
   - FastAPI inspects the handler signature:
     - path params
     - query params
     - headers/cookies
     - body model (Pydantic)
     - dependencies (`Depends`)
   - Builds a “dependency graph” per request

4) **Validation**
   - Pydantic validates request bodies into models
   - Validation errors become structured HTTP errors (often 422)

5) **Handler runs**
   - `async def` supported naturally
   - sync handlers run too (FastAPI can run them in a threadpool)

6) **Response serialization**
   - return dict/model → JSON
   - response_model can filter/shape output

7) **OpenAPI**
   - FastAPI aggregates route metadata + schema models to generate OpenAPI

This design is why FastAPI feels “magical”: your function signature becomes the API contract.

---

## 2. Why FastAPI can be fast (and where it isn’t)
FastAPI’s core web layer (Starlette) is efficient, but performance often depends on:
- validation costs (Pydantic model complexity)
- serialization costs
- dependency injection overhead (can be non-trivial)
- DB I/O and ORM costs (often dominant)

In real services, bottlenecks are frequently:
- database access patterns
- object allocation and JSON encoding
- ORM hydration

This is why “runtime-level” approaches (like Molt) focus on:
- boundary compilation
- typed decoding
- cancellation/backpressure
- reducing allocations

---

## 3. Comparison to other options
### 3.1 Django
**Strengths**
- batteries included: ORM, admin, auth, migrations, templates
- huge ecosystem
- stable and predictable

**Weaknesses**
- historically sync-first
- ORM + request stack can be heavier
- scaling patterns often involve separate worker systems

**When Django wins**
- monolithic apps, admin-heavy systems, rapid CRUD with strong conventions

**How Molt fits**
- keep Django as control plane; offload hot endpoints/jobs to Molt worker

---

### 3.2 Flask
**Strengths**
- tiny, simple mental model
- huge ecosystem
- great for small services

**Weaknesses**
- WSGI model (sync)
- you assemble your own “batteries”

**When Flask wins**
- small APIs, prototypes, internal tools

---

### 3.3 Starlette
**Strengths**
- lightweight ASGI toolkit
- fast routing/middleware
- basis for FastAPI

**Weaknesses**
- fewer batteries than FastAPI

**When Starlette wins**
- you want ASGI speed without FastAPI’s DI/validation machinery

---

### 3.4 Sanic / aiohttp
**Strengths**
- async-first
- mature ecosystems (aiohttp especially for clients)

**Weaknesses**
- less “automatic contract” than FastAPI
- docs/story not as turnkey

**When they win**
- high concurrency apps that don’t want FastAPI’s DI model

---

### 3.5 Falcon
**Strengths**
- performance-focused, minimal overhead
- explicit request/response handling

**Weaknesses**
- less ergonomic “type-driven” API modeling

**When it wins**
- very performance-sensitive APIs where you want minimal framework overhead

---

## 4. The key architectural fork: ASGI vs “compiled runtime”
FastAPI lives in the ASGI ecosystem:
- flexible, dynamic
- easy to extend
- runtime reflection is expected

Molt’s web direction (if/when needed) is different:
- compile-time known routing and schemas
- fewer dynamic hooks
- explicit contracts and tiers

This difference is why “FastAPI compatibility” is best approached as:
- preserving ergonomic patterns (schemas, decorators, great errors)
- not copying the exact internals (ASGI-first reflection and DI graphs)

---

## 5. Practical takeaway for Molt
### What to copy from FastAPI
- schema-driven boundary UX
- great validation errors
- OpenAPI generation
- async handler ergonomics
- easy-to-read routing decorators

### What to avoid copying
- complex runtime DI graphs as the core abstraction
- too much runtime reflection in hot paths
- implicit threadpool behavior

Molt should feel as easy as FastAPI, but execute like a compiled service.
