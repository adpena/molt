"""Purpose: asyncio TLS open_connection path is runtime-executed (not fail-fast placeholder)."""

import asyncio


async def _main() -> None:
    try:
        await asyncio.open_connection("nonexistent.molt.invalid", 443, ssl=True)
    except Exception as exc:  # noqa: BLE001 - differential surface capture
        print(type(exc).__name__)
        print(isinstance(exc, OSError))
        print("not yet available" in str(exc))
    else:
        print("connected")


asyncio.run(_main())
