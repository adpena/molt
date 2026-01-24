"""Purpose: differential coverage for with __exit__ suppression/raise behavior."""


class Suppress:
    def __enter__(self):
        return "ok"

    def __exit__(self, exc_type, exc, tb):
        print("exit", exc_type.__name__ if exc_type else None)
        return True


class RaiseExit:
    def __enter__(self):
        return "ok"

    def __exit__(self, exc_type, exc, tb):
        raise RuntimeError("exit")


class RaiseEnter:
    def __enter__(self):
        raise ValueError("enter")

    def __exit__(self, exc_type, exc, tb):
        return False


try:
    with Suppress() as value:
        print("body", value)
        raise KeyError("boom")
    print("after", "suppressed")
except Exception as exc:
    print("after", type(exc).__name__)

try:
    with RaiseExit():
        raise KeyError("boom")
except Exception as exc:
    print("raise_exit", type(exc).__name__, str(exc))

try:
    with RaiseEnter():
        print("never")
except Exception as exc:
    print("raise_enter", type(exc).__name__, str(exc))
