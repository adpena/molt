"""Purpose: differential coverage for getaddrinfo/getnameinfo flags + IDNA."""

import socket

flags = 0
for name in ("AI_ADDRCONFIG", "AI_V4MAPPED", "AI_PASSIVE"):
    if hasattr(socket, name):
        flags |= getattr(socket, name)

info = socket.getaddrinfo("localhost", 80, 0, 0, 0, flags)
print(len(info) > 0)

if hasattr(socket, "NI_NUMERICHOST") and hasattr(socket, "NI_NUMERICSERV"):
    host, serv = socket.getnameinfo(
        ("127.0.0.1", 80), socket.NI_NUMERICHOST | socket.NI_NUMERICSERV
    )
    print(host, serv)

host = "b\u00fccher.example"
try:
    socket.getaddrinfo(host, 80, flags=getattr(socket, "AI_NUMERICHOST", 0))
except socket.gaierror as exc:
    print("gaierror", exc.errno)
