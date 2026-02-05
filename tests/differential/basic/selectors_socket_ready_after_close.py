# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors readiness on close."""

import selectors
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    conn.sendall(b"ping")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sel = selectors.DefaultSelector()
sock = socket.create_connection(("127.0.0.1", port_holder[0]))
sock.setblocking(False)
sel.register(sock, selectors.EVENT_READ)

events1 = sel.select(timeout=1.0)
data1 = b""
if events1:
    data1 = sock.recv(1024)

events2 = sel.select(timeout=1.0)
data2 = b""
if events2:
    data2 = sock.recv(1024)

sel.unregister(sock)
sock.close()
sel.close()
t.join(timeout=1.0)

print(len(events1), data1)
print(len(events2), data2)
