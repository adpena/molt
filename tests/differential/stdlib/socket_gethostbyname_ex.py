"""Purpose: differential coverage for socket.gethostbyname_ex."""

import socket

try:
    result = socket.gethostbyname_ex("localhost")
    print(isinstance(result, tuple))
    print(len(result) == 3)
    print(isinstance(result[0], str))
    print(isinstance(result[1], list))
    print(isinstance(result[2], list))
    print(any(addr == "127.0.0.1" for addr in result[2]))
except Exception as exc:
    print(type(exc).__name__)

try:
    socket.gethostbyname_ex("this.hostname.does.not.exist.invalid")
    print("no error raised")
except socket.herror:
    print("herror")
except socket.gaierror:
    print("gaierror")
except OSError:
    print("OSError")
