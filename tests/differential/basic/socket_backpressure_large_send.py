# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for large send/recv backpressure."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    payload = b"x" * 200_000
    conn.sendall(payload)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
total = 0
while True:
    data = sock.recv(8192)
    if not data:
        break
    total += len(data)
sock.close()
t.join(timeout=1.0)

print(total)
