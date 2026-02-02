# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket create helpers."""

import socket
import threading

ready = threading.Event()
port_holder: list[int] = []


def server(sock: socket.socket) -> None:
    sock.listen(1)
    port_holder.append(sock.getsockname()[1])
    ready.set()
    conn, _addr = sock.accept()
    conn.sendall(b"ok")
    conn.close()
    sock.close()


if hasattr(socket, "create_server"):
    srv = socket.create_server(("127.0.0.1", 0))
else:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))

t = threading.Thread(target=server, args=(srv,))
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
print(sock.recv(2))
sock.close()

t.join(timeout=1.0)

print(hasattr(socket, "create_server"))
