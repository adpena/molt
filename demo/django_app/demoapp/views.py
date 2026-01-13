from __future__ import annotations

import json
import os
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any

from django.http import JsonResponse

from molt_accel import (
    MoltAccelError,
    MoltInvalidInput,
    molt_offload,
    raw_json_response_factory,
)
from molt_accel.contracts import (
    build_compute_payload,
    build_list_items_payload,
    build_offload_table_payload,
)


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


def _bad_request(detail: str) -> JsonResponse:
    return JsonResponse({"error": "InvalidInput", "detail": detail}, status=400)


def _env_int(name: str, default: int = 0) -> int:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        value = int(raw)
    except (TypeError, ValueError):
        return default
    return max(0, value)


def _burn_cpu(iters: int, seed: int) -> None:
    if iters <= 0:
        return
    acc = seed & 0xFFFFFFFF
    for idx in range(iters):
        acc = (acc * 1664525 + 1013904223 + idx) & 0xFFFFFFFF
    _ = acc


def _sleep_fake_db(rows: int) -> None:
    delay_ms = _env_int("MOLT_FAKE_DB_DELAY_MS", 0)
    decode_us = _env_int("MOLT_FAKE_DB_DECODE_US_PER_ROW", 0)
    if delay_ms > 0:
        time.sleep(delay_ms / 1000.0)
    if decode_us > 0 and rows > 0:
        time.sleep((decode_us * rows) / 1_000_000.0)


def _db_path() -> Path | None:
    raw = os.environ.get("MOLT_DEMO_DB_PATH")
    if raw is None or raw == "":
        return None
    return Path(raw)


def _sqlite_connect(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(path), check_same_thread=False)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA query_only = ON")
    conn.execute("PRAGMA busy_timeout = 100")
    return conn


def _fetch_items_sqlite(
    *,
    path: Path,
    user_id: int,
    q: str | None,
    status: str | None,
    limit: int,
) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(str(path))
    sql = (
        "SELECT id, created_at, status, title, score, unread "
        "FROM items WHERE user_id = ?"
    )
    params: list[Any] = [user_id]
    if status:
        sql += " AND status = ?"
        params.append(status)
    if q:
        sql += " AND title LIKE ?"
        params.append(f"%{q}%")
    sql += " ORDER BY id ASC LIMIT ?"
    params.append(limit)
    items: list[dict[str, Any]] = []
    open_count = 0
    closed_count = 0
    with _sqlite_connect(path) as conn:
        for row in conn.execute(sql, params):
            status_value = row["status"]
            if status_value == "open":
                open_count += 1
            elif status_value == "closed":
                closed_count += 1
            items.append(
                {
                    "id": row["id"],
                    "created_at": row["created_at"],
                    "status": status_value,
                    "title": row["title"],
                    "score": row["score"],
                    "unread": bool(row["unread"]),
                }
            )

    next_cursor = f"{user_id}:{limit}" if len(items) == limit else None
    return {
        "items": items,
        "next_cursor": next_cursor,
        "counts": {"open": open_count, "closed": closed_count},
    }


def _build_items_response(
    *,
    user_id: int,
    q: str | None,
    status: str | None,
    limit: int,
    cursor: str | None,
    cpu_iters: int,
) -> dict[str, Any]:
    q_len = len(q or "")
    status_len = len(status or "")
    cursor_len = len(cursor or "")
    base = abs(user_id) * 1000 + q_len + status_len + cursor_len
    items: list[dict[str, Any]] = []
    open_count = 0
    closed_count = 0
    for idx in range(limit):
        if cpu_iters > 0:
            _burn_cpu(cpu_iters, base + idx)
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
    try:
        payload = build_list_items_payload(request)
    except MoltInvalidInput as exc:
        return _bad_request(str(exc))

    user_id = payload["user_id"]
    q = payload.get("q")
    status = payload.get("status")
    cursor = payload.get("cursor")
    limit = payload.get("limit", 50)
    cpu_iters = _env_int("MOLT_FAKE_DB_CPU_ITERS", 0)
    db_path = _db_path()

    if db_path is not None:
        try:
            payload = _fetch_items_sqlite(
                path=db_path,
                user_id=user_id,
                q=q,
                status=status,
                limit=limit,
            )
            return JsonResponse(payload, status=200)
        except FileNotFoundError:
            return JsonResponse(
                {
                    "error": "DbUnavailable",
                    "detail": "Demo DB not found; run demoapp.db_seed",
                },
                status=503,
            )
        except sqlite3.Error as exc:
            return JsonResponse(
                {"error": "InternalError", "detail": str(exc)}, status=500
            )

    _sleep_fake_db(limit)

    payload = _build_items_response(
        user_id=user_id,
        q=q,
        status=status,
        limit=limit,
        cursor=cursor,
        cpu_iters=cpu_iters,
    )
    return JsonResponse(payload, status=200)


@molt_offload(
    entry="list_items",
    codec="msgpack",
    timeout_ms=250,
    metrics_hook=_metrics_hook("list_items"),
    decode_response=False,
    response_factory=raw_json_response_factory,
)
def offload_items(request: Any) -> JsonResponse:
    try:
        return baseline_items(request)
    except MoltAccelError as exc:
        return JsonResponse(
            {"error": exc.__class__.__name__, "detail": str(exc)}, status=500
        )


def compute_view(request: Any) -> JsonResponse:
    try:
        payload = build_compute_payload(request)
    except MoltInvalidInput as exc:
        return _bad_request(str(exc))
    values = payload["values"]
    scale = payload["scale"]
    offset = payload["offset"]
    scaled = [(v * scale) + offset for v in values]
    return JsonResponse(
        {"count": len(scaled), "sum": sum(scaled), "scaled": scaled}, status=200
    )


@molt_offload(
    entry="compute",
    codec="msgpack",
    timeout_ms=250,
    payload_builder=build_compute_payload,
    metrics_hook=_metrics_hook("compute"),
)
def compute_offload_view(request: Any) -> JsonResponse:
    try:
        return compute_view(request)
    except MoltAccelError as exc:
        return JsonResponse(
            {"error": exc.__class__.__name__, "detail": str(exc)}, status=500
        )


@molt_offload(
    entry="offload_table",
    codec="json",
    timeout_ms=500,
    payload_builder=build_offload_table_payload,
    metrics_hook=_metrics_hook("offload_table"),
)
def offload_table(request: Any) -> JsonResponse:
    rows = build_offload_table_payload(request)["rows"]
    cpu_iters = _env_int("MOLT_FAKE_DB_CPU_ITERS", 0)
    _sleep_fake_db(rows)
    burn_rows = min(rows, 5000)
    data = []
    for i in range(rows):
        if cpu_iters > 0 and i < burn_rows:
            _burn_cpu(cpu_iters, i)
        data.append({"id": i, "value": i % 7})
    return JsonResponse({"rows": rows, "sample": data[:5]}, status=200)


@molt_offload(
    entry="health", codec="json", timeout_ms=100, metrics_hook=_metrics_hook("health")
)
def health_view(request: Any) -> JsonResponse:
    return JsonResponse({"ok": True}, status=200)
