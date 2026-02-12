"""Authoritative stdlib full-coverage attestation manifest.

Any stdlib module/submodule absent from this tuple is classified as
`intrinsic-partial` by `tools/check_stdlib_intrinsics.py`.

Update workflow:
1. Add module/submodule names to `STDLIB_FULLY_COVERED_MODULES` only after full
   CPython 3.12+ API/PEP parity is landed for Molt-supported semantics.
2. Add an explicit required-intrinsics tuple for every attested module in
   `STDLIB_REQUIRED_INTRINSICS_BY_MODULE` (empty tuple is allowed only when the
   module intentionally has no direct intrinsic loads).
3. Keep `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS` aligned with differential tests
   that intentionally remain unsupported under vision/break-policy constraints
   (for example `exec`/`eval` heavy semantics).
"""

from __future__ import annotations


STDLIB_FULLY_COVERED_MODULES: tuple[str, ...] = ()

# Every module listed in STDLIB_FULLY_COVERED_MODULES must have a key here.
# Value is the canonical intrinsic contract required for full coverage.
STDLIB_REQUIRED_INTRINSICS_BY_MODULE: dict[str, tuple[str, ...]] = {}

# Differential tests that are intentionally expected to fail in Molt because
# they rely on "too much dynamism" outside current supported semantics.
TOO_DYNAMIC_POLICY_DOC_REFERENCES: tuple[str, ...] = (
    "docs/spec/areas/core/0000-vision.md",
    "docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md",
    "docs/spec/areas/testing/0007-testing.md",
    "docs/spec/areas/compat/0016_VERIFIED_SUBSET_CONTRACT.md",
)

# Differential tests aligned with the documented policy references above.
# Keep this list focused on behavior intentionally excluded from supported
# semantics, not temporary lowering gaps.
TOO_DYNAMIC_EXPECTED_FAILURE_TESTS: tuple[str, ...] = (
    "tests/differential/planned/exec_eval_compile_capability_errors.py",
    "tests/differential/planned/exec_class_body_locals.py",
    "tests/differential/planned/exec_class_scope.py",
    "tests/differential/planned/exec_in_function_locals.py",
    "tests/differential/planned/exec_locals_mapping.py",
    "tests/differential/planned/exec_locals_scope.py",
    "tests/differential/planned/exec_locals_shadowing.py",
    "tests/differential/planned/eval_locals_scope.py",
)
