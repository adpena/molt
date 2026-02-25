# Dynamic Execution And Reflection Policy Contract
**Spec ID:** 0216
**Status:** Active
**Owner:** frontend + runtime + tooling
**Goal:** Prevent accidental expansion of high-dynamism semantics that can erode AOT performance, determinism, and deployability.

---

## 1. Current Policy (Default, Active)
For compiled Molt binaries, the following are intentionally unsupported as active roadmap targets:
- unrestricted `eval`/`exec` execution paths
- runtime monkeypatching as a general semantic compatibility goal
- unrestricted reflection/introspection lanes that block static reasoning

This aligns with:
- `docs/spec/areas/core/0000-vision.md` (Tier 0 constraints)
- `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md` (intentional break policy)

## 2. Allowed Surface (Now)
- Restricted, deterministic runtime lanes that do not widen dynamic execution semantics.
- Reflection/introspection support that is explicitly scoped and test-backed.
- Capability-gated behavior that is already part of approved contracts.

## 3. Tooling Guardrails
- Differential tests that rely on intentionally unsupported dynamism remain registered in:
  `tools/stdlib_full_coverage_manifest.py` -> `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`.
- Lint-time policy checks must fail if policy references drift or if the dynamic expected-failure contract is broken.
- Runtime import execution paths that remain intentionally restricted should carry `dynamic-exec-policy` notes in code.

## 4. Future Enablement Gate (Explicitly Deferred)
Future support can be considered only behind a capability-gated, opt-in path after all of the following:
1. Documented utility analysis (which libraries/workloads are blocked today).
2. Reproducible native+wasm performance analysis showing acceptable overhead.
3. Spec updates across contracts + status + roadmap in the same change.
4. Targeted parity tests, determinism checks, and memory/regression evidence.
5. Explicit user approval before implementation begins.

Until these gates are satisfied, dynamic execution and unrestricted reflection remain policy-deferred.
