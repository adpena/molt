from __future__ import annotations

from molt.compat import (
    DEFAULT_SUBSET_WORKAROUND,
    VERIFIED_SUBSET_BOUNDARY,
    CompatibilityIssue,
)


def test_unsupported_diagnostic_names_boundary_and_default_workaround() -> None:
    rendered = CompatibilityIssue(
        feature="pow does not support keywords",
        tier="unsupported",
        impact="high",
        location="example.py:1:6",
    ).format_error()

    assert f"boundary: {VERIFIED_SUBSET_BOUNDARY}" in rendered
    assert f"workaround: {DEFAULT_SUBSET_WORKAROUND}" in rendered


def test_unsupported_diagnostic_prefers_specific_replacement() -> None:
    rendered = CompatibilityIssue(
        feature="dynamic call",
        tier="unsupported",
        impact="high",
        location="example.py:2:0",
        alternative="call a statically known function",
    ).format_error()

    assert "replace: call a statically known function" in rendered
    assert "workaround:" not in rendered
