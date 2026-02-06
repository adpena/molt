"""Minimal streaming and WebSocket surface for Molt."""

from __future__ import annotations

from collections.abc import AsyncIterable, AsyncIterator, Iterable
from dataclasses import dataclass, field
from typing import Any, cast

from molt import intrinsics as _intrinsics

Payload = object
_IO_EVENT_READ = 1
_IO_EVENT_WRITE = 1 << 1
_PENDING_SENTINEL: Any | None = None


molt_pending = _intrinsics.require("molt_pending", globals())
molt_async_sleep = _intrinsics.require("molt_async_sleep", globals())
molt_stream_new = _intrinsics.require("molt_stream_new", globals())
molt_stream_send_obj = _intrinsics.require("molt_stream_send_obj", globals())
molt_stream_recv = _intrinsics.require("molt_stream_recv", globals())
molt_stream_close = _intrinsics.require("molt_stream_close", globals())
molt_stream_drop = _intrinsics.require("molt_stream_drop", globals())
molt_ws_pair_obj = _intrinsics.require("molt_ws_pair_obj", globals())
molt_ws_connect_obj = _intrinsics.require("molt_ws_connect_obj", globals())
molt_ws_send_obj = _intrinsics.require("molt_ws_send_obj", globals())
molt_ws_recv = _intrinsics.require("molt_ws_recv", globals())
molt_ws_close = _intrinsics.require("molt_ws_close", globals())
molt_ws_drop = _intrinsics.require("molt_ws_drop", globals())
_MOLT_WS_WAIT_NEW = _intrinsics.load("molt_ws_wait_new", globals())


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is not None:
        return _PENDING_SENTINEL
    _PENDING_SENTINEL = molt_pending()
    return _PENDING_SENTINEL


def _is_pending(value: Any) -> bool:
    return value is _pending_sentinel()


def _stream_payload_bytes(payload: Payload) -> bytes:
    if isinstance(payload, str):
        return payload.encode("utf-8")
    if isinstance(payload, bytes):
        return payload
    raise TypeError("Stream payload must be bytes or str")


def _ws_payload_bytes(payload: Payload) -> bytes:
    if isinstance(payload, str):
        return payload.encode("utf-8")
    if isinstance(payload, bytes):
        return payload
    raise TypeError("WebSocket payload must be bytes or str")


class _RuntimeHandle:
    def __init__(self, handle: Any, drop_fn: Any) -> None:
        self._handle = handle
        self._drop_fn = drop_fn
        self._refs = 0
        self._dropped = False

    def acquire(self) -> "_RuntimeHandle":
        self._refs += 1
        return self

    def release(self) -> None:
        if self._refs <= 0:
            return
        self._refs -= 1
        if self._refs == 0:
            self._drop()

    def _drop(self) -> None:
        if self._dropped:
            return
        self._dropped = True
        try:
            self._drop_fn(self._handle)
        except Exception:
            return

    @property
    def handle(self) -> Any:
        return self._handle

    def __del__(self) -> None:
        if not self._dropped:
            self._drop()


class _SyncAsyncIter:
    def __init__(self, source: Iterable[Payload]) -> None:
        self._iter = iter(source)

    def __aiter__(self) -> AsyncIterator[Payload]:
        return self

    async def __anext__(self) -> Payload:
        try:
            return next(self._iter)
        except StopIteration:
            raise StopAsyncIteration


class _RuntimeStreamIter:
    def __init__(self, handle: _RuntimeHandle) -> None:
        self._handle = handle.acquire()
        self._released = False

    def _release(self) -> None:
        if self._released:
            return
        self._released = True
        self._handle.release()

    def __del__(self) -> None:
        self._release()

    def __aiter__(self) -> AsyncIterator[Payload]:
        return self

    async def __anext__(self) -> Payload:
        while True:
            res = molt_stream_recv(self._handle.handle)
            if _is_pending(res):
                await molt_async_sleep(0.0, None)
                continue
            if res is None:
                self._release()
                raise StopAsyncIteration
            return cast(Payload, res)


class Stream:
    def __init__(self, source: AsyncIterable[Payload] | Iterable[Payload]) -> None:
        self._source = source

    def __aiter__(self) -> AsyncIterator[Payload]:
        if isinstance(self._source, AsyncIterable):
            return self._source.__aiter__()
        source = cast(Iterable[Payload], self._source)
        return _SyncAsyncIter(source)


@dataclass(slots=True)
class Request:
    body: Stream
    method: str = "GET"
    path: str = "/"
    headers: dict[str, str] = field(default_factory=dict)


