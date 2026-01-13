from __future__ import annotations

import os
import shutil
import threading
from typing import Any, Callable

from importlib.resources import files
from molt_accel.client import CancelCheck, Hook, MoltClient, MoltClientPool
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
ClientMode = str


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


def raw_json_response_factory(payload: Any, status: int) -> Any:
    try:
        from django.http import HttpResponse, JsonResponse  # type: ignore

        if isinstance(payload, (bytes, bytearray)):
            return HttpResponse(
                payload,
                status=status,
                content_type="application/json",
            )
        return JsonResponse(payload, status=status, safe=isinstance(payload, dict))
    except Exception:
        return {"status": status, "payload": payload}


_SHARED_CLIENT: MoltClient | MoltClientPool | None = None
_SHARED_CLIENT_LOCK = threading.Lock()


def _pool_size_from_env() -> int:
    raw = os.environ.get("MOLT_ACCEL_POOL_SIZE")
    if raw is None or raw == "":
        return 1
    try:
        value = int(raw)
    except (TypeError, ValueError):
        return 1
    return max(1, value)


def _resolve_worker_cmd() -> tuple[list[str], str | None]:
    cmd = os.environ.get("MOLT_WORKER_CMD")
    wire = os.environ.get("MOLT_WORKER_WIRE") or os.environ.get("MOLT_WIRE")
    if cmd:
        return cmd.split(), wire

    # Fallback: try to locate a `molt-worker` binary in PATH and use the packaged demo exports manifest.
    worker_bin = shutil.which("molt-worker") or shutil.which("molt_worker")
    if worker_bin:
        try:
            exports = files("molt_accel").joinpath("default_exports.json")
            return [worker_bin, "--stdio", "--exports", str(exports)], wire
        except Exception as exc:  # pragma: no cover - defensive
            raise MoltWorkerUnavailable(
                "Failed to locate default exports manifest"
            ) from exc

    raise MoltWorkerUnavailable(
        "MOLT_WORKER_CMD is not set and molt-worker was not found in PATH"
    )


def _build_client() -> MoltClient:
    cmd, wire = _resolve_worker_cmd()
    return MoltClient(worker_cmd=cmd, wire=wire)


def _build_client_pool(pool_size: int) -> MoltClientPool:
    cmd, wire = _resolve_worker_cmd()
    return MoltClientPool(worker_cmd=cmd, wire=wire, pool_size=pool_size)


def _resolve_client(
    client: MoltClient | MoltClientPool | None, client_mode: ClientMode | None
) -> tuple[MoltClient | MoltClientPool, bool]:
    if client is not None:
        return client, False

    mode = (client_mode or os.environ.get("MOLT_ACCEL_CLIENT_MODE", "shared")).lower()
    if mode not in {"shared", "per_request"}:
        mode = "shared"

    if mode == "shared":
        global _SHARED_CLIENT
        with _SHARED_CLIENT_LOCK:
            if _SHARED_CLIENT is None:
                pool_size = _pool_size_from_env()
                if pool_size > 1:
                    _SHARED_CLIENT = _build_client_pool(pool_size)
                else:
                    _SHARED_CLIENT = _build_client()
            return _SHARED_CLIENT, False

    return _build_client(), True


def _auto_cancel_check(request: Any) -> CancelCheck | None:
    if request is None:
        return None
    for attr in ("is_aborted", "is_disconnected"):
        checker = getattr(request, attr, None)
        if callable(checker):

            def _poll_cancel(checker: Callable[[], bool] = checker) -> bool:
                try:
                    return bool(checker())
                except Exception:
                    return False

            return _poll_cancel
    return None


def molt_offload(
    *,
    entry: str,
    codec: str = "msgpack",
    timeout_ms: int = 250,
    client: MoltClient | None = None,
    client_mode: ClientMode | None = None,
    payload_builder: Callable[..., Any] | None = None,
    response_factory: ResponseFactory | None = None,
    allow_fallback: bool = False,
    idempotent: bool = False,
    decode_response: bool = True,
    before_send: Hook | None = None,
    after_recv: Hook | None = None,
    metrics_hook: Hook | None = None,
    cancel_check: Callable[[Any], bool] | None = None,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Decorate a handler to offload the core work to a Molt worker."""

    def decorator(func: Callable[..., Any]) -> Callable[..., Any]:
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            request = args[0] if args else None
            build_payload = payload_builder or build_list_items_payload
            active_client: MoltClient | MoltClientPool
            close_after = False
            build_response = response_factory or _default_response_factory
            poll_cancel: CancelCheck | None = None
            if cancel_check is not None:

                def _poll_cancel() -> bool:
                    return cancel_check(request)

                poll_cancel = _poll_cancel
            else:
                poll_cancel = _auto_cancel_check(request)
            try:
                active_client, close_after = _resolve_client(client, client_mode)
                payload = build_payload(request, *args[1:], **kwargs)
                result = active_client.call(
                    entry=entry,
                    payload=payload,
                    codec=codec,
                    timeout_ms=timeout_ms,
                    idempotent=idempotent,
                    decode_response=decode_response,
                    before_send=before_send,
                    after_recv=after_recv,
                    metrics_hook=metrics_hook,
                    cancel_check=poll_cancel,
                )
                return build_response(result, 200)
            except MoltAccelError as exc:
                if allow_fallback:
                    return func(*args, **kwargs)
                status = _status_for_error(exc)
                payload = {"error": exc.__class__.__name__, "detail": str(exc)}
                return build_response(payload, status)
            finally:
                if close_after and active_client is not None:
                    active_client.close()

        return wrapper

    return decorator
