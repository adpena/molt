# Idiom Rewrites for Agents
**Status:** Canonical (agent-facing)
**Purpose:** Provide mechanical rewrites that transform “valid Python” into “Molt-optimizable Python”.
**Audience:** AI coding agents and human contributors doing refactors.

---

## 0. What this document means by “rewrite”
A **rewrite** is a safe source-to-source transformation that:
- preserves behavior *within Molt’s supported subset*
- moves code from Tier 2 patterns to Tier 0/1 patterns
- improves analyzability, predictability, and performance

Agents should treat rewrites as:
- a refactoring rulebook
- a compatibility bridge
- an optimization on-ramp

This is *not* about style. It is about semantics and performance.

---

## 1. Rewrite protocol (agent procedure)

When you see an unsupported or Tier 2 idiom:
1. Identify the pattern
2. Choose a rewrite from this doc
3. Apply the smallest change that enables Tier 0/1 lowering
4. Add a test (or extend an existing one)
5. Emit a progress report with:
   - before/after snippet
   - why it improves Molt tiering
   - how to resume

---

## 2. High-impact rewrites (do these often)

### 2.1 Prefer `for i in range(n)` over iterator-heavy loops
**Before (Tier 2 risk):**
```python
for i in iter(range(n)):
    ...
```

**After (Tier 0/1):**
```python
for i in range(n):
    ...
```

---

### 2.2 Replace `list(generator)` with list comprehension over range or explicit loop
**Before (Tier 2):**
```python
xs = list(f(i) for i in range(n))
```

**After (Tier 0/1):**
```python
xs = [f(i) for i in range(n)]
```

If `f` is known/analyzable, this becomes a single allocation + loop.

---

### 2.3 Replace `map`/`filter` with comprehensions when targeting strict/WASM
**Before:**
```python
xs = list(map(f, range(n)))
ys = list(filter(p, xs))
```

**After:**
```python
xs = [f(i) for i in range(n)]
ys = [x for x in xs if p(x)]
```

---

### 2.4 Replace `sum([...])` with `sum(range(...))` or loop when appropriate
**Before:**
```python
total = sum([i for i in range(n)])
```

**After:**
```python
total = sum(range(n))
```

Or explicit loop if needed for mixed types.

---

### 2.5 Avoid dynamic attribute access in hot paths
**Before (dynamic):**
```python
val = getattr(obj, name)
```

**After (static):**
```python
val = obj.some_known_attr
```

If `name` must vary, isolate to Tier 2 boundary.

---

## 3. Boundary rewrites (move dynamism to the edges)

### 3.1 “Dynamic at the edges, static in the core”
**Before (dynamism in the middle):**
```python
def handler(req):
    fn = globals()[req["op"]]
    return fn(req)
```

**After (boundary + dispatch table):**
```python
DISPATCH = {"opA": opA, "opB": opB}

def handler(req):
    fn = DISPATCH.get(req["op"])
    if fn is None:
        raise ValueError("unknown op")
    return fn(req)
```

This enables:
- function identity guards
- direct calls
- better analysis

---

### 3.2 Convert “untyped dict payloads” to schema-friendly records
**Before:**
```python
user_id = payload["user_id"]
```

**After:**
```python
# after schema parse / pydantic boundary
user_id = payload.user_id
```

This aligns with Molt’s schema-compiled boundaries.

---

## 4. Numeric/data rewrites (vectorization on-ramp)

### 4.1 Avoid per-element Python callbacks in hot loops
**Before:**
```python
out = [f(x) for x in xs]  # if f is dynamic/closure-heavy
```

**After:**
- prefer built-in arithmetic expressions, or
- move `f` into a known function with stable identity and simple types

---

### 4.2 Replace nested dynamic loops with range-bounded loops
**Before:**
```python
for x in xs:
    for y in ys:
        ...
```

**After:**
```python
for i in range(len(xs)):
    x = xs[i]
    for j in range(len(ys)):
        y = ys[j]
        ...
```

This gives Molt explicit bounds and indexable access.

---

## 5. Web/ORM rewrites (performance wins)

### 5.1 Precompute constant regexes and JSON codecs
**Before:**
```python
import re
def f(s): return re.match(PAT, s)
```

**After:**
```python
import re
R = re.compile(PAT)
def f(s): return R.match(s)
```

This supports snapshotting and reduces cold start.

---

### 5.2 Replace dynamic model field access with explicit schema access
Where possible, route ORM IO through typed schemas at boundaries.

---

## 6. “Do not rewrite” rules
Agents MUST NOT apply rewrites that change semantics in these cases:
- relies on side effects in generator evaluation
- relies on dict iteration order changes due to mutation
- relies on monkeypatching builtins
- relies on reflection (`globals()`, `locals()`) in strict tier code

When in doubt:
- isolate to Tier 2 boundary
- do not “optimize” away semantics

---

## 7. Rewrite checklist (agent output)
Whenever an agent performs rewrites, it must log:
- pattern detected
- rewrite applied
- tier improvement expected (Tier 2 → Tier 1, etc.)
- tests added/updated
- resume command

---

## 8. Appendix: the intent behind this document (what “3” means)
This document exists to make “Molt-optimizable Python” **mechanically reachable**.

It is a collection of transformations that:
- reduce dynamic features in core logic
- push dynamism to boundaries
- produce patterns the compiler can reliably lower

In practice, this lets agents take a codebase that runs in Tier 2 and progressively move hot paths into Tier 0/1 without rewriting everything in Rust.
