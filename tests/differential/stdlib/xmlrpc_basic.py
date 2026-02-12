# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for xmlrpc basic."""

from xmlrpc.server import SimpleXMLRPCServer
from xmlrpc.client import ServerProxy
import threading

server = SimpleXMLRPCServer(("127.0.0.1", 0), logRequests=False)
server.register_function(lambda x, y: x + y, "add")
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
proxy = ServerProxy(f"http://{host}:{port}")
print(proxy.add(2, 3))

server.shutdown()
server.server_close()
thread.join()
