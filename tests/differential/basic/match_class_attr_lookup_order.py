"""Purpose: differential coverage for class pattern attribute lookup order."""


class Tricky:
    __match_args__ = ("x", "y")

    def __getattribute__(self, name):
        if name == "x":
            raise RuntimeError("x")
        return object.__getattribute__(self, name)

    def __init__(self):
        self.x = 1
        self.y = 2


try:
    match Tricky():
        case Tricky(1, 2):
            print("hit")
        case _:
            print("miss")
except Exception as exc:
    print("error", type(exc).__name__)
