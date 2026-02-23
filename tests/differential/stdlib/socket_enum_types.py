"""Purpose: differential coverage for AddressFamily, SocketKind, MsgFlag IntEnum/IntFlag."""

import socket

# AddressFamily
print(hasattr(socket, "AddressFamily"))
af = socket.AddressFamily(socket.AF_INET)
print(type(af).__name__)
print(int(af) == socket.AF_INET)
print(isinstance(af, int))
print(af.name == "AF_INET")

# SocketKind
print(hasattr(socket, "SocketKind"))
sk = socket.SocketKind(socket.SOCK_STREAM)
print(type(sk).__name__)
print(int(sk) == socket.SOCK_STREAM)
print(isinstance(sk, int))
print(sk.name == "SOCK_STREAM")

# MsgFlag
print(hasattr(socket, "MsgFlag"))
mf = socket.MsgFlag(socket.MSG_PEEK)
print(type(mf).__name__)
print(int(mf) == socket.MSG_PEEK)
print(isinstance(mf, int))
print(mf.name == "MSG_PEEK")

# Enum iteration
af_names = [m.name for m in socket.AddressFamily]
print("AF_INET" in af_names)
print("AF_INET6" in af_names)

sk_names = [m.name for m in socket.SocketKind]
print("SOCK_STREAM" in sk_names)
print("SOCK_DGRAM" in sk_names)
