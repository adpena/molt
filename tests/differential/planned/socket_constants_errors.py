# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket constants errors."""

import socket


print("error_alias", socket.error is OSError)
print("timeout_alias", socket.timeout is TimeoutError)
print("gaierror", issubclass(socket.gaierror, OSError))
print("herror", issubclass(socket.herror, OSError))

print("AF_INET", isinstance(socket.AF_INET, int))
print("SOCK_STREAM", isinstance(socket.SOCK_STREAM, int))
print("SOCK_DGRAM", isinstance(socket.SOCK_DGRAM, int))
print("SOL_SOCKET", isinstance(socket.SOL_SOCKET, int))
print("SO_REUSEADDR", isinstance(socket.SO_REUSEADDR, int))
print("IPPROTO_TCP", isinstance(socket.IPPROTO_TCP, int))
