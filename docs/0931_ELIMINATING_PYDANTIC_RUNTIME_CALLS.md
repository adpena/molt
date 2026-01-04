# Eliminating Pydantic Runtime Calls in Hot Paths (While Keeping the UX)
**Spec ID:** 0931
**Status:** Guidance (compiler/runtime optimization plan)
**Audience:** Molt compiler/runtime engineers, web engineers, AI coding agents
**Goal:** Show where Pydantic v2 runtime can be replaced with Molt-compiled boundary machinery, and define a safe adoption path that preserves developer ergonomics.

---

## 0. The key idea
Use Pydantic (or similar) as an **authoring format** for schemas, but compile those schemas into:
- fast decoders/encoders
- fast validators
- struct-like internal representations

So production execution does not pay:
- Python object graph building
- repeated validation passes
- dynamic dispatch inside validators (when avoidable)

---

## 1. Where Pydantic runtime is typically used
### 1.1 Request validation
- JSON → dict → Pydantic model

### 1.2 Response serialization
- Pydantic model → dict/json

### 1.3 Internal normalization layers
- converting DB rows/events into models repeatedly

These are the “edge costs” Molt can eliminate.

---

## 2. The “compiled boundary” replacement
### 2.1 Build-time
- Extract Schema IR (SIR) from models
- Generate codecs for chosen wire formats:
  - JSON (HTTP)
  - MsgPack (IPC)
  - optional Arrow (tabular)

### 2.2 Run-time
- Decode directly into internal struct layout
- Validate using generated rules
- Hand the typed object to the handler
- Encode output with generated encoder

**No Pydantic calls on the request path** (in strict mode).

---

## 3. Safety and correctness tiers
### 3.1 Reference mode (easy, slow, safe)
- Use Pydantic runtime as the oracle
- Use it to verify compiled codecs and validators
- Great for early development and differential testing

### 3.2 Production mode (fast)
- Use compiled boundary code
- Optional “shadow validation” sampling:
  - validate 0.1% of requests with Pydantic as a correctness canary
  - log mismatches and fallback gracefully

### 3.3 Strict tier (fastest)
- compiled boundaries are mandatory
- lying types are rejected at compile time or fail fast at boundary
- internal code can trust shapes strongly (pairs with 0922)

---

## 4. What can be compiled vs what must remain dynamic
### 4.1 Compilable (P0)
- field presence/optionality
- scalar type checks
- nested models
- default values
- simple constraints (min/max, regex length, etc.) where stable

### 4.2 Harder (P1/P2)
- complex custom validators with arbitrary code
- context-dependent validators
- validators with DB lookups or external I/O

**Rule:** custom validators must be explicit hooks, not magic.

---

## 5. Eliminating repeated internal normalization
A common anti-pattern:
- DB row → model → dict → model again

Molt should offer:
- DB row decoding directly into schema layout (0703 + 0921 alignment)
- avoid re-validation inside the core once boundary is validated

---

## 6. Differential testing plan (must)
- For each schema:
  - generate random payloads
  - compare compiled validator outcome vs Pydantic runtime
- Keep a “compatibility delta ledger”:
  - list differences explicitly
  - decide whether to match or intentionally diverge

---

## 7. Developer experience requirements
- Errors must remain as good as Pydantic’s:
  - field path, expected vs received, reason
- Docs generation (OpenAPI) must continue to work
- Debugging mode must exist:
  - dump decoded struct
  - dump validation trace

---

## 8. Exit criteria
We can claim “Pydantic-free hot path” when:
- 99.9% of requests use compiled boundaries
- mismatch rate in shadow validation is ~0 in real traffic
- error format remains stable and helpful
