# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: serve_forever/shutdown lifecycle should terminate the loop cleanly."""

import http.server
import threading


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(204)
        self.end_headers()

    def log_message(self, _format: str, *args) -> None:
        return None


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
thread.daemon = True
thread.start()

server.shutdown()
server.shutdown()
thread.join(timeout=2.0)
print("joined", not thread.is_alive())
server.server_close()
