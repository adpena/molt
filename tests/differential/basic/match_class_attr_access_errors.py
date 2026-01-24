"""Purpose: differential coverage for class pattern attribute access errors."""


class Tricky:
    __match_args__ = ("x",)

    @property
    def x(self):
        raise RuntimeError("boom")


try:
    match Tricky():
        case Tricky(1):
            print("hit")
        case _:
            print("miss")
except Exception as exc:
    print("error", type(exc).__name__)
