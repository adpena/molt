# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket getsockname peername."""

import socket
import threading


ready = threading.Event()
port_holder = []
peer = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    peer.append(conn.getpeername())
    conn.sendall(b"ok")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
print(sock.getsockname()[0] in ("127.0.0.1", "0.0.0.0"))
print(sock.getpeername()[0] == "127.0.0.1")
sock.recv(2)
sock.close()

t.join()

print(peer[0][0] == "127.0.0.1")
