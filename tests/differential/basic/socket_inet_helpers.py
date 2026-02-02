# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket inet helpers."""

import socket


packed = socket.inet_pton(socket.AF_INET, "127.0.0.1")
print("packed_len", len(packed))
print("roundtrip", socket.inet_ntop(socket.AF_INET, packed))

try:
    v6 = socket.inet_pton(socket.AF_INET6, "::1")
    print("v6_len", len(v6))
    print("v6_roundtrip", socket.inet_ntop(socket.AF_INET6, v6))
except Exception as exc:
    print("v6_err", type(exc).__name__)
