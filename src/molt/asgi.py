"""ASGI adapter for Molt handlers (CPython shim only)."""

from __future__ import annotations

from collections.abc import AsyncIterable, Awaitable, Callable
from typing import Any

from molt import capabilities
from molt.net import Request, Response, Stream

Scope = dict[str, Any]
Receive = Callable[[], Awaitable[dict[str, Any]]]
Send = Callable[[dict[str, Any]], Awaitable[None]]


def _payload_bytes(value: Any) -> bytes:
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, str):
        return value.encode("utf-8")
    raise TypeError("Response payload must be bytes or str")


async def _read_body(receive: Receive) -> bytes:
    chunks: list[bytes] = []
    while True:
        event = await receive()
        if event.get("type") != "http.request":
            if event.get("type") == "http.disconnect":
                break
            continue
        chunk = event.get("body", b"")
        if chunk:
            if not isinstance(chunk, (bytes, bytearray)):
                raise TypeError("ASGI body chunk must be bytes")
            chunks.append(bytes(chunk))
        if not event.get("more_body", False):
            break
    return b"".join(chunks)


def _scope_headers(scope: Scope) -> dict[str, str]:
    headers: dict[str, str] = {}
    for key, value in scope.get("headers", []):
        if isinstance(key, (bytes, bytearray)):
            key_str = bytes(key).decode("latin-1").lower()
        else:
            key_str = str(key).lower()
        if isinstance(value, (bytes, bytearray)):
            val_str = bytes(value).decode("latin-1")
        else:
            val_str = str(value)
        headers[key_str] = val_str
    return headers


async def _iter_body(body: Stream | AsyncIterable[Any] | bytes | str | None):
    if body is None:
        return
    if isinstance(body, (bytes, bytearray, str)):
        yield _payload_bytes(body)
        return
    if isinstance(body, Stream):
        async for chunk in body:
            yield _payload_bytes(chunk)
        return
    async for chunk in body:
        yield _payload_bytes(chunk)


def asgi_adapter(
    handler: Callable[[Request], Any],
) -> Callable[[Scope, Receive, Send], Awaitable[None]]:
    """Adapt a Molt handler into an ASGI app (http + lifespan)."""

    async def app(scope: Scope, receive: Receive, send: Send) -> None:
        scope_type = scope.get("type")
        if scope_type == "lifespan":
            while True:
                event = await receive()
                if event.get("type") == "lifespan.startup":
                    await send({"type": "lifespan.startup.complete"})
                elif event.get("type") == "lifespan.shutdown":
                    await send({"type": "lifespan.shutdown.complete"})
                    return
            return
        if scope_type != "http":
            raise RuntimeError(f"Unsupported ASGI scope '{scope_type}'")

        capabilities.require("net")
        body_bytes = await _read_body(receive)
        body_stream = Stream([body_bytes]) if body_bytes else Stream([])
        request = Request(
            body=body_stream,
            method=scope.get("method", "GET"),
            path=scope.get("path", "/"),
            headers=_scope_headers(scope),
        )
        result = handler(request)
        if hasattr(result, "__await__"):
            result = await result
        if not isinstance(result, Response):
            raise TypeError("Handler must return a molt.net.Response")

        headers = [
            (k.encode("latin-1"), v.encode("latin-1"))
            for k, v in result.headers.items()
        ]
        await send(
            {
                "type": "http.response.start",
                "status": int(result.status),
                "headers": headers,
            }
        )
        async for chunk in _iter_body(result.body):
            await send({"type": "http.response.body", "body": chunk, "more_body": True})
        await send({"type": "http.response.body", "body": b"", "more_body": False})

    return app
