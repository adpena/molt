from __future__ import annotations

import json
import os
import threading
import time
from typing import Any

from django.http import JsonResponse

from molt_accel import MoltAccelError, molt_offload


_METRICS_LOCK = threading.Lock()


def _metrics_hook(entry: str):
    def hook(metrics: dict[str, Any]) -> None:
        path = os.environ.get("MOLT_DEMO_METRICS_PATH")
        if not path:
            return
        payload: dict[str, Any] = {
            "entry": entry,
            "ts_ms": int(time.time() * 1000),
        }
        for key, value in metrics.items():
            if isinstance(value, (int, float)):
                payload[key] = value
        line = json.dumps(payload, sort_keys=True)
        with _METRICS_LOCK:
            with open(path, "a", encoding="utf-8") as handle:
                handle.write(line + "\n")

    return hook


def _query_param(request: Any, key: str, default: str | None = None) -> str | None:
    params = getattr(request, "GET", None)
    if params is None:
        return default
    value = params.get(key, default)
    if value is None:
        return default
    return value


def _bad_request(detail: str) -> JsonResponse:
    return JsonResponse({"error": "InvalidInput", "detail": detail}, status=400)


def _parse_limit(raw: str | None) -> int:
    if raw is None:
        return 50
    try:
        value = int(raw)
    except (TypeError, ValueError):
        return 50
    return max(1, min(value, 500))


def _build_items_response(
    *,
    user_id: int,
    q: str | None,
    status: str | None,
    limit: int,
    cursor: str | None,
) -> dict[str, Any]:
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


def baseline_items(request: Any) -> JsonResponse:
    raw_user = _query_param(request, "user_id")
    if raw_user is None:
        return _bad_request("Missing required query param: user_id")
    try:
        user_id = int(raw_user)
    except (TypeError, ValueError):
        return _bad_request("user_id must be an integer")

    q = _query_param(request, "q")
    status = _query_param(request, "status")
    cursor = _query_param(request, "cursor")
    limit = _parse_limit(_query_param(request, "limit"))

    delay_ms = int(os.environ.get("MOLT_FAKE_DB_DELAY_MS", "0") or "0")
    if delay_ms > 0:
        time.sleep(delay_ms / 1000.0)

    payload = _build_items_response(
        user_id=user_id,
        q=q,
        status=status,
        limit=limit,
        cursor=cursor,
    )
    return JsonResponse(payload, status=200)


@molt_offload(
    entry="list_items",
    codec="msgpack",
    timeout_ms=250,
    metrics_hook=_metrics_hook("list_items"),
)
def offload_items(request: Any) -> JsonResponse:
    try:
        return baseline_items(request)
    except MoltAccelError as exc:
        return JsonResponse(
            {"error": exc.__class__.__name__, "detail": str(exc)}, status=500
        )


def compute_view(request: Any) -> JsonResponse:
    values_raw = _query_param(request, "values")
    if values_raw is None:
        values = []
    else:
        try:
            values = [float(x) for x in values_raw.split(",")]
        except Exception:
            values = []
    scale = float(_query_param(request, "scale", "1.0") or "1.0")
    offset = float(_query_param(request, "offset", "0.0") or "0.0")
    scaled = [(v * scale) + offset for v in values]
    return JsonResponse(
        {"count": len(scaled), "sum": sum(scaled), "scaled": scaled}, status=200
    )


def _build_compute_payload(request: Any) -> dict[str, Any]:
    values_raw = _query_param(request, "values")
    values = []
    if values_raw is not None:
        try:
            values = [float(x) for x in values_raw.split(",")]
        except Exception:
            values = []
    scale = float(_query_param(request, "scale", "1.0") or "1.0")
    offset = float(_query_param(request, "offset", "0.0") or "0.0")
    return {"values": values, "scale": scale, "offset": offset}


@molt_offload(
    entry="compute",
    codec="msgpack",
    timeout_ms=250,
    payload_builder=_build_compute_payload,
    metrics_hook=_metrics_hook("compute"),
)
def compute_offload_view(request: Any) -> JsonResponse:
    try:
        return compute_view(request)
    except MoltAccelError as exc:
        return JsonResponse(
            {"error": exc.__class__.__name__, "detail": str(exc)}, status=500
        )


def _build_table_payload(request: Any) -> dict[str, Any]:
    rows = int(_query_param(request, "rows", "10000") or "10000")
    return {"rows": rows}


@molt_offload(
    entry="offload_table",
    codec="json",
    timeout_ms=500,
    payload_builder=_build_table_payload,
    metrics_hook=_metrics_hook("offload_table"),
)
def offload_table(request: Any) -> JsonResponse:
    rows = int(_query_param(request, "rows", "10000") or "10000")
    data = [{"id": i, "value": i % 7} for i in range(rows)]
    return JsonResponse({"rows": rows, "data": data[:5]}, status=200)


@molt_offload(
    entry="health", codec="json", timeout_ms=100, metrics_hook=_metrics_hook("health")
)
def health_view(request: Any) -> JsonResponse:
    return JsonResponse({"ok": True}, status=200)
