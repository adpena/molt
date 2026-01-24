# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http proxy auth basic."""

import base64
import http.client
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
header_seen: list[bool] = []


def proxy() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = conn.recv(4096).decode("latin-1")
    header_seen.append("Proxy-Authorization: Basic" in data)
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


t = threading.Thread(target=proxy)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
credentials = base64.b64encode(b"user:pass").decode()
headers = {"Proxy-Authorization": f"Basic {credentials}"}
conn.request("GET", "http://example.com/", headers=headers)
resp = conn.getresponse()
body = resp.read().decode()
conn.close()

t.join()

print(resp.status, body)
print("proxy_auth", header_seen[0])
