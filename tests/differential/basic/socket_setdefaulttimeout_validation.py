"""Purpose: differential coverage for socket.setdefaulttimeout validation."""

import socket


def _show_timeout(tag: str) -> None:
    value = socket.getdefaulttimeout()
    print(tag, value, type(value).__name__)


socket.setdefaulttimeout(None)
_show_timeout("default_none")

socket.setdefaulttimeout(0)
_show_timeout("default_zero")
s0 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print("socket_zero_timeout", s0.gettimeout())
s0.close()

socket.setdefaulttimeout(1.25)
_show_timeout("default_pos")
s1 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
print("socket_pos_timeout", s1.gettimeout())
s1.close()

for bad in (-1, -0.1, "x"):
    try:
        socket.setdefaulttimeout(bad)  # type: ignore[arg-type]
        print("bad_unexpected", repr(bad))
    except Exception as exc:
        print("bad_exc", repr(bad), type(exc).__name__)

socket.setdefaulttimeout(None)
_show_timeout("default_reset")
