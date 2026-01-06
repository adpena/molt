from __future__ import annotations

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

    payload: dict[str, Any] = {
        "user_id": user_id,
        "q": _get("q"),
        "status": _get("status"),
        "limit": int(_get("limit", 50)),
        "cursor": _get("cursor"),
    }
    return payload
