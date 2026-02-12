"""Purpose: differential coverage for socket.setblocking vs settimeout."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print(sock.gettimeout())
sock.setblocking(False)
print(sock.gettimeout())
sock.setblocking(True)
print(sock.gettimeout())
sock.settimeout(0.2)
print(sock.gettimeout())
sock.setblocking(False)
print(sock.gettimeout())
sock.close()
