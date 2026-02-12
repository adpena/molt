"""Purpose: differential coverage for HTTP upgrade headers end-to-end."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import http.client
import http.server
import socketserver
import threading


_KEY = "dGhlIHNhbXBsZSBub25jZQ=="


class _UpgradeHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        headers = {k.lower(): v for k, v in self.headers.items()}
        self.server.received_headers = headers
        self.send_response(101)
        self.end_headers()

    def log_message(self, format: str, *args: object) -> None:
        return None


class _OneShotServer(socketserver.TCPServer):
    allow_reuse_address = True


server_state: dict[str, object] = {}
ready = threading.Event()


def _serve_once() -> None:
    with _OneShotServer(("127.0.0.1", 0), _UpgradeHandler) as httpd:
        server_state["port"] = httpd.server_address[1]
        server_state["headers"] = {}
        ready.set()
        httpd.handle_request()
        server_state["headers"] = getattr(httpd, "received_headers", {})


thread = threading.Thread(target=_serve_once, daemon=True)
thread.start()
ready.wait()
port = int(server_state["port"])

conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
headers = {
    "Upgrade": "websocket",
    "Connection": "Upgrade",
    "Sec-WebSocket-Key": _KEY,
    "Sec-WebSocket-Version": "13",
}
conn.request("GET", "/", headers=headers)
resp = conn.getresponse()
resp.read()
conn.close()
thread.join(timeout=5)

received = server_state.get("headers")
if not isinstance(received, dict):
    received = {}

print("status", resp.status)
print("upgrade", received.get("upgrade") == "websocket")
print("connection", "upgrade" in received.get("connection", "").lower())
print("sec_key", received.get("sec-websocket-key") == _KEY)
print("sec_ver", received.get("sec-websocket-version") == "13")
print("host", bool(received.get("host")))
