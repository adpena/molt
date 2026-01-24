# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for start_tls-style upgrade."""

import os
import socket
import ssl
import threading

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
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(certfile=cert_path)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()
    conn, _addr = srv.accept()
    data = conn.recv(32)
    if data == b"STARTTLS":
        conn.sendall(b"OK")
        tls = ctx.wrap_socket(conn, server_side=True)
        tls.sendall(b"hi")
        tls.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
sock.sendall(b"STARTTLS")
resp = sock.recv(2)

client_ctx = ssl.create_default_context()
client_ctx.check_hostname = False
client_ctx.verify_mode = ssl.CERT_NONE
wrapped = client_ctx.wrap_socket(sock, server_hostname="localhost")
msg = wrapped.recv(2)
wrapped.close()

t.join(timeout=1.0)

print(resp, msg)
