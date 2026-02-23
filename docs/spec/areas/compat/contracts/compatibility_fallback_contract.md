# Compatibility & Fallback Contract
**Spec ID:** 0211
**Status:** Draft (implementation-targeting)
**Owner:** frontend + runtime + tooling
**Goal:** Provide a deterministic, production-grade policy for handling unsupported Python constructs with explicit fallback tiers, standardized warnings, and clear error behavior.

---

## 1. Principles
- **Determinism first:** Unsupported features never silently degrade.
- **Explicit policy:** Users select fallback behavior via CLI flags or config.
- **Actionable guidance:** Every fallback warning includes the performance impact and a Molt-native alternative.
- **Hard errors when required:** If policy forbids fallback, compilation fails with a detailed error.
- **No implicit CPython fallback:** `molt run` / `molt build` never fall back to CPython.
- **No CPython in binaries:** compiled artifacts are self-contained; the bridge is tooling-only and treated as unavailable for production builds.

---

## 2. Tiers & Outcomes
| Tier | Meaning | Behavior |
| --- | --- | --- |
| **native** | Fully supported by Molt | Compiles to optimized IR/runtime ops. |
| **guarded** | Supported with runtime guards | Compiles; emits guard + deopt path. |
| **bridge** | Execute via CPython/worker bridge | Emits warning; bridge must be enabled by policy. |
| **unsupported** | No safe fallback | Hard error with explicit guidance. |

---

## 3. Policy Controls
- CLI flag: `molt build --fallback {error|bridge}`
  - `error` (default): Any non-native tier fails compilation.
  - `bridge`: Allows bridge-tier constructs with warnings in tooling-only flows; compiled binaries treat bridge as unavailable and fail with a bridge-unavailable error.

---

## 4. Standardized Diagnostics
All non-native features must emit a warning (or error) with:
- Feature identifier (e.g., `with`, `open()`, `match`, `async for`).
- Source location (file:line:col).
- Tier selected (`guarded` or `bridge`).
- **Impact**: perf/memory risk (Low/Med/High).
- **Replacement guidance**: Molt-native alternative or refactor suggestion.

**Format (example):**
```
[MOLT_COMPAT] tier=bridge impact=high feature=open() location=app.py:12:8
  fallback: CPython bridge
  replace: use molt.stdlib.io.open or molt.stdlib.io.stream
```

---

## 5. First-pass Implementation Rules
- Frontend must detect unsupported constructs during AST lowering.
- The compiler selects the **best available tier** based on the feature and policy.
- If the policy forbids fallback, compilation fails with a `MOLT_COMPAT_ERROR`.
- If the policy allows fallback but no bridge is wired, compilation fails with a
  `MOLT_COMPAT_BRIDGE_UNAVAILABLE` error.

---

## 6. Roadmap Hooks
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:missing): full `with`/contextlib lowering with exception flow.
- TODO(type-coverage, owner:frontend, milestone:TC3, priority:P2, status:missing): full import/module fallback classification.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract and enforcement hooks.
