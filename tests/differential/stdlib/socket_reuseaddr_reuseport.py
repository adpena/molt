# MOLT_ENV: MOLT_CAPABILITIES=net.listen
"""Purpose: differential coverage for SO_REUSEADDR/SO_REUSEPORT."""

import socket


sock1 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock1.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock1.bind(("127.0.0.1", 0))
port = sock1.getsockname()[1]
sock1.listen(1)
print("reuseaddr", sock1.getsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR) in (0, 1))

sock2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock2.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
reuseport = getattr(socket, "SO_REUSEPORT", None)
if reuseport is not None:
    try:
        sock2.setsockopt(socket.SOL_SOCKET, reuseport, 1)
        print("reuseport", sock2.getsockopt(socket.SOL_SOCKET, reuseport) in (0, 1))
    except OSError as exc:
        print("reuseport", type(exc).__name__)

try:
    sock2.bind(("127.0.0.1", port))
    print("second_bind", True)
except OSError as exc:
    print("second_bind", type(exc).__name__)

sock2.close()
sock1.close()
