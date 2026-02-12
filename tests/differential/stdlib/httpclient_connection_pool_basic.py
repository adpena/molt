# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for httpclient connection pool basic."""

import http.client
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
conn_ids: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(2)
    ready.set()

    for _ in range(2):
        conn, _addr = srv.accept()
        conn_ids.append(id(conn))
        conn.recv(2048)
        conn.sendall(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
        )
        conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

conn1 = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn1.request("GET", "/one")
resp1 = conn1.getresponse()
resp1.read()
conn1.close()

conn2 = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn2.request("GET", "/two")
resp2 = conn2.getresponse()
resp2.read()
conn2.close()

t.join()

print("connections", len(conn_ids))
