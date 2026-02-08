"""Purpose: asyncio server creation with ssl=False avoids NotImplemented fallback lanes."""

import asyncio


async def main() -> None:
    async def _on_client(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        writer.close()

    try:
        server = await asyncio.start_server(_on_client, host="127.0.0.1", port=0, ssl=False)
    except Exception as exc:
        print(type(exc).__name__, isinstance(exc, NotImplementedError))
        return
    try:
        print("started", isinstance(server, asyncio.Server))
    finally:
        server.close()
        await server.wait_closed()


asyncio.run(main())
