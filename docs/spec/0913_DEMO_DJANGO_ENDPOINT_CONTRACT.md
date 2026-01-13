# Demo Contract: DB-Heavy List Endpoint (Baseline vs Offload)
**Spec ID:** 0913
**Status:** Draft (demo contract)
**Priority:** P0
**Audience:** demo implementers, benchmark authors
**Goal:** Define a stable request/response contract so baseline and offload endpoints are equivalent.

---

## 0. Endpoint
- Demo endpoints: `GET /baseline/` and `GET /offload/` (same contract)
- Reserved contract alias: `GET /api/items` (future adapter/DB path)
- Query params:
  - `user_id` (int)
  - `q` (string, optional)
  - `status` (string, optional)
  - `limit` (int, default 50)
  - `cursor` (string, optional)

---

## 1. Response shape
```json
{
  "items": [
    {"id": 1, "created_at": "...", "status": "open", "title": "...", "score": 0.93, "unread": true}
  ],
  "next_cursor": "opaque",
  "counts": {"open": 12, "closed": 98}
}
```

---

## 2. Fake DB simulation mode (recommended for v0)
To keep benchmark stable and isolate runtime wins:
- add a “fake DB” path that simulates:
  - fixed or distribution-based latency
  - decoding cost proportional to result size
  - optional “join” cost

This allows:
- consistent results
- earlier validation of cancellation/backpressure

Then later swap in real Postgres with the same contract.

**Implementation status:** `molt_worker` returns a deterministic fake response for `list_items` and supports `MOLT_FAKE_DB_DELAY_MS` (base latency), `MOLT_FAKE_DB_DECODE_US_PER_ROW` (per-row decode cost), and `MOLT_FAKE_DB_CPU_ITERS` (per-row CPU work).

---

## 3. Worker entrypoint contract
- entry: `list_items`
- input payload (MsgPack):
  - query params normalized to a struct
- output payload (MsgPack):
  - response struct exactly matching JSON above

---

## 4. Equivalence requirements
- baseline and offload must produce identical outputs for the same input
- ordering and cursor semantics must match
- errors must map to consistent HTTP statuses
