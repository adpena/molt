# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for SNI callback context switch."""

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
server_name: list[str | None] = []
server_alpn: list[str | None] = []


base_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
base_ctx.load_cert_chain(certfile=cert_path)
base_ctx.set_alpn_protocols(["http/1.1"])

alt_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
alt_ctx.load_cert_chain(certfile=cert_path)
alt_ctx.set_alpn_protocols(["h2"])


def sni_callback(_sock, name, _ctx):
    server_name.append(name)
    if name == "alt.example":
        return alt_ctx
    return None


def server() -> None:
    base_ctx.set_servername_callback(sni_callback)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    tls = base_ctx.wrap_socket(conn, server_side=True)
    server_alpn.append(tls.selected_alpn_protocol())
    tls.recv(1)
    tls.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

client_ctx = ssl.create_default_context()
client_ctx.check_hostname = False
client_ctx.verify_mode = ssl.CERT_NONE
client_ctx.set_alpn_protocols(["h2", "http/1.1"])

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
wrapped = client_ctx.wrap_socket(sock, server_hostname="alt.example")
wrapped.sendall(b"x")
wrapped.close()
t.join(timeout=1.0)

print("sni", server_name[0])
print("alpn", server_alpn[0])
