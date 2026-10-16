# MOLT_ENV: MOLT_CAPABILITIES=net,env.read,thread.spawn,thread.shared
"""Purpose: differential coverage for urllib redirect basic."""

import socket
import threading
import urllib.request


ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    # First request: redirect
    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(
        b"HTTP/1.1 302 Found\r\n"
        b"Location: /final\r\n"
        b"Content-Length: 0\r\n"
        b"Connection: close\r\n"
        b"\r\n"
    )
    conn.close()

    # Second request: final
    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

url = f"http://127.0.0.1:{port_holder[0]}/start"
with urllib.request.urlopen(url, timeout=1.0) as resp:
    body = resp.read().decode()
    print(resp.status, resp.geturl().endswith("/final"), body)


t.join()


class TimeoutObserver(urllib.request.BaseHandler):
    def __init__(self):
        self.observed = []

    def http_request(self, req):
        self.observed.append((req.full_url.endswith("/final"), req.timeout))
        return req


class KeywordOnlyOpener(urllib.request.OpenerDirector):
    def __init__(self):
        super().__init__()
        self.calls = []

    def open(self, fullurl, *, timeout):
        self.calls.append(timeout)
        return super().open(fullurl, timeout=timeout)


for timeout, expected, expected_calls in (
    (1.0, [(False, 1.0), (True, 1.0)], [1.0, 1.0]),
    (None, [(False, None), (True, None)], [None, None]),
):
    ready.clear()
    port_holder.clear()
    # If keyword binding regresses, the failed client must not leave a server
    # accept keeping the differential process alive indefinitely.
    thread = threading.Thread(target=server, daemon=True)
    thread.start()
    assert ready.wait(timeout=1.0)
    observer = TimeoutObserver()
    opener = KeywordOnlyOpener()
    for handler in (
        urllib.request.ProxyHandler({}),
        urllib.request.HTTPHandler(),
        urllib.request.HTTPRedirectHandler(),
        urllib.request.HTTPErrorProcessor(),
        observer,
    ):
        opener.add_handler(handler)
    url = f"http://127.0.0.1:{port_holder[0]}/start"
    with opener.open(url, timeout=timeout) as resp:
        body = resp.read().decode()
        assert (resp.status, resp.geturl().endswith("/final"), body) == (
            200,
            True,
            "OK",
        )
    thread.join(timeout=2.0)
    assert not thread.is_alive()
    assert observer.observed == expected
    assert opener.calls == expected_calls
    print("redirect timeout", observer.observed, opener.calls)
