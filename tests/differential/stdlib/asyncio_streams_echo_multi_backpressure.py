"""Purpose: differential coverage for asyncio streams multi-message flow."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio


async def _handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    for _ in range(3):
        data = await reader.readexactly(3)
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
        for payload in (b"one", b"two", b"thr"):
            writer.write(payload)
        await writer.drain()
        data = await reader.readexactly(9)
        print([data[i : i + 3] for i in range(0, 9, 3)])
        writer.close()
        await writer.wait_closed()
    finally:
        server.close()
        await server.wait_closed()


asyncio.run(main())
