from __future__ import annotations

from typing import Any, Iterable, Mapping


def classify_event_tone(event_name: str) -> str:
    name = event_name.lower()
    if "failed" in name or "error" in name or "cancel" in name or "timeout" in name:
        return "danger"
    if "token" in name or "rate" in name or "usage" in name:
        return "info"
    if "retry" in name or "input_required" in name:
        return "warn"
    if "complete" in name or "started" in name:
        return "ok"
    return "warn"


def classify_trace_status(status: str) -> str:
    norm = status.lower()
    if "run" in norm:
        return "status-running"
    if "retry" in norm:
        return "status-retrying"
    if "block" in norm or "fail" in norm:
        return "status-blocked"
    return ""


def compact_recent_events(
    events: Iterable[Mapping[str, Any]],
    *,
    limit: int = 80,
) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    cap = max(int(limit), 1)
    for raw in events:
        if not isinstance(raw, Mapping):
            continue
        row = {
            "event": str(raw.get("event") or ""),
            "message": str(raw.get("message") or ""),
            "detail": str(raw.get("detail") or ""),
            "at": str(raw.get("at") or ""),
            "tone": classify_event_tone(str(raw.get("event") or "")),
        }
        rows.append(row)
        if len(rows) >= cap:
            break
    return rows
