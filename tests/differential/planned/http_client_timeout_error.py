# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for http client timeout error."""

import http.client
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
sock.listen(1)

host, port = sock.getsockname()
conn = http.client.HTTPConnection(host, port, timeout=0.1)
try:
    conn.request("GET", "/")
    conn.getresponse()
except Exception as exc:
    print(type(exc).__name__)

conn.close()
sock.close()
