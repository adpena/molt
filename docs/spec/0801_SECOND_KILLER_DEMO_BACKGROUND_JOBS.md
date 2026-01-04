# Second Killer Demo: Background Jobs Without Celery
**Spec ID:** 0801
**Status:** Strategic Demo

---

## Why this demo matters
Celery exists because Python lacks a strong concurrency runtime.

Molt removes that need.

---

## Demo concept
Django enqueues jobs directly to a Molt worker.
No broker.
No Celery.

---

## What this proves
- Structured concurrency works for background jobs
- Cancellation and retries are correct
- Operational simplicity beats distributed queues

---

## Outcome
Molt becomes a service + job runtime, not just an accelerator.
