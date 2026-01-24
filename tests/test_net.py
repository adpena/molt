import asyncio
import ctypes
import os

import pytest

from molt import net
from molt import shims


def test_stream_channel_backpressure():
    async def run() -> list[bytes]:
        stream, sender = net.stream_channel(maxsize=1)
        first_sent = asyncio.Event()
        second_sent = asyncio.Event()

        async def producer() -> None:
            await sender.send(b"a")
            first_sent.set()
            await sender.send(b"b")
            second_sent.set()
            await sender.close()

        asyncio.create_task(producer())
        await asyncio.wait_for(first_sent.wait(), timeout=0.1)
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(second_sent.wait(), timeout=0.05)

        it = stream.__aiter__()
        item1 = await asyncio.wait_for(it.__anext__(), timeout=0.1)
        await asyncio.wait_for(second_sent.wait(), timeout=0.1)
        item2 = await asyncio.wait_for(it.__anext__(), timeout=0.1)

        with pytest.raises(StopAsyncIteration):
            await asyncio.wait_for(it.__anext__(), timeout=0.1)

        return [item1, item2]

    items = asyncio.run(run())
    assert items == [b"a", b"b"]


def test_websocket_backpressure():
    async def run() -> list[bytes]:
        left, right = net.ws_pair(maxsize=1)
        first_sent = asyncio.Event()
        second_sent = asyncio.Event()

        async def producer() -> None:
            await left.send(b"one")
            first_sent.set()
            await left.send(b"two")
            second_sent.set()
            await left.close()

        asyncio.create_task(producer())
        await asyncio.wait_for(first_sent.wait(), timeout=0.1)
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(second_sent.wait(), timeout=0.05)

        item1 = await asyncio.wait_for(right.recv(), timeout=0.1)
        await asyncio.wait_for(second_sent.wait(), timeout=0.1)
        item2 = await asyncio.wait_for(right.recv(), timeout=0.1)

        return [item1, item2]

    items = asyncio.run(run())
    assert items == [b"one", b"two"]


def test_ws_connect_hook_runtime():
    lib = shims.load_runtime()
    if lib is None:
        pytest.skip("runtime library not available")
    if not all(
        hasattr(lib, name)
        for name in ("molt_ws_set_connect_hook", "molt_ws_pair", "molt_ws_connect")
    ):
        pytest.skip("runtime ws connect hook not available")

    peer_handle: dict[str, ctypes.c_void_p] = {}
    seen_url: dict[str, str] = {}

    def _hook(url_ptr: ctypes.c_void_p, url_len: int) -> int:
        url = ctypes.string_at(url_ptr, url_len).decode("utf-8")
        seen_url["url"] = url
        left = ctypes.c_void_p()
        right = ctypes.c_void_p()
        rc = lib.molt_ws_pair(1, ctypes.byref(left), ctypes.byref(right))
        if rc != 0:
            return 0
        peer_handle["peer"] = right
        return left.value or 0

    hook_type = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t)
    hook_fn = hook_type(_hook)
    original_caps = os.environ.get("MOLT_CAPABILITIES")

    try:
        os.environ["MOLT_CAPABILITIES"] = "websocket.connect"
        lib.molt_ws_set_connect_hook(ctypes.cast(hook_fn, ctypes.c_void_p).value)
        ws = net.ws_connect("wss://example.test/echo")
        peer = net.RuntimeWebSocket(peer_handle["peer"], lib)

        async def run() -> list[bytes]:
            await ws.send(b"ping")
            first = await peer.recv()
            await peer.send(b"pong")
            second = await ws.recv()
            await ws.close()
            await peer.close()
            return [first, second]

        items = asyncio.run(run())
        assert seen_url["url"] == "wss://example.test/echo"
        assert items == [b"ping", b"pong"]
    finally:
        lib.molt_ws_set_connect_hook(0)
        if original_caps is None:
            os.environ.pop("MOLT_CAPABILITIES", None)
        else:
            os.environ["MOLT_CAPABILITIES"] = original_caps
