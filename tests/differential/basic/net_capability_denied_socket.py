# MOLT_ENV: MOLT_CAPABILITIES=
"""Purpose: differential coverage for net capability denied socket."""

import socket


try:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 0))
    sock.close()
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
