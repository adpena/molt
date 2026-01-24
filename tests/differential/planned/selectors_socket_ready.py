# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors with socket read readiness."""

import selectors
import socket
import threading


sel = selectors.DefaultSelector()
ready = threading.Event()
port_holder: list[int] = []
received: list[bytes] = []


def client() -> None:
    ready.wait(timeout=1.0)
    sock = socket.create_connection(("127.0.0.1", port_holder[0]))
    sock.sendall(b"ping")
    sock.close()


srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 0))
srv.listen(1)
srv.setblocking(False)
port_holder.append(srv.getsockname()[1])
sel.register(srv, selectors.EVENT_READ, data="srv")
ready.set()

t = threading.Thread(target=client)
t.start()

events = sel.select(timeout=1.0)
accepted = False
conn = None
for key, mask in events:
    if key.data == "srv" and mask & selectors.EVENT_READ:
        conn, _addr = srv.accept()
        conn.setblocking(False)
        sel.register(conn, selectors.EVENT_READ, data="conn")
        accepted = True

if conn is not None:
    events = sel.select(timeout=1.0)
    for key, mask in events:
        if key.data == "conn" and mask & selectors.EVENT_READ:
            data = conn.recv(1024)
            received.append(data)

if conn is not None:
    sel.unregister(conn)
    conn.close()
sel.unregister(srv)
srv.close()
t.join(timeout=1.0)

print("accepted", accepted)
print("data", received[0].decode("ascii"))
