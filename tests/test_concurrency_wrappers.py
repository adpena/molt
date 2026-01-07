import asyncio

import pytest

from molt import channel, spawn


def test_channel_send_recv():
    chan = channel()
    chan.send(123)
    assert chan.recv() == 123


def test_spawn_sends_to_channel():
    chan = channel()

    async def worker() -> None:
        chan.send(7)

    spawn(worker())
    assert chan.recv() == 7


def test_channel_async_send_recv():
    async def run() -> int:
        chan = channel()
        await chan.send_async(5)
        return await chan.recv_async()

    assert asyncio.run(run()) == 5


def test_channel_async_backpressure():
    async def run() -> tuple[int, int]:
        chan = channel(1)
        first_sent = asyncio.Event()
        second_sent = asyncio.Event()
        allow_recv = asyncio.Event()

        async def producer() -> None:
            await chan.send_async(1)
            first_sent.set()
            await chan.send_async(2)
            second_sent.set()

        async def consumer() -> tuple[int, int]:
            await allow_recv.wait()
            first = await chan.recv_async()
            second = await chan.recv_async()
            return first, second

        prod_task = asyncio.create_task(producer())
        cons_task = asyncio.create_task(consumer())

        await asyncio.wait_for(first_sent.wait(), timeout=0.2)
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(second_sent.wait(), timeout=0.05)

        allow_recv.set()
        items = await asyncio.wait_for(cons_task, timeout=0.2)
        await asyncio.wait_for(second_sent.wait(), timeout=0.2)
        await asyncio.wait_for(prod_task, timeout=0.2)
        return items

    assert asyncio.run(run()) == (1, 2)
