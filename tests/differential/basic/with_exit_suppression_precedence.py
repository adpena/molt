"""Purpose: differential coverage for __exit__ suppression precedence."""


class SuppressThenRaise:
    def __enter__(self):
        return "ok"

    def __exit__(self, exc_type, exc, tb):
        raise RuntimeError("exit")


class Suppress:
    def __enter__(self):
        return "ok"

    def __exit__(self, exc_type, exc, tb):
        return True


try:
    with SuppressThenRaise():
        raise ValueError("boom")
except Exception as exc:
    print("raise_overrides", type(exc).__name__)

try:
    with Suppress():
        raise ValueError("boom")
    print("suppressed", "ok")
except Exception as exc:
    print("suppressed", type(exc).__name__)
