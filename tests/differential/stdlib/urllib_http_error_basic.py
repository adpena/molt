# MOLT_ENV: MOLT_CAPABILITIES=net,thread.spawn,thread.shared
"""Purpose: differential coverage for urllib http error basic."""

import gc
import weakref
import socket
import threading
import urllib.error
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

    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(
        b"HTTP/1.1 404 Not Found\r\n"
        b"Content-Length: 9\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b"Not Found"
    )
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

class ErrorMarker:
    pass


opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
marker = ErrorMarker()
observed_http = weakref.ref(marker)
url = f"http://127.0.0.1:{port_holder[0]}/missing"
try:
    opener.open(url, timeout=1.0)
except urllib.error.HTTPError as exc:
    exc.marker = marker
    body = exc.read().decode()
    print(exc.code, exc.reason, body)


t.join()

# The HTTP error owns its stream and constructor payload only while reachable.
del marker
gc.collect()
assert observed_http() is None

# A missing host selects the URLError producer before any network operation.
marker = ErrorMarker()
observed_url = weakref.ref(marker)
try:
    opener.open("http:///missing", timeout=1.0)
except urllib.error.URLError as exc:
    exc.marker = marker
else:
    raise AssertionError("missing host did not raise URLError")
del marker
gc.collect()
assert observed_url() is None
print("urllib error owners released", True)
