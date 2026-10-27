# MOLT_ENV: MOLT_CAPABILITIES=net
"""Purpose: differential coverage for socket constants errors."""

import socket
import _socket
import builtins


print("error_alias", socket.error is OSError)
print("timeout_alias", socket.timeout is TimeoutError)
print("gaierror", issubclass(socket.gaierror, OSError))
print("herror", issubclass(socket.herror, OSError))

print("AF_INET", isinstance(socket.AF_INET, int))
print("SOCK_STREAM", isinstance(socket.SOCK_STREAM, int))
print("SOCK_DGRAM", isinstance(socket.SOCK_DGRAM, int))
print("SOL_SOCKET", isinstance(socket.SOL_SOCKET, int))
print("SO_REUSEADDR", isinstance(socket.SO_REUSEADDR, int))
print("IPPROTO_TCP", isinstance(socket.IPPROTO_TCP, int))

print("gaierror_identity", socket.gaierror is _socket.gaierror)
print("herror_identity", socket.herror is _socket.herror)
print("gaierror_module", socket.gaierror.__module__)
print("herror_module", socket.herror.__module__)
print("not_builtin", not hasattr(builtins, "gaierror") and not hasattr(builtins, "herror"))
for cls in (socket.gaierror, socket.herror):
    value = cls(9, "lookup detail")
    print("lookup_layout", type(value) is cls, value.errno, value.strerror, value.args)
