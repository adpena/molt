from __future__ import annotations

import os
from typing import Any, Callable

from molt_accel.client import MoltClient
from molt_accel.contracts import build_list_items_payload
from molt_accel.errors import (
    MoltAccelError,
    MoltBusy,
    MoltCancelled,
    MoltInternalError,
    MoltInvalidInput,
    MoltTimeout,
    MoltWorkerUnavailable,
)

ResponseFactory = Callable[[Any, int], Any]


def _default_response_factory(payload: Any, status: int) -> Any:
    try:
        from django.http import JsonResponse  # type: ignore

        return JsonResponse(payload, status=status, safe=isinstance(payload, dict))
    except Exception:
        return {"status": status, "payload": payload}


def _status_for_error(error: MoltAccelError) -> int:
    if isinstance(error, MoltInvalidInput):
        return 400
    if isinstance(error, MoltBusy):
        return 429
    if isinstance(error, MoltTimeout):
        return 504
    if isinstance(error, MoltWorkerUnavailable):
        return 503
    if isinstance(error, MoltCancelled):
        return 499
    if isinstance(error, MoltInternalError):
        return 500
    return 500


def _default_client() -> MoltClient:
    cmd = os.environ.get("MOLT_WORKER_CMD")
    if not cmd:
        raise MoltWorkerUnavailable("MOLT_WORKER_CMD is not set")
    return MoltClient(worker_cmd=cmd.split())


def molt_offload(
    *,
    entry: str,
    codec: str = "msgpack",
    timeout_ms: int = 250,
    client: MoltClient | None = None,
    payload_builder: Callable[..., Any] | None = None,
    response_factory: ResponseFactory | None = None,
    allow_fallback: bool = False,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Decorate a handler to offload the core work to a Molt worker."""

    def decorator(func: Callable[..., Any]) -> Callable[..., Any]:
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            request = args[0] if args else None
            build_payload = payload_builder or build_list_items_payload
            active_client = client or _default_client()
            build_response = response_factory or _default_response_factory
            try:
                payload = build_payload(request, *args[1:], **kwargs)
                result = active_client.call(
                    entry=entry,
                    payload=payload,
                    codec=codec,
                    timeout_ms=timeout_ms,
                )
                return build_response(result, 200)
            except MoltAccelError as exc:
                if allow_fallback:
                    return func(*args, **kwargs)
                status = _status_for_error(exc)
                payload = {"error": exc.__class__.__name__, "detail": str(exc)}
                return build_response(payload, status)

        return wrapper

    return decorator
