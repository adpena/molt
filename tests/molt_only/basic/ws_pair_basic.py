import asyncio

from molt import net


async def main() -> None:
    left, right = net.ws_pair()
    await left.send(b"ping")
    msg = await right.recv()
    assert msg == b"ping"
    await right.send(b"pong")
    msg = await left.recv()
    assert msg == b"pong"


asyncio.run(main())
