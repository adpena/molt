# Molt Lowering Rules
**Status:** Canonical (compiler-facing)
**Purpose:** Define deterministic, testable transforms from Python AST → Molt IR for supported idioms.
**Audience:** Compiler engineers, optimization authors, AI agents writing compiler passes.

---

## 0. Terms

- **Pattern:** A recognizable AST/IR shape (e.g., `list(range(n))`).
- **Lowering:** Converting a Pattern into a smaller set of IR primitives (loops, alloc, calls).
- **Tier:** Optimization/semantic level (Tier 0 static, Tier 1 guarded, Tier 2 dynamic).
- **Guard:** Runtime check that enables a fast path (Tier 1).
- **Deopt:** Transfer of control from fast path to dynamic semantics when a guard fails.

---

## 1. Canonical IR primitives
These are the *only* primitives idiom lowerings may emit (v0.1). Everything else should be expressed in terms of these.

### 1.1 Control
- `Loop(counted)` — counted loop with canonical induction var `i: i64`
- `If(cond)` — branch
- `Break/Continue`
- `Trap(reason)` — compilation/runtime trap with reason

### 1.2 Memory / containers
- `AllocVec(len, dtype)` — allocate contiguous vector
- `VecSet(vec, idx, value)`
- `VecGet(vec, idx)`
- `AllocDict(capacity_hint)`
- `DictSet(dict, key, value)`
- `DictGet(dict, key)`
- `AllocTuple(len)`
- `TupleSet(tuple, idx, value)`

### 1.3 Arithmetic / comparisons
- `Add/Sub/Mul/Div/Mod`
- `CmpEq/Lt/Le/Gt/Ge`
- `And/Or/Not`

### 1.4 Calls / effects
- `CallKnown(fn_id, args...)` — direct call to known function
- `CallDyn(obj, args...)` — dynamic call (Tier 2 only)
- `Raise(exc)` — raise exception
- `Return(value)`

### 1.5 Range primitive
- `RangeTriplet(start, stop, step)` — normalized triplet
- `RangeLen(triplet)` — computed length (may guard for overflow)
- `RangeAt(triplet, i)` — ith value

---

## 2. Lowering cookbook (idioms)

Each rule includes:
- **Pattern**
- **Tier eligibility**
- **Lowering steps**
- **Correctness notes**
- **Tests**

### 2.1 `for i in range(n): body`
**Pattern:** `For(target=i, iter=Call(range,[n]), body=...)`
**Tier:** 0/1

**Lowering:**
1. Normalize: `trip = RangeTriplet(0, n, 1)`
2. Length: `len = RangeLen(trip)`
3. Emit counted loop `Loop(i=0..len-1)`
4. Inside loop: `i_val = RangeAt(trip, i)`
5. Bind `i := i_val`
6. Lower `body`

**Correctness notes:**
- Python `range` supports negative steps; must normalize.
- `RangeLen` must match Python semantics (empty ranges allowed).
- Overflow behavior: in Tier 0 require `n` fits i64 or raise compile-time error; Tier 1 guard.

**Tests:**
- `n=0,1,10`
- `n<0` (range empty)
- `n` near i64 boundary (Tier 1 guard fail → deopt)

---

### 2.2 `list(range(a,b,s))`
**Pattern:** `Call(list, [Call(range,[a,b,s])])`
**Tier:** 0/1

**Lowering:**
1. Normalize: `trip = RangeTriplet(a,b,s)`
2. `len = RangeLen(trip)`
3. `vec = AllocVec(len, i64)` (dtype may generalize; start with i64)
4. Emit `Loop(i=0..len-1)`:
   - `v = RangeAt(trip, i)`
   - `VecSet(vec, i, v)`
5. Return `vec`

**Correctness notes:**
- In Python, list of range yields ints; Molt may use i64.
- If `a/b/s` not statically known, Tier 1 emits guards:
  - args are ints
  - step != 0
  - length computation does not overflow

**Tests:**
- `list(range(5)) == [0,1,2,3,4]`
- `list(range(5,0,-2)) == [5,3,1]`
- `step=0` raises `ValueError` (must match)

---

### 2.3 `tuple(range(...))`
Same as `list(range(...))`, but:
- allocate tuple
- set via `TupleSet`
- tuple is immutable after construction

---

### 2.4 List comprehension over range
**Pattern:** `[EXPR(x) for x in range(...)]`
**Tier:** 0/1

**Lowering:**
1. Lower range to `trip/len`
2. `vec=AllocVec(len, <dtype_of_expr or Any>)`
3. Counted loop:
   - bind `x`
   - lower `EXPR(x)` into `v`
   - `VecSet(vec,i,v)`

**Notes:**
- If `EXPR` may throw, exceptions propagate normally.
- If dtype unknown, start with `Any` or specialize later.

**Tests:**
- simple arithmetic
- function call inside expr (Tier 1 guard if `f` is known)

---

### 2.5 `sum(range(...))`
**Pattern:** `Call(sum,[Call(range,...)])`
**Tier:** 0/1

**Lowering options:**
- **Analytic** (preferred): if step constant, use arithmetic series:
  - `len = RangeLen(trip)`
  - `first = RangeAt(trip,0)` if len>0 else 0
  - `last = RangeAt(trip,len-1)` if len>0 else 0
  - `sum = len*(first+last)/2` (careful with overflow)
- **Loop** fallback: accumulate in counted loop

**Guards:**
- overflow checks → deopt
- numeric type must be int

**Tests:**
- compare with CPython for random ranges
- huge ranges to force overflow guard/deopt

---

### 2.6 `any(iterable)` / `all(iterable)`
**Tier:** 0/1 if iterable is recognized (range, list, tuple) and predicate is implicit truthiness.

**Lowering:**
- Short-circuit loop with truthiness checks.
- For `range`, exploit emptiness quickly.

**Notes:**
- If iterable is generator or side-effectful, Tier 2 only.

---

### 2.7 `enumerate(range(...))`
**Pattern:** `for (i,x) in enumerate(range(...))`
**Tier:** 0/1

**Lowering:**
- use one counted loop
- `i` is induction var
- `x = RangeAt(trip,i)`

---

### 2.8 `zip(range(a), range(b))`
**Tier:** 0/1 if both are ranges and no `strict=True` behavior required (Python 3.10+ has strict in itertools, but built-in zip has no strict).

**Lowering:**
- compute `len = min(lenA,lenB)`
- one counted loop
- compute each `RangeAt`

---

## 3. Lowering constraints

### 3.1 No hidden allocations
If a lowering allocates, it MUST be explicit in IR (`AllocVec`, etc.).

### 3.2 No dynamic calls in Tier 0/1
Tier 0/1 lowerings cannot emit `CallDyn` except inside a deopt block.

### 3.3 Deterministic lowering
Given the same AST and Tier configuration, lowering must be deterministic.
No heuristic-only transforms without a controlling flag.

---

## 4. Validation and testing

### 4.1 Golden tests
For each idiom, maintain:
- input source snippet
- expected IR (pretty-printed)
- expected runtime behavior vs CPython oracle (where applicable)

### 4.2 Differential oracle
For Tier 2 fallbacks and deopt exits, use CPython as the oracle for:
- result
- raised exception type/message class (message may be loose)
- side effects ordering (within supported subset)

---

## 5. Extension points (how to add a new idiom)
A new idiom requires:
1. Pattern match spec (AST + type facts used)
2. Tier eligibility
3. Lowering steps to IR primitives
4. Guard/deopt plan
5. Test suite additions
