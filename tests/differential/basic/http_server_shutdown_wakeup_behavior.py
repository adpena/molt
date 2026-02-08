# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: shutdown should wake serve_forever promptly even with long poll_interval."""

import http.server
import threading
import time


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(204)
        self.end_headers()

    def log_message(self, _format: str, *args) -> None:
        return None


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 5.0})
thread.daemon = True
thread.start()

time.sleep(0.05)
server.shutdown()
thread.join(timeout=1.0)
print("joined", not thread.is_alive())
server.server_close()
