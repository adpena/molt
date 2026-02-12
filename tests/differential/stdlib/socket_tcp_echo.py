# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket tcp echo."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[str] = []


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
    conn.sendall(b"pong")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
sock.sendall(b"ping")
resp = sock.recv(1024)
sock.close()

t.join()

print("recv", received[0])
print("resp", resp.decode())
