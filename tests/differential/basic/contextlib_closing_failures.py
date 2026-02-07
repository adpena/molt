import contextlib


class MissingClose:
    pass


try:
    with contextlib.closing(MissingClose()):
        pass
except Exception as exc:
    print("missing", type(exc).__name__)


class NonCallableClose:
    close = 1


try:
    with contextlib.closing(NonCallableClose()):
        pass
except Exception as exc:
    print("noncallable", type(exc).__name__)
