# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket nonblocking connect."""

import errno
import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setblocking(False)
err = sock.connect_ex(("127.0.0.1", 9))
print(
    err
    in (0, errno.EINPROGRESS, errno.ECONNREFUSED, errno.ECONNRESET, errno.EHOSTUNREACH)
)
sock.close()
