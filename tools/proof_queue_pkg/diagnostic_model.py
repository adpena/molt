"""Shared proof diagnostic values and deterministic presentation helpers."""

from __future__ import annotations

import datetime as dt
from typing import Sequence

from tools.proof_queue_pkg import state

DIAGNOSTIC_EVIDENCE_MAX_CHARS = 640

TERMINAL_STALE_DIAGNOSTIC_IDS = frozenset(
    {
        "running-proof-child-missing",
        "running-proof-guard-timeout-expired",
    }
)


def _elapsed_since(started_at: str | None, elapsed_s: float | None = None) -> str:
    if elapsed_s is not None:
        return f"{elapsed_s:.1f}s"
    if not started_at:
        return "?"
    try:
        started = dt.datetime.fromisoformat(started_at)
    except ValueError:
        return "?"
    if started.tzinfo is None:
        started = started.replace(tzinfo=dt.UTC)
    elapsed = max(0.0, (dt.datetime.now(dt.UTC) - started).total_seconds())
    return f"{elapsed:.1f}s"


def _running_age_seconds(started_at: str | None) -> float | None:
    """Wall-clock seconds since ``started_at``, or None if unparseable."""
    if not started_at:
        return None
    try:
        started = dt.datetime.fromisoformat(started_at)
    except ValueError:
        return None
    if started.tzinfo is None:
        started = started.replace(tzinfo=dt.UTC)
    return max(0.0, (dt.datetime.now(dt.UTC) - started).total_seconds())


def _format_duration(seconds: float) -> str:
    if seconds < 60.0:
        return f"{seconds:.1f}s"
    if seconds < 3600.0:
        return f"{seconds / 60.0:.1f}m"
    return f"{seconds / 3600.0:.1f}h"


def _diagnostic(
    *,
    signal_id: str,
    severity: str,
    summary: str,
    evidence: str,
    next_action: str,
    scopes: Sequence[str] = (),
    artifacts: Sequence[str] = (),
) -> dict[str, object]:
    return {
        "signal_id": signal_id,
        "severity": severity,
        "summary": summary,
        "evidence": state._shorten(evidence, DIAGNOSTIC_EVIDENCE_MAX_CHARS),
        "next_action": next_action,
        "scopes": list(scopes),
        "artifacts": list(artifacts),
    }


def _diagnostics_have_terminal_stale_signal(
    diagnostics: Sequence[dict[str, object]],
) -> bool:
    return any(
        diagnostic.get("signal_id") in TERMINAL_STALE_DIAGNOSTIC_IDS
        for diagnostic in diagnostics
    )


def _diagnostics_have_signal(
    diagnostics: Sequence[dict[str, object]], signal_id: str
) -> bool:
    return any(diagnostic.get("signal_id") == signal_id for diagnostic in diagnostics)


def _format_diagnostic_summary(diagnostics: list[dict[str, object]]) -> str | None:
    if not diagnostics:
        return None
    first = diagnostics[0]
    return f"{first['signal_id']} [{first['severity']}]: {state._shorten(str(first['summary']))}"


def _diagnostic_artifacts(diagnostics: Sequence[dict[str, object]]) -> list[str]:
    if not diagnostics:
        return []
    artifacts = diagnostics[0].get("artifacts", [])
    if not isinstance(artifacts, list):
        return []
    return [str(path) for path in artifacts]
