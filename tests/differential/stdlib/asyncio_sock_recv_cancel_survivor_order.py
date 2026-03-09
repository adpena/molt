# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: surviving sock_recv waiters preserve FIFO order after cancellation."""

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
        first = asyncio.create_task(loop.sock_recv(right, 1))
        cancelled_task = asyncio.create_task(loop.sock_recv(right, 1))
        third = asyncio.create_task(loop.sock_recv(right, 1))
        await asyncio.sleep(0)

        cancelled_task.cancel()
        try:
            await cancelled_task
        except asyncio.CancelledError:
            cancelled = True
        else:
            cancelled = False

        await loop.sock_sendall(left, b"ab")
        after_cancel = await asyncio.wait_for(
            asyncio.gather(first, third),
            1.0,
        )

        print("cancelled", cancelled, "after_cancel", after_cancel)
        assert cancelled is True
        assert after_cancel == [b"a", b"b"], after_cancel
    finally:
        left.close()
        right.close()


asyncio.run(main())
