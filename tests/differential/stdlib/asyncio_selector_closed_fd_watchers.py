"""Purpose: closed selector loops reject adds and return False on removes."""

import asyncio
import socket


loop = asyncio.SelectorEventLoop()
left, right = socket.socketpair()
try:
    loop.close()
    for name, fn, fd in (
        ("add_reader", loop.add_reader, left.fileno()),
        ("add_writer", loop.add_writer, right.fileno()),
    ):
        try:
            fn(fd, lambda: None)
        except Exception as exc:
            print(name, type(exc).__name__, str(exc))
    print("remove_reader", loop.remove_reader(left.fileno()))
    print("remove_writer", loop.remove_writer(right.fileno()))
finally:
    left.close()
    right.close()
