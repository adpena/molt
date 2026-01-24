# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for basic WebSocket handshake."""

import base64
import hashlib
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
seen_accept: list[str] = []


GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def compute_accept(key: str) -> str:
    digest = hashlib.sha1((key + GUID).encode("ascii")).digest()
    return base64.b64encode(digest).decode("ascii")


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
    seen_accept.append(accept)
    response = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n"
        "\r\n"
    ).encode("ascii")
    conn.sendall(response)
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
resp = sock.recv(1024).decode("ascii", errors="replace")
sock.close()
t.join(timeout=1.0)

status = resp.split("\r\n", 1)[0]
accept_line = [line for line in resp.split("\r\n") if line.lower().startswith("sec-websocket-accept")]
print(status)
print(accept_line[0] if accept_line else "")
print(seen_accept[0])
