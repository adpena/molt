"""Purpose: differential coverage for __getattribute__ vs __getattr__ in class patterns."""


class Tricky:
    __match_args__ = ("x",)

    def __getattribute__(self, name):
        if name == "x":
            raise RuntimeError("getattribute")
        return object.__getattribute__(self, name)

    def __getattr__(self, name):
        raise RuntimeError("getattr")


try:
    match Tricky():
        case Tricky(1):
            print("hit")
        case _:
            print("miss")
except Exception as exc:
    print("error", type(exc).__name__, str(exc))
