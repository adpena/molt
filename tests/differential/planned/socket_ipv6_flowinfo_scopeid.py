# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for IPv6 flowinfo/scope_id."""

import socket

if not socket.has_ipv6:
    print("no_ipv6")
else:
    sock = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    sock.bind(("::1", 0, 0, 0))
    addr = sock.getsockname()
    print(len(addr), addr[2], addr[3])
    sock.close()
