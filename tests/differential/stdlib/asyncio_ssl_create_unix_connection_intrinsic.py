"""Purpose: loop.create_unix_connection SSL path should avoid unsupported-op fallback."""

import asyncio


SOCK_PATH = "/tmp/molt_asyncio_ssl_create_unix_connection_intrinsic.sock"


class _Proto(asyncio.Protocol):
    pass


async def main() -> None:
    loop = asyncio.get_running_loop()
    try:
        await loop.create_unix_connection(_Proto, SOCK_PATH, ssl=True)
    except Exception as exc:
        message = str(exc)
        print(type(exc).__name__)
        print("unsupported asyncio SSL transport operation" in message)
        print(isinstance(exc, NotImplementedError))
    else:
        print("connected")
        print(False)
        print(False)


asyncio.run(main())
