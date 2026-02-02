"""Purpose: differential coverage for socket.detach/fileno."""

import os
import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
before = sock.fileno()
fd = sock.detach()
after = sock.fileno()
print(before >= 0, fd >= 0, after)
os.close(fd)
