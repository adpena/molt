# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound,env.read
"""Purpose: differential coverage for smtplib.SMTP.sendmail."""

from __future__ import annotations

import smtplib
import socketserver
import threading


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.wfile.write(b"220 ready\r\n")
        data_mode = False
        while True:
            line = self.rfile.readline()
            if not line:
                break
            if data_mode:
                if line.strip() == b".":
                    self.wfile.write(b"250 queued\r\n")
                    data_mode = False
                continue
            cmd = line.strip().upper()
            if cmd.startswith(b"HELO") or cmd.startswith(b"EHLO"):
                self.wfile.write(b"250 hello\r\n")
            elif cmd.startswith(b"MAIL FROM"):
                self.wfile.write(b"250 sender ok\r\n")
            elif cmd.startswith(b"RCPT TO:") and b"BAD@EXAMPLE.COM" in cmd:
                self.wfile.write(b"550 no such user\r\n")
            elif cmd.startswith(b"RCPT TO:"):
                self.wfile.write(b"250 rcpt ok\r\n")
            elif cmd == b"DATA":
                self.wfile.write(b"354 end data\r\n")
                data_mode = True
            elif cmd.startswith(b"QUIT"):
                self.wfile.write(b"221 bye\r\n")
                break
            else:
                self.wfile.write(b"250 ok\r\n")


server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

host, port = server.server_address
client = smtplib.SMTP(host, port, timeout=2)
refused = client.sendmail(
    "sender@example.com",
    ["good@example.com", "bad@example.com"],
    "Subject: test\r\n\r\nBody\r\n",
)
print(sorted(refused))
print(refused["bad@example.com"][0])
client.quit()

server.shutdown()
server.server_close()
thread.join()
