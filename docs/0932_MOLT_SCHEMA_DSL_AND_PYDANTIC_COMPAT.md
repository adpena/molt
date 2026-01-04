# Molt Schema DSL: Pydantic-Compatible, Compiler-Friendly, and Fast
**Spec ID:** 0932
**Status:** Draft (language/API design)
**Audience:** Molt language designers, runtime engineers, framework authors
**Goal:** Define a Molt-native schema DSL that:
- feels familiar to Pydantic/FastAPI users
- maps cleanly to Schema IR (SIR)
- enables compilation to fast codecs/validators/layouts
- supports versioning and compatibility rules

---

## 0. Why a Molt-native DSL (even if we support Pydantic)
Pydantic is a great UX, but:
- it contains features designed for dynamic Python flexibility
- not all features map cleanly to ahead-of-time compilation
- we need a stable, minimal core that Molt can guarantee long-term

So:
- accept Pydantic as an input format
- offer Molt Schema DSL as the “strict, stable core”
- allow auto-generation between them where possible

---

## 1. User-facing syntax (proposed)
### 1.1 `@model` (struct-like schema)
```python
from molt_schema import model, field

@model
class Item:
    id: int
    title: str
    score: float = 0.0
    unread: bool = False
```

### 1.2 Constraints (minimal core)
```python
@model
class Query:
    user_id: int
    q: str | None = None
    limit: int = field(default=50, ge=1, le=200)
```

### 1.3 Strictness
```python
@model(strict=True)
class Input:
    ...
```

---

## 2. Mapping to Schema IR (SIR)
SIR must encode:
- fields, types, defaults
- optionality / unions
- constraints (core set)
- strict vs lax coercion
- schema_id + schema_version

SIR is the only thing the compiler/runtime depends on.

---

## 3. Compatibility and evolution
Every schema has:
- `schema_id` (stable)
- `schema_version` (semver-like)

Rules:
- adding an optional field = backward compatible
- adding a required field = breaking
- changing type = breaking (unless explicit coercion policy allows)
- removing a field = breaking

Tooling:
- `molt schema diff old new`
- `molt schema bump --patch/--minor/--major`

---

## 4. Code generation targets
From SIR generate:
- JSON decoder/encoder
- MsgPack decoder/encoder
- validator code
- internal layout descriptors (struct offsets, nullability)

Optionally generate:
- OpenAPI schema
- JSON Schema

---

## 5. Pydantic compatibility plan
### 5.1 Import Pydantic models
- `molt schema import` reads Pydantic v2 models and emits SIR

### 5.2 Export Pydantic models (optional)
- generate Pydantic models for teams that want runtime validation outside Molt

### 5.3 Feature intersection
Define the supported subset explicitly:
- fields, defaults, nested models
- core constraints
- strict/lax modes
- no arbitrary dynamic validators in the core subset

Non-core features can exist behind explicit hooks.

---

## 6. Runtime guarantees
In strict tier:
- model shapes are closed
- values are stored in struct layout
- encoding/decoding is bounded and safe
- errors are deterministic

---

## 7. Testing
- golden tests for SIR emission
- diff tests for schema evolution rules
- fuzz tests for decoders
