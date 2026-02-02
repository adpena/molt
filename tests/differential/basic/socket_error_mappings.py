# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket error mappings."""

import socket


try:
    socket.getaddrinfo("no-such-host.invalid", 80)
except Exception as exc:
    print(type(exc).__name__)

try:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.01)
    s.connect(("10.255.255.1", 65535))
except Exception as exc:
    print(type(exc).__name__)
finally:
    try:
        s.close()
    except Exception:
        pass
