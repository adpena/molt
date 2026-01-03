# Introspective Acceleration: Building Molt at Light Speed (Without Breaking It)

## Purpose

This document defines **how Molt is built**, not just what it is.

The goal is to combine:
- deep introspection
- ruthless feedback loops
- aggressive iteration speed

with:
- production hardening
- correctness guarantees
- long-term maintainability

**Fast iteration and production rigor are not opposites.**
They are achieved by deliberately structuring feedback, invariants, and stop conditions.

---

## Core Principle

> *Every iteration must teach us something, and every lesson must harden the system.*

Introspection is only valuable if it **changes decisions**.
Speed is only valuable if it **converges**.

---

## 1. Introspection as a First-Class Engineering Tool

### 1.1 What We Introspect (Continuously)

Molt is built by constantly observing itself and its inputs:

- Python programs under real workloads
- Runtime behavior of compiled binaries
- Guard hit/miss rates
- Deoptimization frequency
- Allocation patterns
- Tail latency
- Binary size growth
- CI failures and flakiness
- Developer friction points

If something is not measured, it is not improved.

---

### 1.2 Introspection ≠ Guessing

Rules:
- Observations never become assumptions without guards.
- Guards never exist without fallback.
- Fallbacks never exist without tests.
- Tests never exist without failure examples.

This chain is non-negotiable.

---

## 2. The Fast Loop: Spec → Implement → Measure → Decide

Molt development runs on **tight, explicit loops**:

1. **Spec**
   - Write or update a spec document.
   - Define invariants and non-goals.
   - Define acceptance criteria *before* coding.

2. **Implement**
   - Implement the smallest slice that satisfies the spec.
   - Prefer clarity over cleverness.
   - Prefer explicit state over implicit behavior.

3. **Measure**
   - Run benchmarks and differential tests.
   - Collect runtime feedback artifacts.
   - Measure real workloads, not microbenchmarks only.

4. **Decide**
   - Keep, modify, or delete the implementation.
   - Document the decision.
   - Update the spec if reality disagrees.

**Deletion is success**, not failure.

---

## 3. Guardrails That Enable Speed

### 3.1 Tiered Semantics Prevent Thrashing

By enforcing compatibility tiers:
- Tier 0 stays clean, fast, and simple.
- Tier 1 absorbs complexity and experimentation.

This prevents the entire system from slowing down to accommodate edge cases.

---

### 3.2 Explicit Invariants Enable Fearless Refactoring

Every major component must answer:
- What invariants does it rely on?
- What invariants does it provide?
- What happens if they are violated?

If invariants are explicit:
- refactors are safe
- performance work is localized
- AI agents can reason about correctness

---

## 4. Designing for AI-Assisted Development

This project assumes **AI coding partners are always present**.

Therefore:
- Specs must be machine-readable and human-readable.
- Invariants must be written in declarative language.
- Error messages must be precise and structured.
- IRs must reject invalid states loudly.

AI accelerates development only when:
- the system has clear contracts
- failure modes are explicit
- tests encode intent

---

## 5. Production Hardening Is Continuous, Not a Phase

### 5.1 No “Prototype-Only” Code

Every line of code is either:
- production-grade
- or temporary with a deletion plan

Temporary code must be:
- clearly marked
- isolated
- scheduled for removal

---

### 5.2 Performance Regressions Are Build Breakers

Molt treats performance as correctness:
- binary size regressions are failures
- tail latency regressions are failures
- allocation explosions are failures

CI must fail loudly when these occur.

---

## 6. Shipping Early Without Lying

Early Molt releases must:
- clearly state supported subsets
- clearly state forbidden features
- fail fast and loudly on violations

Undefined behavior is worse than slow behavior.

---

## 7. Decision Logging: Institutional Memory

Every major decision must be logged:
- what was tried
- what was measured
- what was chosen
- what was rejected
- why

These logs prevent:
- repeated mistakes
- cargo-cult optimizations
- regressions in reasoning

---

## 8. Light-Speed, Not Reckless Speed

Molt moves fast by:
- cutting scope aggressively
- enforcing invariants early
- measuring reality continuously
- deleting without remorse
- preferring boring solutions that work

Molt does **not** move fast by:
- guessing
- overgeneralizing
- chasing benchmarks without context
- trusting unverified assumptions

---

## Final Rule

> *If the system cannot explain itself, it is not ready to go faster.*

When Molt can explain:
- why it is fast
- why it is correct
- why it rejects certain programs

then accelerating further is safe.

That is how Molt reaches production at light speed.
