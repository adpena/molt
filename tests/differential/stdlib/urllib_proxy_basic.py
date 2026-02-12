# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib proxy basic."""

import socket
import threading
import urllib.request


ready = threading.Event()
port_holder: list[int] = []
request_line: list[str] = []


def proxy() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = conn.recv(4096).decode("latin-1")
    request_line.append(data.split("\r\n", 1)[0])
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


t = threading.Thread(target=proxy)
t.start()
ready.wait(timeout=1.0)

proxy_url = f"http://127.0.0.1:{port_holder[0]}"
handler = urllib.request.ProxyHandler({"http": proxy_url})
opener = urllib.request.build_opener(handler)
url = "http://example.com/path"
with opener.open(url, timeout=1.0) as resp:
    body = resp.read().decode()
    print(resp.status, body)


t.join()

print("line", request_line[0])
