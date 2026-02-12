# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http client timeout read."""

import http.client
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
    conn.recv(1024)
    # Never send a response to trigger timeout on client read.
    threading.Event().wait(0.5)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=0.1)
conn.request("GET", "/")

try:
    conn.getresponse()
except Exception as exc:
    print(type(exc).__name__)

conn.close()

t.join()
