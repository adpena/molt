# MOLT_META: stderr=exact
# MOLT_ENV: MOLT_CAPABILITIES=net,thread.spawn,thread.shared
"""Purpose: actual urllib wire, method override, and error identity coverage."""

import socket
import threading
import urllib.request


ready = threading.Event()
port_holder: list[int] = []
received: list[tuple[bytes, bytes, bytes]] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.settimeout(5.0)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()
    try:
        for _ in range(3):
            conn, _addr = srv.accept()
            try:
                conn.settimeout(5.0)
                raw = b""
                while b"\r\n\r\n" not in raw:
                    chunk = conn.recv(4096)
                    assert chunk, "connection ended before request headers"
                    raw += chunk
                head, body = raw.split(b"\r\n\r\n", 1)
                lines = head.split(b"\r\n")
                headers = {}
                for line in lines[1:]:
                    key, value = line.split(b":", 1)
                    headers[key.lower()] = value.strip()
                length = int(headers.get(b"content-length", b"0"))
                while len(body) < length:
                    chunk = conn.recv(4096)
                    assert chunk, "connection ended before request body"
                    body += chunk
                received.append((lines[0], headers.get(b"x-probe", b""), body))
                conn.sendall(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n"
                    b"Connection: close\r\n\r\nOK"
                )
            finally:
                conn.close()
    finally:
        srv.close()


class MethodRequest(urllib.request.Request):
    def get_method(self):
        return "PATCH"


method_error = RuntimeError("method callback failed")


class FailingMethodRequest(urllib.request.Request):
    def get_method(self):
        raise method_error


t = threading.Thread(target=server, daemon=True)
t.start()
assert ready.wait(timeout=5.0)

url = f"http://127.0.0.1:{port_holder[0]}/"
# Explicit policy makes this loopback witness independent of host proxy state.
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
with opener.open(url, timeout=5.0) as resp:
    body = resp.read().decode()
    assert resp.status == 200 and body == "OK"
    print(resp.status, body)

for request in (
    urllib.request.Request(url, data=b"", headers={"X-Probe": "empty"}),
    MethodRequest(url, data=b"payload", headers={"X-Probe": "override"}),
):
    with opener.open(request, timeout=5.0) as resp:
        assert resp.status == 200
        assert resp.read() == b"OK"

t.join(timeout=5.0)
assert not t.is_alive()
assert received == [
    (b"GET / HTTP/1.1", b"", b""),
    (b"POST / HTTP/1.1", b"empty", b""),
    (b"PATCH / HTTP/1.1", b"override", b"payload"),
]
print("request methods and headers", received)

# The server is closed: a missing callback would reach transport and lose identity.
try:
    opener.open(FailingMethodRequest(url), timeout=5.0)
except RuntimeError as error:
    assert error is method_error
    print("method error identity", error is method_error)
else:
    raise AssertionError("method callback did not propagate")


method_lookup_error = AttributeError("method descriptor failed")
data_lookup_error = AttributeError("data descriptor failed")


class FailingMethodLookupRequest(urllib.request.Request):
    @property
    def get_method(self):
        raise method_lookup_error


class FailingDataLookupRequest(urllib.request.Request):
    def __init__(self, url):
        self.method_called = False
        super().__init__(url)

    def get_method(self):
        self.method_called = True
        return "GET"

    @property
    def data(self):
        if self.method_called:
            raise data_lookup_error
        return self._probe_data

    @data.setter
    def data(self, value):
        self._probe_data = value


for label, request, expected in (
    ("method lookup", FailingMethodLookupRequest(url), method_lookup_error),
    ("data lookup", FailingDataLookupRequest(url), data_lookup_error),
):
    try:
        opener.open(request, timeout=5.0)
    except AttributeError as error:
        assert error is expected
        if label == "data lookup":
            assert request.method_called
        print(label, "error identity", error is expected)
    else:
        raise AssertionError("required request lookup did not propagate")
