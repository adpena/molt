# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for poplib basic."""

import socketserver
import threading
import poplib

class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"+OK ready
")
        while True:
            line = self.rfile.readline()
            if not line:
                break
            cmd = line.strip().upper()
            if cmd.startswith(b"QUIT"):
                self.wfile.write(b"+OK bye
")
                break
            else:
                self.wfile.write(b"+OK
")

server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
client = poplib.POP3(host, port, timeout=2)
print(client.getwelcome().startswith(b"+OK"))
client.quit()

server.shutdown()
server.server_close()
thread.join()
