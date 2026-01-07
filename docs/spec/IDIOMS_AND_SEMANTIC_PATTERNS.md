# Molt Idioms & Semantic Patterns
**Status:** Canonical design document
**Audience:** Compiler authors, contributors, AI agents
**Goal:** Define which Python idioms Molt intentionally supports, how they are lowered, and where limits apply.

---

## 1. Philosophy

Molt does **not** aim to support Python by reimplementing CPython’s dynamic runtime.

Instead, Molt:
- recognizes **common Python idioms**
- treats them as **semantic patterns**
- lowers them into efficient native or WASM constructs
- rejects or de-optimizes patterns that defeat static reasoning

> Idioms are supported because they are common and optimizable, not because they are possible in Python.

---

## 2. Tiers of Support

All idioms are evaluated under a tiered model.

### Tier 0 — Fully Static / Trusted
- Types and shapes are known
- No dynamic dispatch
- Lowered directly to native code

### Tier 1 — Guarded / Deoptimizable
- Assumptions enforced via runtime guards
- Fast path when guards hold
- Deopt to Tier 2 on violation

### Tier 2 — Dynamic / Compatibility
- Python-like behavior
- Slower
- May be restricted in WASM or strict builds

---

## 3. Iterable Materialization Idioms

### 3.1 `list(range(...))`
**Status:** Fully supported (Tier 0 / Tier 1)

Recognized as a first-class idiom.

Lowering:
- compute length statically or via guard
- allocate contiguous buffer once
- fill via native loop
- no iterator objects created

---

### 3.2 `tuple(range(...))`
**Status:** Supported

Lowered similarly to `list(range(...))`, with immutable container semantics.

---

### 3.3 `list(map(f, range(...)))`
**Status:** Supported with restrictions

Lowering:
- inline map loop if `f` is analyzable
- otherwise Tier 1 guarded loop
- may deopt if `f` escapes or captures dynamic state

---

### 3.4 `list(<generator>)`
**Status:** Tier 2 only

General generator materialization is not optimized and may be restricted in WASM.

---

## 4. Reduction Idioms

### 4.1 `sum(range(...))`
**Status:** Supported

Lowering:
- arithmetic series formula when possible
- otherwise native loop

---

### 4.2 `any(...)`, `all(...)`
**Status:** Supported

Lowered to short-circuiting native loops.

---

### 4.3 `min(range(...))`, `max(range(...))`
**Status:** Supported

Lowered analytically where possible.

---

## 5. Comprehensions

### 5.1 List comprehensions over `range`
```python
[x * 2 for x in range(10)]
```

**Status:** Fully supported

Lowered to a single allocation and loop.

---

### 5.2 Nested comprehensions
**Status:** Supported with limits

Requires analyzable bounds; otherwise Tier 1 or Tier 2.

---

## 6. Dictionary & Set Idioms

### 6.1 Dict comprehensions
**Status:** Tier 1

Hash semantics preserved; allocation optimized when size is known.

---

### 6.2 Set comprehensions
**Status:** Tier 1

---

## 7. Indexing & Slicing Idioms

### 7.1 Slicing lists
```python
xs[1:10]
```

**Status:** Supported

Lowered via bounds-checked slicing.

---

### 7.2 Negative indices
**Status:** Supported

---

## 8. Control-Flow Idioms

### 8.1 `for _ in range(n):`
**Status:** Fully supported

Lowered to canonical counted loops.

---

### 8.2 `enumerate(range(...))`
**Status:** Supported

Lowered to dual-index loops.

---

### 8.3 `zip(range(...), range(...))`
**Status:** Supported

Lowered to bounded multi-index loops.

---

## 9. Numeric & Vectorization-Friendly Idioms

Arithmetic loops over arrays and ranges are preferred patterns and may be vectorized.

---

## 10. Idioms Explicitly Not Optimized

- `eval`, `exec`
- monkey-patching builtins
- reflection-heavy metaprogramming
- self-modifying globals

Such patterns may force Tier 2 or be rejected.

---

## 11. WASM-Specific Constraints

Iterator-heavy or reflection-based idioms may be disallowed in WASM builds.

---

## 12. Guidance for Contributors & AI Agents

- Prefer idioms in this document
- Rewrite unsupported idioms into supported patterns
- Propose new idioms with clear semantics and lowering strategy

---

## 13. Non-Goal

This document is not a promise of full Python compatibility. It is a contract for performance and analyzability.
