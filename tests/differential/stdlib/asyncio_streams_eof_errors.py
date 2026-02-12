"""Purpose: differential coverage for asyncio streams EOF errors."""

import asyncio


async def handle_close(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    writer.write(b"hi")
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def main() -> None:
    server = await asyncio.start_server(handle_close, "127.0.0.1", 0)
    host, port = server.sockets[0].getsockname()[:2]

    reader, writer = await asyncio.open_connection(host, port)
    try:
        await reader.readexactly(4)
    except asyncio.IncompleteReadError as exc:
        print(len(exc.partial), exc.expected)
    writer.close()
    await writer.wait_closed()

    server.close()
    await server.wait_closed()


asyncio.run(main())
