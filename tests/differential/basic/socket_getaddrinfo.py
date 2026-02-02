# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket getaddrinfo."""

import socket


info = socket.getaddrinfo("localhost", 0)
print("count", len(info))
print("shape", all(len(item) == 5 for item in info))

families = {item[0] for item in info}
print("has_inet", socket.AF_INET in families or socket.AF_INET6 in families)
