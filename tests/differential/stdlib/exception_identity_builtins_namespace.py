"""Purpose: module-owned exception classes never appear in the builtins namespace."""

import asyncio
import builtins
import io
import pickle
import socket
import statistics


def main():
    for name in (
        "CancelledError",
        "UnsupportedOperation",
        "PickleError",
        "PicklingError",
        "UnpicklingError",
        "gaierror",
        "herror",
        "StatisticsError",
        "_DeadlockError",
        "InvalidStateError",
        "QueueEmpty",
    ):
        print("builtins has", name, hasattr(builtins, name), name in dir(builtins))
    print(
        "CancelledError",
        asyncio.CancelledError.__module__,
        asyncio.CancelledError.__mro__[1].__name__,
    )
    print(
        "UnsupportedOperation",
        io.UnsupportedOperation.__module__,
        [cls.__name__ for cls in io.UnsupportedOperation.__mro__[1:3]],
    )
    print("PickleError", pickle.PickleError.__module__)
    print("gaierror", socket.gaierror.__module__, socket.gaierror.__mro__[1].__name__)
    print("StatisticsError", statistics.StatisticsError.__module__)
    try:
        io.BytesIO(b"x").fileno()
    except io.UnsupportedOperation as exc:
        print("fileno", type(exc) is io.UnsupportedOperation, isinstance(exc, OSError))
    try:
        statistics.mean([])
    except statistics.StatisticsError as exc:
        print("mean", type(exc) is statistics.StatisticsError, isinstance(exc, ValueError))


if __name__ == "__main__":
    main()
