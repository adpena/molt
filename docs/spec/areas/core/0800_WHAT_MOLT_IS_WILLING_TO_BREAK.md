# What Molt Is WIlling To Break
**Spec ID:** 0800
**Status:** Foundational Positioning
**Audience:** contributors, adopters, skeptics, investors

---

## Why this document exists
Most infrastructure projects fail not because they are wrong, but because they refuse to commit.

Molt is not trying to be everything to everyone.
This document defines the **lines Molt will not cross**, even if doing so limits compatibility or short-term adoption.

These breaks are not accidents. They are the source of Molt’s power.

---

## 1. Molt breaks maximal Python dynamism
Molt does not promise to support:
- arbitrary monkeypatching at runtime
- mutation of global state after startup
- reflection-heavy patterns that prevent static reasoning

This enables:
- ahead-of-time compilation
- safe concurrency
- predictable performance

---

## 2. Molt breaks CPython ABI compatibility
Molt does not load arbitrary CPython C extensions.

This enables:
- static binaries
- WASM targets
- long-term runtime stability

---

## 3. Molt breaks implicit async
Blocking code is not silently tolerated.

Molt requires:
- explicit async boundaries
- structured concurrency
- cancellation awareness

---

## 4. Molt breaks legacy pandas semantics
Performance-first dataframe tiers are the default.
Compatibility is opt-in and measured.

---

## 5. Molt breaks rewrite culture
Molt rejects the idea that Python is a prototype language.

---

## 6. Molt breaks implicit parallelism
Molt does not allow silent shared-memory parallelism or hidden scheduling.

This enables:
- explicit concurrency boundaries
- predictable performance under load
- correctness-first parallel speedups via isolated runtimes/actors or safe, opt-in shared types

---

## Identity statement
**Molt is Python with explicit contracts, built for long-lived services and pipelines.**
