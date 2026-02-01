# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for http client server basic more."""

import http.client
import http.server
import threading

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
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
conn = http.client.HTTPConnection(host, port, timeout=2)
conn.request("GET", "/")
resp = conn.getresponse()
print(resp.status)
print(resp.read().decode("ascii"))
conn.close()

server.shutdown()
server.server_close()
thread.join()
