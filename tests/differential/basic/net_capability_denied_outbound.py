# MOLT_ENV: MOLT_CAPABILITIES=
# MOLT_META: expect_fail=molt expect_fail_reason=requires_network_capability
"""Purpose: differential coverage for net capability denied outbound."""

import socket


try:
    sock = socket.create_connection(("example.com", 80), timeout=1.0)
    sock.close()
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
