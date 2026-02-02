# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for nonblocking connect SO_ERROR."""

import errno
import select
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
    conn.recv(1)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setblocking(False)
result = sock.connect_ex(("127.0.0.1", port_holder[0]))

if result not in (0, errno.EINPROGRESS, errno.EWOULDBLOCK):
    print("connect_ex", result)
else:
    _r, w, _x = select.select([], [sock], [], 1.0)
    if w:
        err = sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
        print("so_error", err)
    else:
        print("timeout")

sock.send(b"x")
sock.close()

t.join(timeout=1.0)
