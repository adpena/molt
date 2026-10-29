"""Compatibility diagnostics for Molt."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

CompatibilityTier = Literal["unsupported"]
Impact = Literal["low", "medium", "high"]

VERIFIED_SUBSET_BOUNDARY = (
    "docs/spec/areas/compat/contracts/verified_subset_contract.md"
)
DEFAULT_SUBSET_WORKAROUND = (
    "run this path with CPython or rewrite it within Molt's verified subset"
)


@dataclass(frozen=True)
class CompatibilityIssue:
    feature: str
    tier: CompatibilityTier
    impact: Impact
    location: str
    alternative: str | None = None
    detail: str | None = None
    diagnostic_code: str | None = None

    def format_error(self) -> str:
        lines = [
            "MOLT_COMPAT_ERROR: unsupported construct",
            f"  feature: {self.feature}",
            f"  location: {self.location}",
            f"  tier: {self.tier}",
            f"  impact: {self.impact}",
            f"  boundary: {VERIFIED_SUBSET_BOUNDARY}",
        ]
        if self.detail:
            lines.append(f"  detail: {self.detail}")
        if self.alternative:
            lines.append(f"  replace: {self.alternative}")
        else:
            lines.append(f"  workaround: {DEFAULT_SUBSET_WORKAROUND}")
        return "\n".join(lines)


class CompatibilityError(RuntimeError):
    def __init__(self, issue: CompatibilityIssue) -> None:
        super().__init__(issue.format_error())
        self.issue = issue


class CompatibilityReporter:
    def __init__(self, *, source_path: str | None = None) -> None:
        self.source_path = source_path

    def _location(self, node: object) -> str:
        lineno = getattr(node, "lineno", None)
        col = getattr(node, "col_offset", None)
        if lineno is None:
            return self.source_path or "<unknown>"
        if self.source_path:
            return f"{self.source_path}:{lineno}:{col or 0}"
        return f"<unknown>:{lineno}:{col or 0}"

    def unsupported(
        self,
        node: object,
        feature: str,
        *,
        tier: CompatibilityTier = "unsupported",
        impact: Impact = "high",
        alternative: str | None = None,
        detail: str | None = None,
        diagnostic_code: str | None = None,
    ) -> CompatibilityError:
        location = self._location(node)
        issue = CompatibilityIssue(
            feature=feature,
            tier=tier,
            impact=impact,
            location=location,
            alternative=alternative,
            detail=detail,
            diagnostic_code=diagnostic_code,
        )
        return CompatibilityError(issue)
