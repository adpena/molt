# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for WebSocket ping/pong frames."""

import base64
import hashlib
import socket
import threading


GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

ready = threading.Event()
port_holder: list[int] = []
server_seen: list[tuple[int, str]] = []


def compute_accept(key: str) -> str:
    digest = hashlib.sha1((key + GUID).encode("ascii")).digest()
    return base64.b64encode(digest).decode("ascii")


def recv_exact(sock: socket.socket, size: int) -> bytes:
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            break
        data += chunk
    return data


def read_frame(sock: socket.socket) -> tuple[int, bytes]:
    header = recv_exact(sock, 2)
    if len(header) < 2:
        return 0, b""
    b1, b2 = header[0], header[1]
    opcode = b1 & 0x0F
    masked = bool(b2 & 0x80)
    length = b2 & 0x7F
    if length == 126:
        length = int.from_bytes(recv_exact(sock, 2), "big")
    elif length == 127:
        length = int.from_bytes(recv_exact(sock, 8), "big")
    mask_key = b""
    if masked:
        mask_key = recv_exact(sock, 4)
    payload = recv_exact(sock, length) if length else b""
    if masked and mask_key:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return opcode, payload


def build_frame(payload: bytes, opcode: int, mask: bool) -> bytes:
    b1 = 0x80 | (opcode & 0x0F)
    length = len(payload)
    mask_bit = 0x80 if mask else 0x00
    if length < 126:
        header = bytes([b1, mask_bit | length])
        ext = b""
    elif length < 65536:
        header = bytes([b1, mask_bit | 126])
        ext = length.to_bytes(2, "big")
    else:
        header = bytes([b1, mask_bit | 127])
        ext = length.to_bytes(8, "big")
    if not mask:
        return header + ext + payload
    mask_key = b"\x05\x06\x07\x08"
    masked_payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return header + ext + mask_key + masked_payload


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(1024)
        if not chunk:
            break
        data += chunk
    text = data.decode("ascii", errors="replace")
    headers = {}
    for line in text.split("\r\n")[1:]:
        if not line:
            break
        name, value = line.split(":", 1)
        headers[name.strip().lower()] = value.strip()
    key = headers.get("sec-websocket-key", "")
    accept = compute_accept(key)
    response = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n"
        "\r\n"
    ).encode("ascii")
    conn.sendall(response)

    opcode, payload = read_frame(conn)
    server_seen.append((opcode, payload.decode("ascii", errors="replace")))
    pong = build_frame(payload, opcode=0x0A, mask=False)
    conn.sendall(pong)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
key = "dGhlIHNhbXBsZSBub25jZQ=="
request = (
    "GET /chat HTTP/1.1\r\n"
    "Host: example.com\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    "\r\n"
).encode("ascii")
sock.sendall(request)
resp = b""
while b"\r\n\r\n" not in resp:
    chunk = sock.recv(1024)
    if not chunk:
        break
    resp += chunk

ping = build_frame(b"ping", opcode=0x09, mask=True)
sock.sendall(ping)
opcode, payload = read_frame(sock)
sock.close()
t.join(timeout=1.0)

print(server_seen[0])
print(opcode, payload.decode("ascii", errors="replace"))
