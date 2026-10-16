# Dynamic Execution And Reflection Policy Contract
**Spec ID:** 0216
**Status:** Active
**Owner:** frontend + runtime + tooling
**Goal:** Prevent accidental expansion of high-dynamism semantics that can erode AOT performance, determinism, and deployability.

---

## 1. Current Policy (Default, Active)
For compiled Molt binaries, the [Python contract](verified_subset_contract.md)
limits excessive dynamism that obstructs efficient ahead-of-time compilation
and standalone execution. Verification records implementation evidence separately
from that intended scope. The restrictions concern:
- unrestricted `eval`/`exec` execution paths
- runtime monkeypatching as a general semantic compatibility goal, including
  reassigning `__bases__` once a class's layout is fixed: class layouts, method
  resolution orders and devirtualized calls are compiled against the bases a
  class is created with, so a reassignment would invalidate them across the
  program. The assignment raises `TypeError`; differential cases that exercise it
  carry the expected-failure policy below.
- unrestricted reflection/introspection lanes that block static reasoning

Each exclusion requires a concrete performance or compilation rationale. Missing
implementation, a slow implementation or a failing conformance test cannot justify
an exclusion. Behavior inside the contract remains a correctness and performance
obligation until implemented and verified.

This aligns with:
- `docs/spec/areas/core/0000-vision.md` (Tier 0 constraints)
- `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md` (intentional break policy)

## 2. Allowed Surface (Now)
- Restricted, deterministic runtime lanes that do not widen dynamic execution semantics.
- Reflection/introspection support that is explicitly scoped and test-backed.
- Capability-gated behavior that is already part of approved contracts.

The carve-outs are not a blanket ban on mutable globals, object attributes,
function defaults, or code/closure introspection. Each supported operation must
preserve its own binding, ownership, invalidation, exception, and version
contracts; support on one backend does not grant it to another. Consult the
[language surface index](../surfaces/language/language_surface_matrix.md) and
[call binding contract](call_argument_binding_contract.md) for those boundaries.

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
