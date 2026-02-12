"""Purpose: differential coverage for asyncio streams over TCP."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio


async def _handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    data = await reader.readexactly(4)
    writer.write(data)
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def main() -> None:
    server = await asyncio.start_server(_handle, "127.0.0.1", 0)
    try:
        sock = server.sockets[0]
        host, port = sock.getsockname()[:2]
        reader, writer = await asyncio.open_connection(host, port)
        writer.write(b"ping")
        await writer.drain()
        data = await reader.readexactly(4)
        print(data)
        writer.close()
        await writer.wait_closed()
    finally:
        server.close()
        await server.wait_closed()


asyncio.run(main())
