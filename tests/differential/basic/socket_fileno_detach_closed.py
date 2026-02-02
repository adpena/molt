"""Purpose: differential coverage for fileno/close/detach."""

import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
fd = sock.fileno()
print(fd >= 0)

raw_fd = sock.detach()
print(isinstance(raw_fd, int), sock.fileno())

wrapped = socket.socket(fileno=raw_fd)
print(wrapped.fileno() == raw_fd)
wrapped.close()

sock2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock2.close()
print(sock2.fileno())
