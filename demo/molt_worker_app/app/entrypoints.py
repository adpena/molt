from __future__ import annotations

from typing import Any


def _as_int(value: Any, default: int | None = None) -> int | None:
    if value is None:
        return default
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def list_items(payload: dict[str, Any]) -> dict[str, Any]:
    """Demo entrypoint for the Django offload contract.

    TODO(offload, owner:runtime, milestone:SL1): compile entrypoints into molt_worker.
    """
    user_id = _as_int(payload.get("user_id"))
    if user_id is None:
        raise ValueError("user_id must be an integer")
    q = payload.get("q")
    status = payload.get("status")
    cursor = payload.get("cursor")
    limit = _as_int(payload.get("limit"), 50) or 50
    limit = max(1, min(limit, 500))

    q_len = len(q or "")
    status_len = len(status or "")
    cursor_len = len(cursor or "")
    base = abs(user_id) * 1000 + q_len + status_len + cursor_len
    items: list[dict[str, Any]] = []
    open_count = 0
    closed_count = 0
    for idx in range(limit):
        is_open = idx % 2 == 0
        status_value = "open" if is_open else "closed"
        if is_open:
            open_count += 1
        else:
            closed_count += 1
        item_id = base + idx
        created_at = f"2026-01-{(idx % 28) + 1:02}T00:00:{idx % 60:02}Z"
        items.append(
            {
                "id": item_id,
                "created_at": created_at,
                "status": status_value,
                "title": f"Item {item_id}",
                "score": (idx % 100) / 100.0,
                "unread": idx % 3 == 0,
            }
        )

    next_cursor = f"{user_id}:{limit}" if items else None
    return {
        "items": items,
        "next_cursor": next_cursor,
        "counts": {"open": open_count, "closed": closed_count},
    }


def compute(payload: dict[str, Any]) -> dict[str, Any]:
    """Example compute entrypoint to exercise compiled path."""
    values = payload.get("values") or []
    scale = payload.get("scale", 1.0)
    offset = payload.get("offset", 0.0)
    if not isinstance(values, list):
        raise ValueError("values must be a list")
    scaled = []
    total = 0.0
    for val in values:
        f = float(val)
        out = f * scale + offset
        scaled.append(out)
        total += out
    return {"count": len(scaled), "sum": total, "scaled": scaled}
