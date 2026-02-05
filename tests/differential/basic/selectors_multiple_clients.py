# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors with multiple clients."""

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
    srv.listen(2)
    ready.set()

    conns = []
    for _ in range(2):
        conn, _addr = srv.accept()
        conns.append(conn)

    conns[0].sendall(b"first")
    conns[1].sendall(b"second")
    for conn in conns:
        conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sel = selectors.DefaultSelector()
socks = []
for _ in range(2):
    sock = socket.create_connection(("127.0.0.1", port_holder[0]))
    sock.setblocking(False)
    sel.register(sock, selectors.EVENT_READ)
    socks.append(sock)

received: list[str] = []
while len(received) < 2:
    events = sel.select(timeout=1.0)
    if not events:
        break
    for key, _mask in events:
        data = key.fileobj.recv(1024)
        if data:
            received.append(data.decode("ascii", errors="replace"))
        else:
            sel.unregister(key.fileobj)

for sock in socks:
    try:
        sel.unregister(sock)
    except Exception:
        pass
    sock.close()
sel.close()
t.join(timeout=1.0)

print(sorted(received))
