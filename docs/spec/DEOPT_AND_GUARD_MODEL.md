# Molt Guard & Deoptimization Model
**Status:** Canonical (runtime + compiler contract)
**Purpose:** Define how Tier 1 fast paths are guarded, how deopt works, and what must be preserved.
**Audience:** Runtime engineers, compiler engineers, AI agents implementing optimization passes.

---

## 0. Why this exists

Molt’s goal is to be fast **without** requiring whole-program proofs for every program.
Tier 1 exists to:
- optimize common cases
- retain safety
- fall back when reality is more dynamic than expected

This doc defines the *only allowed* way to do that: guards + deopt.

---

## 1. Definitions

- **Fast path:** optimized code with assumptions.
- **Guard:** runtime check verifying an assumption.
- **Deopt:** jump to a slower semantics-preserving path when a guard fails.
- **Deopt state:** the minimal reconstructable state needed to resume execution in Tier 2.
- **Safepoint:** a location where deopt is allowed.

---

## 2. Guard types (v0.1)

### 2.1 Type guards
- `is_int(x)`
- `is_str(x)`
- `is_list(x)` etc.

### 2.2 Shape guards
- list length stable
- tuple length fixed
- dict version tag unchanged (future)

### 2.3 Function identity guards
- `f` refers to known function `fn_id`
- no monkeypatch observed (strict tier forbids monkeypatch anyway)

### 2.4 Overflow/Range guards
- range length computation does not overflow
- arithmetic does not overflow when using narrow types

### 2.5 Capability guards (WASM/edge)
- required host capability exists (e.g., crypto, fetch)
- else deopt or trap depending on tier

---

## 3. Safepoints: where deopt is permitted

Deopt must occur only at explicit safepoints:
- loop headers
- before/after calls
- before raising exceptions
- before container mutations
- at explicit `Guard(...)` IR nodes

This keeps state reconstruction tractable.

---

## 4. Deopt mechanism (conceptual)

### 4.1 Code shape
Tier 1 code is compiled as:

1. **Fast path block**
2. Guards that, on failure, jump to:
3. **Deopt block** (Tier 2 bridge)

### 4.2 Deopt block responsibilities
The deopt block must:
- reconstruct Python-level objects as needed
- map Molt locals to Tier 2 locals
- transfer control to the Tier 2 interpreter/runtime at a defined resume point

### 4.3 What must be preserved
- observable side effects already performed
- ordering of side effects within supported subset
- exception behavior

If a fast path has performed partial side effects and then deopts, Tier 2 must resume *after* those effects, not replay them.

---

## 5. Deopt state contract

### 5.1 Minimal state
At each safepoint eligible for deopt, compiler must be able to reconstruct:
- current function/frame id
- program counter / resume id
- local variables needed for continuation
- operand stack / temporary values if applicable

### 5.2 Serialization
For WASM portability, deopt state must be representable in:
- a compact struct layout
- pointer-free form where possible
- stable schema versioning

---

## 6. How guards are emitted (rules)

### 6.1 Prefer early guards
Guard as close as possible to the assumption introduction:
- before entering a loop
- before calling a function under identity assumptions

### 6.2 Group guards
Multiple guards may be grouped into a single guard block to reduce overhead, but must keep precise resume points.

### 6.3 Guard cost budgeting
Tier 1 should not emit more guard overhead than the expected win.
If guard overhead dominates, the idiom should remain Tier 2.

---

## 7. Examples

### 7.1 `list(range(n))` with `n` dynamic
Tier 1 lowering:
- guard `is_int(n)`
- guard `n` within i64
- compute `len`
- allocate
- fill loop
On failure:
- deopt to Tier 2 implementation of `list(range(n))` or generic `list(iterable)`.

### 7.2 `f(x)` where `f` assumed known
Tier 1:
- guard identity of `f`
- call direct
On failure:
- deopt and do dynamic dispatch.

---

## 8. When to trap instead of deopt
In **strict/WASM/edge tiers**, some behaviors are disallowed:
- `eval/exec`
- dynamic imports
- reflection-heavy metaprogramming

For these:
- either compile-time error
- or runtime `Trap(reason)` (not deopt)

Rule: If Tier 2 is unavailable (e.g., strict WASM build), deopt is not possible—trap is required.

---

## 9. Testing the guard/deopt system

### 9.1 Guard fuzzing
Randomize inputs to attempt to break assumptions and ensure:
- deopt triggers correctly
- results match Tier 2 oracle

### 9.2 Differential testing
For each idiom:
- run fast path case (guards hold)
- run fail path case (guards fail)
Both must match oracle behavior.

### 9.3 Telemetry
Runtime must optionally emit:
- guard hit/miss counters
- deopt reasons
- time spent in deopt

These power the profile-guided roadmap.

---

## 10. AI agent checklist
When implementing Tier 1 optimizations:
- declare assumptions explicitly (guards)
- ensure safepoints exist
- write at least one test that forces deopt
- write at least one test that stays in fast path
- include resume instructions and logs per project conventions
