# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for urllib request basic more."""

import http.server
import threading
import urllib.request

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"hello")

    def log_message(self, format, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
with urllib.request.urlopen(f"http://{host}:{port}/") as resp:
    print(resp.read().decode("ascii"))

server.shutdown()
server.server_close()
thread.join()
