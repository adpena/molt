# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket error paths after close."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
errors: list[str] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()
    conn, _addr = srv.accept()
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
try:
    sock.sendall(b"ping")
except Exception as exc:
    errors.append(type(exc).__name__)
try:
    data = sock.recv(1024)
    errors.append(f"recv:{len(data)}")
except Exception as exc:
    errors.append(type(exc).__name__)
sock.close()
t.join(timeout=1.0)

print(errors)
