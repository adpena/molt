# Language Surface Matrix (CPython 3.12+)

**Status:** Active
**Owner:** frontend + runtime
**Scope:** Python language reference surface tracked for CPython 3.12/3.13/3.14 across native and wasm.

## Canonical Inputs
- CPython language reference index:
  - `docs/python_documentation/python-3.12-docs-text/reference/index.txt`
- Differential coverage index:
  - `tests/differential/COVERAGE_INDEX.yaml`

## Canonical Language Surface Files
- Type coverage:
  - `docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md`
- Syntax coverage:
  - `docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md`
- Semantic behavior coverage:
  - `docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md`
- Generated PEP coverage map:
  - `docs/spec/areas/compat/surfaces/language/core_language_pep_coverage.generated.md`
- Generated generator/async-generator API map:
  - `docs/spec/areas/compat/surfaces/language/generator_api_coverage.generated.md`

## Coverage Dimensions (Required)
Every new language-compat entry should explicitly specify:
- CPython version lane (`3.12`, `3.13`, `3.14`) or version-gated rule.
- Native status.
- WASM status (`wasi` and browser-like host lanes when applicable).
- Evidence tests (prefer differential tests).

## Quality Rules
- Do not claim `behavior_full` without version-specific parity evidence.
- If one file claims support and another claims missing for the same language surface, treat as a blocker and resolve in the same change.
- Track intentional divergences explicitly and link to policy/spec rationale.
