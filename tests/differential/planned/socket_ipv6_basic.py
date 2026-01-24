# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket ipv6 basic."""

import socket


if not socket.has_ipv6:
    print("no_ipv6")
else:
    sock = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    sock.bind(("::1", 0))
    addr = sock.getsockname()
    print(addr[0] in ("::1", "0:0:0:0:0:0:0:1"), isinstance(addr[1], int))
    sock.close()
