# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket timeouts sockopts."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print(sock.gettimeout())
sock.settimeout(0.5)
print(sock.gettimeout())

sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
print(sock.getsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR) in (0, 1))

sock.close()
