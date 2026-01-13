from __future__ import annotations

import json
from typing import Any, Mapping

from molt_accel.errors import MoltInvalidInput


def _get_query_mapping(request: Any) -> Mapping[str, Any] | None:
    for attr in ("GET", "query_params", "args", "params"):
        mapping = getattr(request, attr, None)
        if mapping is not None:
            return mapping
    if isinstance(request, Mapping):
        return request
    return None


def _get_body_mapping(request: Any) -> Mapping[str, Any] | None:
    body = getattr(request, "body", None)
    if body is None:
        return None
    if isinstance(body, (bytes, bytearray)):
        if not body:
            return None
        try:
            payload = json.loads(body.decode("utf-8"))
        except Exception:
            return None
        return payload if isinstance(payload, Mapping) else None
    if isinstance(body, str):
        if not body:
            return None
        try:
            payload = json.loads(body)
        except Exception:
            return None
        return payload if isinstance(payload, Mapping) else None
    return None


def _get_payload_mapping(request: Any) -> Mapping[str, Any] | None:
    return _get_query_mapping(request) or _get_body_mapping(request)


def _coerce_int(value: Any, default: int) -> int:
    if value is None:
        return default
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _coerce_float(value: Any, default: float) -> float:
    if value is None:
        return default
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _parse_values(raw: Any) -> list[float]:
    if raw is None:
        return []
    if isinstance(raw, str):
        if not raw.strip():
            return []
        parts = raw.split(",")
    elif isinstance(raw, (list, tuple)):
        parts = raw
    else:
        return []
    try:
        return [float(value) for value in parts]
    except Exception:
        return []


def build_list_items_payload(request: Any) -> dict[str, Any]:
    params = _get_query_mapping(request)
    if params is None:
        raise MoltInvalidInput("Request does not expose query parameters")

    def _get(key: str, default: Any = None) -> Any:
        value = params.get(key, default)
        if value is None:
            return default
        return value

    raw_user = _get("user_id")
    if raw_user is None:
        raise MoltInvalidInput("Missing required query param: user_id")

    try:
        user_id = int(raw_user)
    except (TypeError, ValueError) as exc:
        raise MoltInvalidInput("user_id must be an integer") from exc

    raw_limit = _get("limit", 50)
    limit = _coerce_int(raw_limit, 50)

    payload: dict[str, Any] = {
        "user_id": user_id,
        "q": _get("q"),
        "status": _get("status"),
        "limit": max(1, min(limit, 500)),
        "cursor": _get("cursor"),
    }
    return payload


def build_compute_payload(request: Any) -> dict[str, Any]:
    params = _get_payload_mapping(request)
    if params is None:
        raise MoltInvalidInput("Request does not expose payload parameters")

    raw_values = params.get("values")
    values = _parse_values(raw_values)
    scale = _coerce_float(params.get("scale", 1.0), 1.0)
    offset = _coerce_float(params.get("offset", 0.0), 0.0)
    return {"values": values, "scale": scale, "offset": offset}


def build_offload_table_payload(request: Any) -> dict[str, Any]:
    params = _get_payload_mapping(request)
    if params is None:
        rows = 10_000
    else:
        rows = _coerce_int(params.get("rows", 10_000), 10_000)
    rows = max(1, min(rows, 50_000))
    return {"rows": rows}
