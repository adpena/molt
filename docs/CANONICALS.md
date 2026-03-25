# Canonical Reading List

These are the must-read documents for anyone adding or reviewing functionality.

**Version policy:** Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11.

## Core
- [AGENTS.md](../AGENTS.md)
- [CONTRIBUTING.md](../CONTRIBUTING.md)
- [ROOT_LAYOUT.md](ROOT_LAYOUT.md)
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- In [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md), treat "Rules Of Thumb For New Work" and "Coverage And Optimization Strategy" as mandatory execution policy for parity + optimization work.
- [OPERATIONS.md](OPERATIONS.md)
- [INDEX.md](INDEX.md)
- Platform pitfalls: [README.md](../README.md), [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md), and [OPERATIONS.md](OPERATIONS.md) (macOS/Linux/Windows/WASM notes).

## Vision and Scope
- 0000 Vision: [spec/areas/core/0000-vision.md](spec/areas/core/0000-vision.md)
- 0025 Reproducible And Deterministic Mode: [spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md](spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md)
- 0800 What Molt Is Willing To Break: [spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md)
- STATUS: [spec/STATUS.md](spec/STATUS.md)
- [ROADMAP.md](../ROADMAP.md)
- [ROADMAP_90_DAYS.md](ROADMAP_90_DAYS.md)

## Specifications
- [spec/README.md](spec/README.md)
- [spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
- [spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
- [spec/areas/compat/README.md](spec/areas/compat/README.md)
- [spec/areas/compat/surfaces/language/language_surface_matrix.md](spec/areas/compat/surfaces/language/language_surface_matrix.md)
- [spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md](spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- [spec/areas/compat/surfaces/c_api/c_api_surface_index.md](spec/areas/compat/surfaces/c_api/c_api_surface_index.md)
- [spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)
- [spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md](spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md)
- `tools/stdlib_module_union.py`
- [spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md](spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md)
- [spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md](spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md)
- [spec/areas/compat/plans/stdlib_lowering_plan.md](spec/areas/compat/plans/stdlib_lowering_plan.md)
