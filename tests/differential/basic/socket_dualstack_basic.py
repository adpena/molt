# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket dualstack basic."""

import socket


if not socket.has_ipv6:
    print("no_ipv6")
else:
    sock = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    try:
        sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
        v6only = sock.getsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY)
        print("v6only", v6only)
    except Exception as exc:
        print(type(exc).__name__)
    sock.close()
