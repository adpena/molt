# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors socket write readiness."""

import selectors
import socket
import threading


sel = selectors.DefaultSelector()
ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port_holder.append(srv.getsockname()[1])
    ready.set()
    conn, _addr = srv.accept()
    conn.recv(1)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setblocking(False)
code = sock.connect_ex(("127.0.0.1", port_holder[0]))
sel.register(sock, selectors.EVENT_WRITE, data="client")
events = sel.select(timeout=1.0)
write_ready = False
for key, mask in events:
    if key.data == "client" and mask & selectors.EVENT_WRITE:
        write_ready = True
sock.sendall(b"x")
sel.unregister(sock)
sock.close()
t.join(timeout=1.0)

print("connect_ex", code)
print("write_ready", write_ready)
