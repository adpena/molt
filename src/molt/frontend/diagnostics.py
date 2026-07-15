"""Typed frontend lowering rejections and compatibility conversion authority."""

from __future__ import annotations

from typing import NoReturn, cast

from molt.compat import CompatibilityReporter
from molt.frontend.frontend_diagnostics_generated import (
    FRONTEND_DIAGNOSTIC_METADATA,
    FrontendDiagnostic,
)


class FrontendRejection(Exception):
    """A deterministic compiler diagnostic, never an executable stub marker."""

    __slots__ = ()


def raise_compatibility_error(
    reporter: CompatibilityReporter,
    node: object,
    rejection: FrontendRejection,
) -> NoReturn:
    """Convert the typed rejection into the public compatibility diagnostic."""

    fields = rejection.args
    diagnostic = cast(FrontendDiagnostic, fields[0])
    message = cast(str, fields[1])
    alternative = cast(str | None, fields[2] if len(fields) > 2 else None)
    site_detail = cast(str | None, fields[3] if len(fields) > 3 else None)
    metadata = FRONTEND_DIAGNOSTIC_METADATA[diagnostic]
    detail = f"{metadata.detail}; {site_detail}" if site_detail else metadata.detail
    raise reporter.unsupported(
        node,
        message,
        tier=metadata.tier,
        impact=metadata.impact,
        alternative=alternative,
        detail=detail,
    ) from rejection


__all__ = [
    "FrontendDiagnostic",
    "FrontendRejection",
    "raise_compatibility_error",
]
