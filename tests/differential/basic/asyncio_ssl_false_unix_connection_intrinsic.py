"""Purpose: ssl=False unix-connection path should avoid NotImplemented fallback errors."""

import asyncio


async def main() -> None:
    try:
        await asyncio.open_unix_connection(
            "/tmp/molt_asyncio_ssl_false_unix_connection_intrinsic.sock",
            ssl=False,
        )
    except Exception as exc:
        print(type(exc).__name__, isinstance(exc, NotImplementedError))
    else:
        print("connected", False)


asyncio.run(main())
