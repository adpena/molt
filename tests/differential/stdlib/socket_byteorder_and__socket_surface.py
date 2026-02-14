"""Purpose: CPython-parity for socket byte-order helpers + `_socket` surface."""

import socket


def _call(name: str, fn, arg) -> None:
    try:
        print(name, repr(arg), fn(arg))
    except Exception as exc:
        print(name, repr(arg), type(exc).__name__, str(exc))


for v in [0, 1, 0x1234, 0xFFFF, 0x1FFFF, -1, "x", 1.5, True]:
    _call("htons", socket.htons, v)
    _call("ntohs", socket.ntohs, v)

for v in [0, 1, 0x12345678, 0xFFFFFFFF, 0x1FFFFFFFF, -1, "x", 1.5, True]:
    _call("htonl", socket.htonl, v)
    _call("ntohl", socket.ntohl, v)


import _socket  # noqa: E402

print("_socket.SocketType_is_socket", _socket.SocketType is _socket.socket)
print("_socket.error_is_OSError", _socket.error is OSError)
print("_socket.htons_eq_socket", _socket.htons(1) == socket.htons(1))
print("_socket.ntohl_eq_socket", _socket.ntohl(1) == socket.ntohl(1))
