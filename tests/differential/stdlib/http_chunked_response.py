# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http chunked response."""

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
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Transfer-Encoding: chunked\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b"5\r\nHello\r\n"
        b"5\r\nWorld\r\n"
        b"0\r\n\r\n"
    )
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.request("GET", "/")
resp = conn.getresponse()
body = resp.read().decode()
print(resp.status, body)
conn.close()


t.join()
