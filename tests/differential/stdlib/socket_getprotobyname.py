"""Purpose: differential coverage for socket.getprotobyname."""

import socket

print(socket.getprotobyname("tcp") == 6)
print(socket.getprotobyname("udp") == 17)
print(socket.getprotobyname("icmp") == 1)

try:
    socket.getprotobyname("nonexistent_protocol_xyz")
    print("no error")
except OSError:
    print("OSError raised")
