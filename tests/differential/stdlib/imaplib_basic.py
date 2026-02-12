# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for imaplib basic."""

import socketserver
import threading
import imaplib

class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"* OK IMAP4 ready
")
        while True:
            line = self.rfile.readline()
            if not line:
                break
            if b"LOGOUT" in line.upper():
                self.wfile.write(b"* BYE
")
                self.wfile.write(b"A001 OK LOGOUT completed
")
                break
            self.wfile.write(b"A001 OK
")

server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
client = imaplib.IMAP4(host, port)
resp, _ = client.logout()
print(resp.decode("ascii") if isinstance(resp, bytes) else resp)

server.shutdown()
server.server_close()
thread.join()
