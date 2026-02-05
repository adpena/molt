import asyncio
import base64
import hashlib
import os

os.environ.setdefault("MOLT_TRUSTED", "1")
os.environ.setdefault("MOLT_CAPABILITIES", "net,net.poll,websocket.connect")

from molt import net

_GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def _build_accept(key: bytes) -> str:
    digest = hashlib.sha1(key + _GUID).digest()
    return base64.b64encode(digest).decode("ascii")


async def _read_headers(reader: asyncio.StreamReader) -> tuple[bytes, bytearray] | None:
    buf = bytearray()
    while True:
        chunk = await reader.read(4096)
        if not chunk:
            return None
        buf.extend(chunk)
        idx = buf.find(b"\r\n\r\n")
        if idx != -1:
            header = bytes(buf[: idx + 4])
            rest = bytearray(buf[idx + 4 :])
            return header, rest
        if len(buf) > 65536:
            return None


async def _read_exact(
    reader: asyncio.StreamReader, buf: bytearray, size: int
) -> bytes | None:
    while len(buf) < size:
        chunk = await reader.read(size - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    data = bytes(buf[:size])
    del buf[:size]
    return data


def _mask_payload(payload: bytes, mask_key: bytes) -> bytes:
    return bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))


async def _ws_echo_handler(
    reader: asyncio.StreamReader, writer: asyncio.StreamWriter
) -> None:
    headers = await _read_headers(reader)
    if headers is None:
        writer.close()
        return
    header_bytes, buf = headers
    lines = header_bytes.split(b"\r\n")
    header_map: dict[bytes, bytes] = {}
    for line in lines[1:]:
        if not line:
            break
        if b":" not in line:
            continue
        name, value = line.split(b":", 1)
        header_map[name.strip().lower()] = value.strip()
    key = header_map.get(b"sec-websocket-key")
    if not key:
        writer.close()
        return
    accept = _build_accept(key)
    response = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n"
        "\r\n"
    )
    writer.write(response.encode("ascii"))
    await writer.drain()

    while True:
        header = await _read_exact(reader, buf, 2)
        if not header:
            break
        b1, b2 = header[0], header[1]
        opcode = b1 & 0x0F
        masked = (b2 & 0x80) != 0
        length = b2 & 0x7F
        if length == 126:
            ext = await _read_exact(reader, buf, 2)
            if not ext:
                break
            length = int.from_bytes(ext, "big")
        elif length == 127:
            ext = await _read_exact(reader, buf, 8)
            if not ext:
                break
            length = int.from_bytes(ext, "big")
        mask_key = b""
        if masked:
            mask_key = await _read_exact(reader, buf, 4) or b""
        payload = b""
        if length:
            payload = await _read_exact(reader, buf, length) or b""
        if masked and payload:
            payload = _mask_payload(payload, mask_key)

        if opcode == 0x8:
            # close
            await _send_frame(writer, 0x8, payload)
            break
        if opcode == 0x9:
            # ping
            await _send_frame(writer, 0xA, payload)
            continue
        if opcode == 0xA:
            # pong
            continue
        if opcode in (0x1, 0x2):
            await _send_frame(writer, opcode, payload)
    writer.close()


async def _send_frame(
    writer: asyncio.StreamWriter, opcode: int, payload: bytes
) -> None:
    fin = 0x80 | (opcode & 0x0F)
    length = len(payload)
    if length < 126:
        header = bytes([fin, length])
    elif length < 65536:
        header = bytes([fin, 126]) + length.to_bytes(2, "big")
    else:
        header = bytes([fin, 127]) + length.to_bytes(8, "big")
    writer.write(header + payload)
    await writer.drain()


async def _run_client(url: str, iterations: int, payload: bytes) -> None:
    ws = net.ws_connect(url)
    try:
        for _ in range(iterations):
            await ws.send(payload)
            msg = await ws.recv()
            if msg != payload:
                raise RuntimeError("websocket echo mismatch")
    finally:
        await ws.close()


async def _run_bench(url: str | None, iterations: int, payload: bytes) -> None:
    server = None
    if url is None:
        server = await asyncio.start_server(_ws_echo_handler, "127.0.0.1", 0)
        sock = server.sockets[0]
        host, port = sock.getsockname()[:2]
        url = f"ws://{host}:{port}"
    try:
        await _run_client(url, iterations, payload)
    finally:
        if server is not None:
            server.close()
            await server.wait_closed()


def main() -> None:
    url = os.environ.get("MOLT_WS_BENCH_URL")
    iterations = int(os.environ.get("MOLT_WS_BENCH_ITERS", "2000"))
    size = int(os.environ.get("MOLT_WS_BENCH_SIZE", "32"))
    payload = b"x" * size
    asyncio.run(_run_bench(url, iterations, payload))
    print(iterations)


if __name__ == "__main__":
    main()
