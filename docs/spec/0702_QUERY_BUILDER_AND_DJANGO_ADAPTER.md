# Query Builder + Django Adapter Strategy (Compatibility Without a Rewrite)
**Spec ID:** 0702
**Status:** Draft
**Priority:** P0 (migration wedge), P1 (ergonomics)
**Audience:** API designers, Django integrators, AI coding agents
**Goal:** Provide a Django-friendly path to async DB access and fast decoding without rewriting Django ORM internals.

---

## 0. Big idea
We do not replace Django ORM. We provide:
- a **query builder** for Molt-native code
- a **Django adapter** that can offload DB-intensive operations to Molt via IPC

---

## 1. `molt_sql` Query Builder (Molt-native)
### 1.1 API shape (target ergonomics)
```python
from molt_sql import table

User = table("users")

q = (User
     .where(User.email.endswith("@austin.edu"))
     .select(User.id, User.email)
     .limit(100))

rows = await q.fetch_all()
```

### 1.2 Scope
DF0:
- select/where/limit/order
- simple joins
- groupby + aggregates
- parameterized queries only

DF1:
- window functions (via DuckDB or SQL pushdown)
- richer expressions

---

## 2. Django adapter (Phase 1: IPC-based)
### 2.1 Design
- `molt_db_adapter` installs:
  - a DB router or helper API
  - a decorator/middleware for offloading
- Uses the `molt_worker` IPC protocol:
  - Django sends query requests (structured)
  - Molt executes via async Postgres client
  - returns results as:
    - Arrow IPC (preferred for bulk)
    - MsgPack (for small result sets)
- IPC payloads must follow `docs/spec/0915_MOLT_DB_IPC_CONTRACT.md` so the same
  builder can be reused by Flask/FastAPI adapters without Django-specific logic.

### 2.2 What we offload first
- reporting endpoints
- list views with heavy filtering
- batch jobs and exports
- expensive joins/aggregations
 - Phase 0: SQLite-backed list endpoint for the demo (real DB path before Postgres)

### 2.3 What stays in Django
- admin
- auth/session middleware
- forms
- templates

---

## 3. Django “future” integration options
Later, if justified:
- provide an async ORM-like facade that matches common QuerySet patterns
- but still compile/execute queries in Molt-native layer

The key is compatibility-by-adapter, not reimplementation of internals.

---

## 4. Testing
- adapter integration tests with a sample Django project
- correctness tests vs Django ORM results for supported query shapes
- performance tests: measure wins and detect regressions
