"""Purpose: differential coverage for socket.sethostname error paths."""

import socket

# sethostname requires root, so test the EPERM error path
try:
    socket.sethostname("test-molt-hostname")
    print("sethostname ok")
except PermissionError:
    print("PermissionError")
except OSError as e:
    print("OSError raised")
    print(e.errno is not None)

# Verify sethostname exists and is callable
print(callable(socket.sethostname))
