# MOLT_META: backends=llvm,luau,native
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for imaplib basic."""

import socketserver
import threading
import imaplib


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"* OK IMAP4 ready\r\n")
        while True:
            line = self.rfile.readline()
            if not line:
                break
            tag = line.split(maxsplit=1)[0]
            command = line.upper()
            if b"LOGOUT" in command:
                self.wfile.write(b"* BYE\r\n")
                self.wfile.write(tag + b" OK LOGOUT completed\r\n")
                break
            if b"CAPABILITY" in command:
                self.wfile.write(b"* CAPABILITY IMAP4rev1\r\n")
                self.wfile.write(tag + b" OK CAPABILITY completed\r\n")
                continue
            self.wfile.write(tag + b" OK\r\n")


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
