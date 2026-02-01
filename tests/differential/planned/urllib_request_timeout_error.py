# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for urllib request timeout error."""

import http.server
import threading
import time
import urllib.request
import urllib.error

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        time.sleep(0.5)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, format, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
try:
    urllib.request.urlopen(f"http://{host}:{port}/", timeout=0.1)
except Exception as exc:
    print(type(exc).__name__)

server.shutdown()
server.server_close()
thread.join()
