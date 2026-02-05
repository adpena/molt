"""Purpose: differential coverage for HTTP upgrade via asyncio raw sockets."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio

_KEY = "dGhlIHNhbXBsZSBub25jZQ=="


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

    response = "HTTP/1.1 101 Switching Protocols\r\n\r\n"
    writer.write(response.encode("ascii"))
    await writer.drain()
    writer.close()
    await writer.wait_closed()

    server_state["headers"] = headers


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
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {_KEY}\r\n"
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

    headers = server_state.get("headers", {})
    if not isinstance(headers, dict):
        headers = {}
    print("status", status.decode("ascii", "surrogateescape").strip())
    print("upgrade", headers.get("upgrade") == "websocket")
    print("connection", "upgrade" in headers.get("connection", "").lower())
    print("sec_key", headers.get("sec-websocket-key") == _KEY)
    print("sec_ver", headers.get("sec-websocket-version") == "13")


server_state = {}
asyncio.run(main())
