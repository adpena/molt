# MOLT_ENV: MOLT_CAPABILITIES=net,select.select
"""Purpose: an accepted socket takes the default timeout, never the listener's.

CPython 3.12 gives the same answer on Linux and macOS for every listener mode,
although the OS hands out accepted sockets whose non-blocking flag differs by
platform (inherited on BSD and macOS, not on Linux).
"""

import select
import socket


def accepted_mode(listener_mode: object, default: float | None) -> str:
    socket.setdefaulttimeout(default)
    try:
        srv = socket.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        if listener_mode == "blocking":
            srv.setblocking(True)
        elif listener_mode == "nonblocking":
            srv.setblocking(False)
        else:
            srv.settimeout(listener_mode)
        client = socket.create_connection(srv.getsockname(), timeout=5)
        ready, _, _ = select.select([srv], [], [], 5)
        assert ready, "the connection never reached the listener"
        conn, _addr = srv.accept()
        result = f"timeout={conn.gettimeout()!r} blocking={conn.getblocking()}"
        conn.close()
        client.close()
        srv.close()
        return result
    finally:
        socket.setdefaulttimeout(None)


for listener_mode in ("blocking", "nonblocking", 1.5):
    for default in (None, 2.5):
        print(listener_mode, default, accepted_mode(listener_mode, default))
