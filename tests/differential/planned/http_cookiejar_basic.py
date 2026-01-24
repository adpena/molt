# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http cookiejar basic."""

import socket
import threading
import http.cookiejar
import urllib.request


ready = threading.Event()
port_holder: list[int] = []
request_cookie: list[str] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    # First request: set cookie
    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Set-Cookie: session=abc; Path=/\r\n"
        b"Content-Length: 2\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b"OK"
    )
    conn.close()

    # Second request: expect cookie
    conn, _addr = srv.accept()
    data = conn.recv(2048).decode("latin-1")
    for line in data.split("\r\n"):
        if line.lower().startswith("cookie:"):
            request_cookie.append(line)
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

jar = http.cookiejar.CookieJar()
handler = urllib.request.HTTPCookieProcessor(jar)
opener = urllib.request.build_opener(handler)
base = f"http://127.0.0.1:{port_holder[0]}"

with opener.open(base + "/one", timeout=1.0) as resp:
    resp.read()
with opener.open(base + "/two", timeout=1.0) as resp:
    resp.read()


t.join()
print("cookie", len(jar), "sent", "session=abc" in request_cookie[0])
