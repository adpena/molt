import asyncio

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
