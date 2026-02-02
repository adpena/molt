# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket shutdown basic."""

import socket
import threading


ready = threading.Event()
port_holder = []
received = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = conn.recv(1024)
    received.append(data.decode())
    conn.shutdown(socket.SHUT_WR)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
sock.sendall(b"ping")
sock.shutdown(socket.SHUT_WR)
try:
    sock.recv(1)
except Exception:
    pass
sock.close()

t.join()

print(received[0])
