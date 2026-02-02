# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket makefile basic."""

import socket
import threading


ready = threading.Event()
port_holder = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    fileobj = conn.makefile("rwb")
    data = fileobj.readline().strip()
    fileobj.write(data + b"\n")
    fileobj.flush()
    fileobj.close()
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
fileobj = sock.makefile("rwb")
fileobj.write(b"ping\n")
fileobj.flush()
resp = fileobj.readline().strip()
fileobj.close()
sock.close()

t.join()

print(resp.decode())
