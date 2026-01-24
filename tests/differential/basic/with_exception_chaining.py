"""Purpose: differential coverage for with exception chaining semantics."""


class ExitWrap:
    def __enter__(self):
        return "ok"

    def __exit__(self, exc_type, exc, tb):
        raise RuntimeError("exit")


try:
    with ExitWrap():
        raise KeyError("boom")
except Exception as exc:
    print("exc", type(exc).__name__)
    print("context", type(exc.__context__).__name__)
    print("cause", exc.__cause__)
