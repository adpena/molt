"""Purpose: differential coverage for MSG_DONTWAIT."""

import socket

if not hasattr(socket, "MSG_DONTWAIT"):
    print("no_flag")
else:
    try:
        a, b = socket.socketpair()
    except AttributeError:
        print("no_socketpair")
    else:
        try:
            a.recv(1, socket.MSG_DONTWAIT)
        except Exception as exc:
            print(type(exc).__name__)
        a.close()
        b.close()
