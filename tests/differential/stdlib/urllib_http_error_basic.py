# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib http error basic."""

import socket
import threading
import urllib.error
import urllib.request


ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(
        b"HTTP/1.1 404 Not Found\r\n"
        b"Content-Length: 9\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b"Not Found"
    )
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

url = f"http://127.0.0.1:{port_holder[0]}/missing"
try:
    urllib.request.urlopen(url, timeout=1.0)
except urllib.error.HTTPError as exc:
    body = exc.read().decode()
    print(exc.code, exc.reason, body)


t.join()
