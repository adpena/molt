"""Purpose: differential coverage for AF_UNIX socketpair and PathLike."""

import socket
import tempfile


class _PathLike:
    def __init__(self, path: str):
        self._path = path

    def __fspath__(self) -> str:
        return self._path

if not hasattr(socket, "AF_UNIX"):
    print("no_unix")
else:
    try:
        a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    except Exception as exc:
        print(type(exc).__name__)
    else:
        a.sendall(b"hi")
        print(b.recv(2))
        a.close()
        b.close()

    try:
        a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
    except Exception as exc:
        print(type(exc).__name__)
    else:
        a.send(b"yo")
        print(b.recv(2))
        a.close()
        b.close()

    with tempfile.TemporaryDirectory() as tmp:
        path = _PathLike(tmp + "/sock")
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(path)
        print(isinstance(srv.getsockname(), str))
        srv.close()
