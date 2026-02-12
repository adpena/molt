"""Purpose: asyncio unix-socket gate is intrinsic-backed and avoids NotImplementedError."""

import asyncio


async def main() -> None:
    try:
        await asyncio.open_unix_connection(
            "/tmp/molt_asyncio_unix_socket_capability_intrinsic.sock",
            ssl=object(),
        )
    except Exception as exc:
        print(type(exc).__name__, isinstance(exc, NotImplementedError))
    else:
        print("no_error", False)


asyncio.run(main())
