# Transactions, Cancellation, and Request Scopes
**Spec ID:** 0704
**Status:** Draft
**Priority:** P0
**Audience:** runtime engineers, DB engineers
**Goal:** Define correct transaction semantics that work under Molt structured concurrency and cancellation.

---

## 0. Requirements
- Transactions must be scope-managed.
- Cancellation must not leave connections in bad states.
- Timeouts must be enforceable.
- Rollback must be reliable.

---

## 1. API shape
```python
from molt_db import pool

async def handler():
    async with pool.transaction() as tx:
        await tx.execute("INSERT ...", params)
        row = await tx.fetch_one("SELECT ...", params)
        return row
```

---

## 2. Cancellation behavior
If a task is cancelled:
- any in-flight query must be cancelled
- transaction scope must:
  - attempt rollback
  - if rollback fails, discard the connection from pool

**Token model:** cancellation flows through request-scoped tokens by default.
Handlers may override the current token for sub-operations (task-scoped
override) and must check `molt.cancelled()` (or the explicit token) at safe
points to abort promptly.

---

## 3. Nested transactions
Support savepoints (phase-in):
- `BEGIN`
- `SAVEPOINT`
- `RELEASE SAVEPOINT`
- `ROLLBACK TO SAVEPOINT`

---

## 4. Isolation levels
Expose basic isolation levels:
- read committed (default)
- repeatable read
- serializable

---

## 5. Observability
- transaction duration
- rollback counts
- abandoned connections

---

## 6. Testing
- integration tests for rollback under cancellation
- concurrent transactions under load
- nested transaction correctness
