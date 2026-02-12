# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http keepalive reuse."""

import http.client
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
request_count: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    # Handle two requests on the same connection.
    for _ in range(2):
        data = conn.recv(2048)
        if data:
            request_count.append(1)
            conn.sendall(
                b"HTTP/1.1 200 OK\r\n"
                b"Content-Length: 2\r\n"
                b"Connection: keep-alive\r\n"
                b"\r\n"
                b"OK"
            )
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.request("GET", "/one")
resp1 = conn.getresponse()
body1 = resp1.read().decode()

conn.request("GET", "/two")
resp2 = conn.getresponse()
body2 = resp2.read().decode()
conn.close()

t.join()

print(resp1.status, body1, resp2.status, body2, len(request_count))
