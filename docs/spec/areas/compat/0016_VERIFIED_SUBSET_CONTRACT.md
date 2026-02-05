# Verified Subset Contract
**Spec ID:** 0016
**Status:** Draft (implementation-targeting)
**Owner:** frontend + runtime + tooling
**Goal:** Define the precise meaning and guarantees of the Molt "verified subset".

---

## 1. Definition
The verified subset is the set of language/runtime/stdlib behaviors for which
Molt guarantees CPython 3.12+ equivalence under deterministic inputs. Equivalence
means identical stdout/stderr/exit codes and matching exception types/messages
for covered behaviors. Version-specific divergences across 3.12/3.13/3.14 must
be documented in specs and tests.

## 2. Scope
- Applies to Tier 0 builds with fallback policy `error`.
- Excludes any feature that requires bridge fallback, dynamic imports, or
  capability-gated I/O unless explicitly listed in the manifest.
- The canonical capability status remains in `docs/spec/STATUS.md`.

## 3. Guarantees
- Deterministic semantics for covered behavior.
- No silent fallback to CPython or bridge tiers.
- Type guard violations raise `TypeError: type guard mismatch`.
- Deterministic build inputs (lockfiles, compiler version, target triple).

## 4. Verification Criteria
A feature may be listed in the verified subset only if all of the following are
true:
- It is covered by a differential test in the suites listed below.
- It is documented in `docs/spec/STATUS.md` as supported.
- Any capability gating is explicitly documented in the feature spec.

## 5. Promotion/Demotion Rules
- To promote a feature: add a differential test, update STATUS, update this
  manifest, and run the verified subset suite.
- If a verified feature regresses, treat it as a P0; revert or disable until
  parity is restored.

## 6. Relationship to Other Specs
- `docs/spec/STATUS.md` remains the canonical capability summary.
- `docs/spec/areas/testing/0007-testing.md` defines the differential testing harness.
- `docs/spec/areas/compat/0211_COMPATIBILITY_AND_FALLBACK_CONTRACT.md` defines fallback tiers.

## 7. Tooling + CI Enforcement
- `tools/verified_subset.py check` validates the manifest and referenced paths.
- `tools/verified_subset.py run` executes the listed differential suites.
- CI must run the `check` command at minimum; `run` is recommended when suite
  duration is acceptable.

## 8. Verification Targets (machine readable)
```json
{
  "python_version_min": "3.12",
  "differential_suites": [
    "tests/differential/basic"
  ],
  "status_doc": "docs/spec/STATUS.md"
}
```

## 9. Non-Goals
- Proving full Python equivalence across all stdlib modules.
- Guaranteeing behavior for dynamic metaprogramming outside the manifest.
