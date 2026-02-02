# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket.connect_ex result codes."""

import errno
import socket


tmp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
tmp.bind(("127.0.0.1", 0))
port = tmp.getsockname()[1]
tmp.close()

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
code = sock.connect_ex(("127.0.0.1", port))
sock.close()
print(code, errno.errorcode.get(code))
