# Dynamic Execution And Reflection Policy Contract
**Spec ID:** 0216
**Status:** Active
**Owner:** frontend + runtime + tooling
**Goal:** Prevent accidental expansion of high-dynamism semantics that can erode AOT performance, determinism, and deployability.

---

## 1. Current Policy (Default, Active)
For compiled Molt binaries, CPython dynamic semantics are fully supported except for the carve-outs below:
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
- Each differential test that relies on intentionally unsupported dynamism
  declares the complete policy at its source:
  `# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy`.
- `tools/compat/test_policy.py` is the canonical parser and projection authority;
  consumers discover the scope through `verification_scope_paths(...)` rather
  than maintaining path manifests.
- Lint-time policy checks fail if scope metadata, required policy documents, or
  the expected-failure contract drifts.
- The dynamic-policy guard checks the concrete fail-closed control-flow and
  diagnostic evidence in runtime import execution paths. A comment or marker is
  not proof of enforcement.

## 4. Future Enablement Gate (Explicitly Deferred)
Future support can be considered only behind a capability-gated, opt-in path after all of the following:
1. Documented utility analysis (which libraries/workloads are blocked today).
2. Reproducible native+wasm performance analysis showing acceptable overhead.
3. Spec updates across contracts + status + roadmap in the same change.
4. Targeted parity tests, determinism checks, and memory/regression evidence.
5. Explicit user approval before implementation begins.

Until these gates are satisfied, dynamic execution and unrestricted reflection remain policy-deferred.
