"""Purpose: differential coverage for asyncio streams flow control."""

import asyncio


async def handle_drain(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    total = 0
    while True:
        chunk = await reader.read(4096)
        if not chunk:
            break
        total += len(chunk)
    writer.write(str(total).encode())
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def main() -> None:
    server = await asyncio.start_server(handle_drain, "127.0.0.1", 0)
    host, port = server.sockets[0].getsockname()[:2]

    reader, writer = await asyncio.open_connection(host, port)
    payload = b"x" * 65536
    writer.write(payload)
    await writer.drain()
    try:
        writer.write_eof()
    except (AttributeError, OSError):
        pass
    response = await asyncio.wait_for(reader.read(), timeout=1.0)
    writer.close()
    await writer.wait_closed()

    server.close()
    await server.wait_closed()
    print(response)


asyncio.run(main())
