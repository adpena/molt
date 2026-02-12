"""Purpose: differential coverage for asyncio streams basic."""

import asyncio


async def handle_echo(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    data = await reader.readexactly(4)
    writer.write(data.upper())
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def main() -> None:
    server = await asyncio.start_server(handle_echo, "127.0.0.1", 0)
    host, port = server.sockets[0].getsockname()[:2]

    reader, writer = await asyncio.open_connection(host, port)
    writer.write(b"ping")
    await writer.drain()
    response = await reader.readexactly(4)
    writer.close()
    await writer.wait_closed()

    server.close()
    await server.wait_closed()
    print(response)


asyncio.run(main())
