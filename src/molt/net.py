"""Minimal streaming and WebSocket surface for Molt.

This provides a CPython fallback for tests and local tooling while the native
runtime bindings are still in progress.
"""

from __future__ import annotations

import asyncio
import ctypes
import importlib
from collections.abc import AsyncIterable, AsyncIterator, Iterable
from dataclasses import dataclass, field
from typing import Any, cast

from molt import capabilities
from molt.concurrency import channel

Payload = object
_SHIMS: Any | None = None


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


def _decode_molt_payload(value: int) -> Payload | None:
    from molt_json import _decode_molt_object

    return _decode_molt_object(value)


class _AsyncQueue:
    def __init__(self, maxsize: int = 0) -> None:
        self._chan = channel(maxsize)

    async def put(self, item: Payload | None) -> None:
        await self._chan.send_async(item)

    async def get(self) -> Payload | None:
        return await self._chan.recv_async()


def _get_shims() -> Any:
    global _SHIMS
    if _SHIMS is None:
        _SHIMS = importlib.import_module("molt.shims")
    return _SHIMS


class _RuntimeHandle:
    def __init__(self, handle: ctypes.c_void_p, lib: Any, drop_name: str) -> None:
        self._handle = handle
        self._lib = lib
        self._drop_name = drop_name
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
        drop_fn = getattr(self._lib, self._drop_name)
        drop_fn(self._handle)

    @property
    def handle(self) -> ctypes.c_void_p:
        return self._handle

    @property
    def lib(self) -> Any:
        return self._lib

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
        pending = _pending_bits()
        while True:
            res = int(self._handle.lib.molt_stream_recv(self._handle.handle))
            if res == pending:
                await asyncio.sleep(0)
            else:
                obj = _decode_molt_payload(res)
                if obj is None:
                    self._release()
                    raise StopAsyncIteration
                return obj


class _QueueStreamIter:
    def __init__(self, queue: "_AsyncQueue") -> None:
        self._queue = queue

    def __aiter__(self) -> AsyncIterator[Payload]:
        return self

    async def __anext__(self) -> Payload:
        item = await self._queue.get()
        if item is None:
            raise StopAsyncIteration
        return item


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


class WebSocket:
    def __init__(self, maxsize: int = 0) -> None:
        self._incoming = _AsyncQueue(maxsize)
        self._outgoing = _AsyncQueue(maxsize)
        self._closed = False

    async def send(self, msg: Payload) -> None:
        if self._closed:
            raise RuntimeError("WebSocket is closed")
        await self._outgoing.put(msg)

    async def recv(self) -> Payload:
        msg = await self._incoming.get()
        if msg is None:
            self._closed = True
            raise RuntimeError("WebSocket closed")
        return msg

    async def close(self) -> None:
        if not self._closed:
            self._closed = True
            await self._incoming.put(None)
            await self._outgoing.put(None)

    def __aiter__(self) -> AsyncIterator[Payload]:
        return _WebSocketIter(self)


def stream(source: AsyncIterable[Payload] | Iterable[Payload]) -> Stream:
    return Stream(source)


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


class StreamSenderBase:
    async def send(self, payload: Payload) -> None:
        raise NotImplementedError

    async def close(self) -> None:
        raise NotImplementedError


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
        pending = _pending_bits()
        data = _stream_payload_bytes(payload)
        while True:
            res = int(
                self._handle.lib.molt_stream_send(self._handle.handle, data, len(data))
            )
            if res != pending:
                return
            await asyncio.sleep(0)

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._handle.lib.molt_stream_close(self._handle.handle)
        self._release()


class StreamSender(StreamSenderBase):
    def __init__(self, queue: "_AsyncQueue") -> None:
        self._queue = queue

    async def send(self, payload: Payload) -> None:
        await self._queue.put(payload)

    async def close(self) -> None:
        await self._queue.put(None)


def stream_channel(maxsize: int = 1) -> tuple[Stream, StreamSenderBase]:
    shims = _get_shims()
    lib = shims.load_runtime()
    handle = shims.stream_new_handle(lib, maxsize)
    if handle is not None:
        shared = _RuntimeHandle(handle, lib, "molt_stream_drop")
        return RuntimeStream(shared), RuntimeStreamSender(shared)

    queue = _AsyncQueue(maxsize)

    return Stream(_QueueStreamIter(queue)), StreamSender(queue)


class RuntimeWebSocket(WebSocket):
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

    async def send(self, msg: Payload) -> None:
        pending = _pending_bits()
        data = _ws_payload_bytes(msg)
        while True:
            res = int(
                self._handle.lib.molt_ws_send(self._handle.handle, data, len(data))
            )
            if res != pending:
                return
            await asyncio.sleep(0)

    async def recv(self) -> Payload:
        pending = _pending_bits()
        while True:
            res = int(self._handle.lib.molt_ws_recv(self._handle.handle))
            if res == pending:
                await asyncio.sleep(0)
                continue
            obj = _decode_molt_payload(res)
            if obj is None:
                self._closed = True
                raise RuntimeError("WebSocket closed")
            if isinstance(obj, (bytes, str)):
                return obj
            raise TypeError("WebSocket payload must be bytes or str")

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._handle.lib.molt_ws_close(self._handle.handle)
        self._release()


def ws_pair(maxsize: int = 0) -> tuple[WebSocket, WebSocket]:
    shims = _get_shims()
    lib = shims.load_runtime()
    handles = shims.ws_pair_handles(lib, maxsize)
    if handles is not None:
        left_handle, right_handle = handles
        left_shared = _RuntimeHandle(left_handle, lib, "molt_ws_drop")
        right_shared = _RuntimeHandle(right_handle, lib, "molt_ws_drop")
        return RuntimeWebSocket(left_shared), RuntimeWebSocket(right_shared)
    left = WebSocket(maxsize)
    right = WebSocket(maxsize)
    left._incoming = right._outgoing
    right._incoming = left._outgoing
    return left, right


def ws_connect(url: str, capability: str = "websocket.connect") -> WebSocket:
    capabilities.require(capability)
    shims = _get_shims()
    lib = shims.load_runtime()
    if lib is None:
        raise RuntimeError("WebSocket runtime not available")
    handle = shims.ws_connect_handle(lib, url)
    if handle is None:
        raise RuntimeError("WebSocket connect failed")
    shared = _RuntimeHandle(handle, lib, "molt_ws_drop")
    return RuntimeWebSocket(shared)


def _pending_bits() -> int:
    shims = _get_shims()
    return shims._PENDING
