# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for select.select timeout with no ready fds."""

import select
import socket


try:
    left, right = socket.socketpair()
except AttributeError:
    left = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    right = socket.socket(socket.AF_INET, socket.SOCK_STREAM)

try:
    r, w, x = select.select([left], [], [], 0.05)
    print(len(r), len(w), len(x))
finally:
    left.close()
    right.close()
