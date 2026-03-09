# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: cancelling one sock_recv waiter must not poison later recv readiness."""

import asyncio
import socket


async def main() -> None:
    if not hasattr(socket, "socketpair"):
        print("unsupported")
        return

    loop = asyncio.get_running_loop()
    left, right = socket.socketpair()
    left.setblocking(False)
    right.setblocking(False)
    try:
        cancelled_task = asyncio.create_task(loop.sock_recv(right, 1))
        await asyncio.sleep(0)
        cancelled_task.cancel()
        try:
            await cancelled_task
        except asyncio.CancelledError:
            cancelled = True
        else:
            cancelled = False

        survivor = asyncio.create_task(loop.sock_recv(right, 1))
        await asyncio.sleep(0)
        await loop.sock_sendall(left, b"x")
        follow_up = await asyncio.wait_for(survivor, 1.0)

        print("cancelled", cancelled, "follow_up", follow_up)
        assert cancelled is True
        assert follow_up == b"x", follow_up
    finally:
        left.close()
        right.close()


asyncio.run(main())
