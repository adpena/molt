"""Purpose: unix SSL open_connection path should not fail with unsupported-op fallback."""

import asyncio


SOCK_PATH = "/tmp/molt_asyncio_ssl_unix_connection_intrinsic.sock"


async def main() -> None:
    try:
        await asyncio.open_unix_connection(SOCK_PATH, ssl=True)
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
