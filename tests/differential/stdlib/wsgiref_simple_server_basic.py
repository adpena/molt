# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for wsgiref simple server basic."""

from wsgiref.simple_server import make_server
import threading
import urllib.request


def app(environ, start_response):
    start_response("200 OK", [("Content-Type", "text/plain")])
    return [b"ok"]

server = make_server("127.0.0.1", 0, app)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
with urllib.request.urlopen(f"http://{host}:{port}/") as resp:
    print(resp.read().decode("ascii"))

server.shutdown()
thread.join()
