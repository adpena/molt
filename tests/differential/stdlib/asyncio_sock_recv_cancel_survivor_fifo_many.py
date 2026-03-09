# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: cancelling a mid-queue recv waiter must preserve FIFO for survivors."""

import asyncio
import socket
import sys


async def main() -> None:
    if sys.platform == "win32":
        print("unsupported")
        return
    if not hasattr(socket, "socketpair"):
        print("unsupported")
        return

    loop = asyncio.get_running_loop()
    left, right = socket.socketpair()
    left.setblocking(False)
    right.setblocking(False)
    try:
        first = asyncio.create_task(loop.sock_recv(right, 1))
        cancelled_task = asyncio.create_task(loop.sock_recv(right, 1))
        third = asyncio.create_task(loop.sock_recv(right, 1))
        fourth = asyncio.create_task(loop.sock_recv(right, 1))
        await asyncio.sleep(0)

        cancelled_task.cancel()
        try:
            await cancelled_task
        except asyncio.CancelledError:
            cancelled = True
        else:
            cancelled = False

        await loop.sock_sendall(left, b"acd")
        survivors = await asyncio.wait_for(
            asyncio.gather(first, third, fourth),
            1.0,
        )

        print("cancelled", cancelled, "survivors", survivors)
        assert cancelled is True
        assert survivors == [b"a", b"c", b"d"], survivors
    finally:
        left.close()
        right.close()


asyncio.run(main())
