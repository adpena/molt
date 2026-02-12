# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib retry handler basic."""

import socket
import threading
import urllib.request


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

    for _ in range(2):
        conn, _addr = srv.accept()
        conn.recv(1024)
        request_count.append(1)
        if len(request_count) == 1:
            conn.sendall(
                b"HTTP/1.1 503 Service Unavailable\r\n"
                b"Content-Length: 0\r\n"
                b"Connection: close\r\n"
                b"\r\n"
            )
        else:
            conn.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
            )
        conn.close()
    srv.close()


class RetryHandler(urllib.request.HTTPErrorProcessor):
    def http_response(self, request, response):
        if response.code == 503:
            return self.parent.open(request)
        return response

    https_response = http_response


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

opener = urllib.request.build_opener(RetryHandler())
url = f"http://127.0.0.1:{port_holder[0]}/"
with opener.open(url, timeout=1.0) as resp:
    body = resp.read().decode()
    print(resp.status, body, len(request_count))


t.join()
