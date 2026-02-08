"""Purpose: validate socket error subclass mapping for intrinsic-backed lookups."""

import socket


def _capture_exc_name(fn):
    try:
        fn()
    except Exception as exc:  # parity capture for class mapping
        return type(exc).__name__
    return "NO_ERROR"


missing_host = "molt-does-not-exist.invalid"
missing_reverse_ip = "1.2.3.4"
invalid_numeric_host = "300.1.1.1"

print(_capture_exc_name(lambda: socket.getaddrinfo(missing_host, 80)))
print(_capture_exc_name(lambda: socket.gethostbyname(missing_host)))
print(_capture_exc_name(lambda: socket.gethostbyaddr(missing_host)))
print(_capture_exc_name(lambda: socket.gethostbyaddr(missing_reverse_ip)))
print(_capture_exc_name(lambda: socket.gethostbyaddr(invalid_numeric_host)))
