# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for ftplib basic."""

import socketserver
import threading
import ftplib

class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"220 ready
")
        while True:
            line = self.rfile.readline()
            if not line:
                break
            cmd = line.strip().upper()
            if cmd.startswith(b"USER"):
                self.wfile.write(b"331 ok
")
            elif cmd.startswith(b"PASS"):
                self.wfile.write(b"230 ok
")
            elif cmd.startswith(b"QUIT"):
                self.wfile.write(b"221 bye
")
                break
            else:
                self.wfile.write(b"200 ok
")

server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
ftp = ftplib.FTP()
ftp.connect(host, port, timeout=2)
ftp.login("user", "pass")
print(ftp.getwelcome().startswith("220"))
ftp.quit()

server.shutdown()
server.server_close()
thread.join()
