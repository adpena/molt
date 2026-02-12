# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket linger struct."""

import socket
import struct


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    linger = struct.pack("ii", 1, 0)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, linger)
    raw = sock.getsockopt(socket.SOL_SOCKET, socket.SO_LINGER, 8)
    onoff, linger_val = struct.unpack("ii", raw)
    print(onoff in (0, 1), linger_val >= 0)
except Exception as exc:
    print(type(exc).__name__)

sock.close()
