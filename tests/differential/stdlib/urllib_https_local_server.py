# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib https local server."""

import os
import socket
import ssl
import threading
import urllib.request


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CANDIDATES = [
    os.path.join(
        ROOT, "third_party", "cpython-3.13", "Lib", "test", "certdata", "keycert.pem"
    ),
    os.path.join(
        ROOT, "third_party", "cpython-3.14", "Lib", "test", "certdata", "keycert.pem"
    ),
    os.path.join(
        ROOT, "third_party", "cpython-3.12", "Lib", "test", "certdata", "keycert.pem"
    ),
]

cert_path = None
for path in CANDIDATES:
    if os.path.exists(path):
        cert_path = path
        break

if cert_path is None:
    raise FileNotFoundError("missing test certificate")

ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certfile=cert_path)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    tls = context.wrap_socket(conn, server_side=True)
    tls.recv(1024)
    tls.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    tls.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

url = f"https://127.0.0.1:{port_holder[0]}/"
ctx = ssl._create_unverified_context()
with urllib.request.urlopen(url, context=ctx, timeout=1.0) as resp:
    body = resp.read().decode()
    print(resp.status, body)

t.join()
