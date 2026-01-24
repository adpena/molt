"""Purpose: differential coverage for class pattern keyword attr errors."""


class Tricky:
    def __getattr__(self, name):
        raise RuntimeError(f"attr:{name}")


try:
    match Tricky():
        case Tricky(x=1):
            print("hit")
        case _:
            print("miss")
except Exception as exc:
    print("error", type(exc).__name__)
