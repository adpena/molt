"""Purpose: verify reader/writer unregister routes through intrinsic logic."""

import asyncio
import socket


async def main() -> None:
    if not hasattr(socket, "socketpair"):
        print("unsupported")
        return

    left, right = socket.socketpair()
    left.setblocking(False)
    right.setblocking(False)
    loop = asyncio.get_running_loop()
    try:
        lfd = left.fileno()
        rfd = right.fileno()
        loop.add_reader(lfd, lambda: None)
        loop.add_writer(rfd, lambda: None)
        removed_reader_first = loop.remove_reader(lfd)
        removed_reader_second = loop.remove_reader(lfd)
        removed_writer_first = loop.remove_writer(rfd)
        removed_writer_second = loop.remove_writer(rfd)
        print(
            removed_reader_first,
            removed_reader_second,
            removed_writer_first,
            removed_writer_second,
        )
    finally:
        left.close()
        right.close()


asyncio.run(main())
