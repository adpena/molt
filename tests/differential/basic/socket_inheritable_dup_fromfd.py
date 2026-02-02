"""Purpose: differential coverage for inheritable + dup/fromfd."""

import os
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print(sock.get_inheritable())

sock.set_inheritable(True)
print(sock.get_inheritable())

sock.set_inheritable(False)
print(sock.get_inheritable())

fd = os.dup(sock.fileno())
clone = socket.fromfd(fd, socket.AF_INET, socket.SOCK_STREAM)
os.close(fd)
print(clone.fileno() != sock.fileno())

clone.close()
sock.close()
