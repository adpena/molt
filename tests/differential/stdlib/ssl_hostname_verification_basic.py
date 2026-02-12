# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for ssl hostname verification basic."""

import os
import socket
import ssl
import threading


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CANDIDATES = [
    os.path.join(
        ROOT,
        "third_party",
        "cpython-3.13",
        "Lib",
        "test",
        "certdata",
        "keycert.pem",
    ),
    os.path.join(
        ROOT,
        "third_party",
        "cpython-3.14",
        "Lib",
        "test",
        "certdata",
        "keycert.pem",
    ),
    os.path.join(
        ROOT,
        "third_party",
        "cpython-3.12",
        "Lib",
        "test",
        "certdata",
        "keycert.pem",
    ),
]

CERT_PATH = None
for candidate in CANDIDATES:
    if os.path.exists(candidate):
        CERT_PATH = candidate
        break

if CERT_PATH is None:
    raise FileNotFoundError("missing test certificate")

ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certfile=CERT_PATH)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    tls = context.wrap_socket(conn, server_side=True)
    tls.recv(1)
    tls.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

client_ctx = ssl.create_default_context()
client_ctx.check_hostname = True
client_ctx.verify_mode = ssl.CERT_REQUIRED
client_ctx.load_verify_locations(cafile=CERT_PATH)

try:
    sock = socket.create_connection(("127.0.0.1", port_holder[0]))
    wrapped = client_ctx.wrap_socket(sock, server_hostname="localhost")
    wrapped.sendall(b"x")
    wrapped.close()
    print("ok")
except Exception as exc:
    print(type(exc).__name__)


t.join()
