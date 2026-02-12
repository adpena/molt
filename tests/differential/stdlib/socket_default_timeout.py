"""Purpose: differential coverage for socket default timeout."""

import socket

print(socket.getdefaulttimeout())

socket.setdefaulttimeout(1.5)
print(socket.getdefaulttimeout())

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print(sock.gettimeout())
sock.close()

socket.setdefaulttimeout(None)
print(socket.getdefaulttimeout())

sock2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print(sock2.gettimeout())
sock2.close()
