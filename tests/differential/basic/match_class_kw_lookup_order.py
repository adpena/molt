"""Purpose: differential coverage for keyword lookup order in class patterns."""


class Tricky:
    def __getattribute__(self, name):
        if name == "x":
            raise RuntimeError("getattribute")
        return object.__getattribute__(self, name)

    def __getattr__(self, name):
        raise RuntimeError("getattr")


try:
    match Tricky():
        case Tricky(x=1):
            print("hit")
        case _:
            print("miss")
except Exception as exc:
    print("error", type(exc).__name__, str(exc))
