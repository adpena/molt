"""Purpose: differential coverage for malformed HTTP upgrade headers."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio


async def _read_headers(reader: asyncio.StreamReader) -> list[str]:
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = await reader.read(4096)
        if not chunk:
            break
        data += chunk
        if len(data) > 65536:
            break
    text = data.decode("ascii", "surrogateescape")
    head = text.split("\r\n\r\n", 1)[0]
    return [line for line in head.split("\r\n") if line]


async def _handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    lines = await _read_headers(reader)
    headers = {}
    for line in lines[1:]:
        if ":" not in line:
            continue
        name, value = line.split(":", 1)
        headers[name.strip().lower()] = value.strip()

    valid = (
        headers.get("upgrade", "").lower() == "websocket"
        and "upgrade" in headers.get("connection", "").lower()
        and "sec-websocket-key" in headers
        and headers.get("sec-websocket-version") == "13"
    )

    if valid:
        response = "HTTP/1.1 101 Switching Protocols\r\n\r\n"
    else:
        response = "HTTP/1.1 400 Bad Request\r\n\r\n"
    writer.write(response.encode("ascii"))
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def main() -> None:
    server = await asyncio.start_server(_handle, "127.0.0.1", 0)
    try:
        sock = server.sockets[0]
        host, port = sock.getsockname()[:2]
        reader, writer = await asyncio.open_connection(host, port)
        request = (
            "GET / HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: keep-alive\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "\r\n"
        )
        writer.write(request.encode("ascii"))
        await writer.drain()
        status = await reader.readline()
        writer.close()
        await writer.wait_closed()
    finally:
        server.close()
        await server.wait_closed()

    print("status", status.decode("ascii", "surrogateescape").strip())


asyncio.run(main())
