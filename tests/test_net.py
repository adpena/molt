import asyncio

import pytest

from molt import net
from molt import intrinsics as _intrinsics


def _has_intrinsic(name: str) -> bool:
    try:
        return _intrinsics.load(name) is not None
    except Exception:
        return False


def _require_intrinsics(names: tuple[str, ...]) -> None:
    missing = [name for name in names if not _has_intrinsic(name)]
    if missing:
        pytest.skip("Missing Molt intrinsics: " + ", ".join(missing))


def test_stream_channel_backpressure():
    _require_intrinsics(
        (
            "molt_stream_new",
            "molt_stream_send_obj",
            "molt_stream_recv",
            "molt_stream_close",
            "molt_stream_drop",
            "molt_async_sleep",
            "molt_pending",
        )
    )

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
    _require_intrinsics(
        (
            "molt_ws_pair_obj",
            "molt_ws_send_obj",
            "molt_ws_recv",
            "molt_ws_close",
            "molt_ws_drop",
            "molt_async_sleep",
            "molt_pending",
        )
    )

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
    pytest.skip("runtime connect hooks are no longer exposed via CPython shims")
