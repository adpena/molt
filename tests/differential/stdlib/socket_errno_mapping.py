# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket errno mapping."""

import errno
import socket


try:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(0.01)
    sock.connect(("10.255.255.1", 65535))
except Exception as exc:
    if isinstance(exc, OSError):
        print(
            exc.errno in (errno.EHOSTUNREACH, errno.ETIMEDOUT, errno.ECONNREFUSED, None)
        )
    else:
        print(type(exc).__name__)
finally:
    try:
        sock.close()
    except Exception:
        pass
