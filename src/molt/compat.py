"""Compatibility and fallback diagnostics for Molt."""

from __future__ import annotations

from dataclasses import dataclass
import os
import sys
from typing import Literal

FallbackPolicy = Literal["error", "bridge"]
FallbackTier = Literal["native", "guarded", "bridge", "unsupported"]
Impact = Literal["low", "medium", "high"]


@dataclass(frozen=True)
class CompatibilityIssue:
    feature: str
    tier: FallbackTier
    impact: Impact
    location: str
    alternative: str | None = None
    detail: str | None = None

    def format_warning(self) -> str:
        lines = [
            f"[MOLT_COMPAT] tier={self.tier} impact={self.impact} feature={self.feature} location={self.location}",
        ]
        if self.detail:
            lines.append(f"  detail: {self.detail}")
        if self.alternative:
            lines.append(f"  replace: {self.alternative}")
        return "\n".join(lines)

    def format_error(self) -> str:
        lines = [
            "MOLT_COMPAT_ERROR: unsupported construct",
            f"  feature: {self.feature}",
            f"  location: {self.location}",
            f"  tier: {self.tier}",
            f"  impact: {self.impact}",
        ]
        if self.detail:
            lines.append(f"  detail: {self.detail}")
        if self.alternative:
            lines.append(f"  replace: {self.alternative}")
        return "\n".join(lines)

    def runtime_message(self) -> str:
        msg = (
            f"{self.feature} requires fallback tier={self.tier} (impact={self.impact})"
        )
        if self.alternative:
            msg += f". Replace with: {self.alternative}"
        return msg


class CompatibilityError(RuntimeError):
    def __init__(self, issue: CompatibilityIssue) -> None:
        super().__init__(issue.format_error())
        self.issue = issue


class CompatibilityReporter:
    def __init__(self, policy: FallbackPolicy, source_path: str | None = None) -> None:
        self.policy = policy
        self.source_path = source_path

    def _location(self, node: object) -> str:
        lineno = getattr(node, "lineno", None)
        col = getattr(node, "col_offset", None)
        if lineno is None:
            return self.source_path or "<unknown>"
        if self.source_path:
            return f"{self.source_path}:{lineno}:{col or 0}"
        return f"<unknown>:{lineno}:{col or 0}"

    def warn(self, issue: CompatibilityIssue) -> None:
        disabled = os.environ.get("MOLT_COMPAT_WARNINGS", "").strip().lower()
        if disabled in {"0", "false", "no", "off"}:
            return
        print(issue.format_warning(), file=sys.stderr)

    def error(self, issue: CompatibilityIssue) -> CompatibilityError:
        return CompatibilityError(issue)

    def unsupported(
        self,
        node: object,
        feature: str,
        *,
        tier: FallbackTier = "unsupported",
        impact: Impact = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> CompatibilityError:
        location = self._location(node)
        issue = CompatibilityIssue(
            feature=feature,
            tier=tier,
            impact=impact,
            location=location,
            alternative=alternative,
            detail=detail,
        )
        if tier in {"bridge", "guarded"}:
            self.warn(issue)
        return self.error(issue)

    def bridge_unavailable(
        self,
        node: object,
        feature: str,
        *,
        impact: Impact = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> CompatibilityIssue:
        location = self._location(node)
        issue = CompatibilityIssue(
            feature=feature,
            tier="bridge",
            impact=impact,
            location=location,
            alternative=alternative,
            detail=detail,
        )
        self.warn(issue)
        return issue
