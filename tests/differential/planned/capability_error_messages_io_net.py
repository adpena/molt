"""Purpose: differential coverage for capability-gated I/O + network errors."""

import socket


def show(label: str, thunk) -> None:
    try:
        result = thunk()
        print(label, "ok", result)
    except Exception as exc:
        print(label, type(exc).__name__, exc)


def read_self() -> int:
    with open(__file__, "r", encoding="utf-8") as handle:
        return len(handle.readline())


def create_socket() -> str:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.close()
    return "closed"


show("open", read_self)
show("socket", create_socket)
