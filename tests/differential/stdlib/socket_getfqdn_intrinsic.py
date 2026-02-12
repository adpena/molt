"""Purpose: verify socket.getfqdn is intrinsic-backed with CPython-shaped return values."""

import socket


default_name = socket.getfqdn()
loopback_name = socket.getfqdn("127.0.0.1")

print(isinstance(default_name, str))
print(len(default_name) > 0)
print(isinstance(loopback_name, str))
print(len(loopback_name) > 0)
