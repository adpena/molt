# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib proxy auth handler."""

import socket
import threading
import urllib.request


ready = threading.Event()
port_holder: list[int] = []
headers_seen: list[bool] = []


def proxy() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = conn.recv(4096).decode("latin-1")
    headers_seen.append("Proxy-Authorization:" in data)
    conn.sendall(
        b"HTTP/1.1 407 Proxy Authentication Required\r\n"
        b'Proxy-Authenticate: Basic realm="test"\r\n'
        b"Content-Length: 0\r\n"
        b"Connection: close\r\n"
        b"\r\n"
    )
    conn.close()
    srv.close()


t = threading.Thread(target=proxy)
t.start()
ready.wait(timeout=1.0)

proxy_url = f"http://127.0.0.1:{port_holder[0]}"
password_mgr = urllib.request.HTTPPasswordMgrWithDefaultRealm()
password_mgr.add_password(None, proxy_url, "user", "pass")
handler = urllib.request.ProxyBasicAuthHandler(password_mgr)
opener = urllib.request.build_opener(
    handler, urllib.request.ProxyHandler({"http": proxy_url})
)

try:
    opener.open("http://example.com/", timeout=1.0)
except Exception as exc:
    print(type(exc).__name__)


t.join()
print("proxy_auth", headers_seen[0])
