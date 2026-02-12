"""Purpose: asyncio SSL gate is intrinsic-backed and never raises NotImplementedError."""

import asyncio


async def main() -> None:
    try:
        await asyncio.open_connection("127.0.0.1", 1, ssl=object())
    except Exception as exc:
        print(type(exc).__name__, isinstance(exc, NotImplementedError))
    else:
        print("no_error", False)


asyncio.run(main())