@dataclass(slots=True)
class Response:
    body: Stream | bytes | str | None = None
    status: int = 200
    headers: dict[str, str] = field(default_factory=dict)


class StreamSenderBase:
    async def send(self, payload: Payload) -> None:
        raise NotImplementedError

    async def close(self) -> None:
        raise NotImplementedError


class RuntimeStream(Stream):
    def __init__(self, handle: _RuntimeHandle) -> None:
        self._handle = handle.acquire()
        self._released = False

    def __aiter__(self) -> AsyncIterator[Payload]:
        return _RuntimeStreamIter(self._handle)

    def _release(self) -> None:
        if self._released:
            return
        self._released = True
        self._handle.release()

    def __del__(self) -> None:
        self._release()


class RuntimeStreamSender(StreamSenderBase):
    def __init__(self, handle: _RuntimeHandle) -> None:
        self._handle = handle.acquire()
        self._closed = False
        self._released = False

    def _release(self) -> None:
        if self._released:
            return
        self._released = True
        self._handle.release()

    def __del__(self) -> None:
        self._release()

    async def send(self, payload: Payload) -> None:
        if self._closed:
            raise RuntimeError("StreamSender is closed")
        data = _stream_payload_bytes(payload)
        while True:
            res = molt_stream_send_obj(self._handle.handle, data)
            if not _is_pending(res):
                return
            await molt_async_sleep(0.0, None)

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        molt_stream_close(self._handle.handle)
        self._release()


class WebSocket:
    def __init__(self, handle: Any) -> None:
        self._handle = _RuntimeHandle(handle, molt_ws_drop).acquire()
        self._closed = False
        self._released = False

    def _release(self) -> None:
        if self._released:
            return
        self._released = True
        self._handle.release()

    def __del__(self) -> None:
        self._release()

    async def send(self, msg: Payload) -> None:
        if self._closed:
            raise RuntimeError("WebSocket is closed")
        data = _ws_payload_bytes(msg)
        while True:
            res = molt_ws_send_obj(self._handle.handle, data)
            if not _is_pending(res):
                return
            if _MOLT_WS_WAIT_NEW is not None:
                wait_obj = _MOLT_WS_WAIT_NEW(self._handle.handle, _IO_EVENT_WRITE, None)
                if wait_obj is not None:
                    await wait_obj
                    continue
            await molt_async_sleep(0.0, None)

    async def recv(self) -> Payload:
        while True:
            res = molt_ws_recv(self._handle.handle)
            if _is_pending(res):
                if _MOLT_WS_WAIT_NEW is not None:
                    wait_obj = _MOLT_WS_WAIT_NEW(
                        self._handle.handle, _IO_EVENT_READ, None
                    )
                    if wait_obj is not None:
                        await wait_obj
                        continue
                await molt_async_sleep(0.0, None)
                continue
            if res is None:
                self._closed = True
                raise RuntimeError("WebSocket closed")
            if isinstance(res, (bytes, str)):
                return res
            raise TypeError("WebSocket payload must be bytes or str")

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        molt_ws_close(self._handle.handle)
        self._release()

    def __aiter__(self) -> AsyncIterator[Payload]:
        return _WebSocketIter(self)


class _WebSocketIter:
    def __init__(self, socket: WebSocket) -> None:
        self._socket = socket

    def __aiter__(self) -> AsyncIterator[Payload]:
        return self

    async def __anext__(self) -> Payload:
        try:
            return await self._socket.recv()
        except RuntimeError:
            raise StopAsyncIteration


class RuntimeWebSocket(WebSocket):
    pass


def stream(source: AsyncIterable[Payload] | Iterable[Payload]) -> Stream:
    return Stream(source)


def stream_channel(maxsize: int = 1) -> tuple[Stream, StreamSenderBase]:
    handle = molt_stream_new(maxsize)
    shared = _RuntimeHandle(handle, molt_stream_drop)
    return RuntimeStream(shared), RuntimeStreamSender(shared)


def ws_pair(maxsize: int = 0) -> tuple[WebSocket, WebSocket]:
    handles = molt_ws_pair_obj(maxsize)
    if not isinstance(handles, tuple) or len(handles) != 2:
        raise RuntimeError("molt_ws_pair_obj returned invalid handles")
    left_handle, right_handle = handles
    return WebSocket(left_handle), WebSocket(right_handle)


def ws_connect(url: str, capability: str = "websocket.connect") -> WebSocket:
    handle = molt_ws_connect_obj(url)
    return WebSocket(handle)
