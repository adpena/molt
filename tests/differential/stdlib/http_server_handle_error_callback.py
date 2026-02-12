# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: handle_error should run for handler exceptions and server should continue."""

import http.server
import threading
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/err":
            raise RuntimeError("handler_fail")
        body = b"OK"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *args) -> None:
        return None


class Server(http.server.HTTPServer):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.errors: list[str] = []

    def handle_error(self, request, client_address) -> None:
        self.errors.append(type(request).__name__)
        super().handle_error(request, client_address)


server = Server(("127.0.0.1", 0), Handler)
port = server.server_address[1]
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

try:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/err", timeout=1.0):
            print("unexpected_success")
    except Exception:
        print("error_seen")
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/ok", timeout=1.0) as resp:
        print(resp.status, resp.read().decode("ascii"))
    print("error_callbacks", len(server.errors))
finally:
    server.shutdown()
    thread.join(timeout=1.0)
    server.server_close()
