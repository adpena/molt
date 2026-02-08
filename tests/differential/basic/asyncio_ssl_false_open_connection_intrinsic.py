"""Purpose: ssl=False stays on intrinsic path without NotImplemented fallback errors."""

import asyncio


async def main() -> None:
    try:
        await asyncio.open_connection("127.0.0.1", 9, ssl=False)
    except Exception as exc:
        print(type(exc).__name__, isinstance(exc, NotImplementedError))
    else:
        print("connected", False)


asyncio.run(main())
