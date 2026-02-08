"""Purpose: validate intrinsic-backed ContextDecorator call semantics."""

import contextlib


class Recorder(contextlib.ContextDecorator):
    def __init__(self, suppress: bool = False) -> None:
        self.suppress = suppress
        self.events: list[str] = []

    def __enter__(self):
        self.events.append("enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        self.events.append("none" if exc_type is None else exc_type.__name__)
        return self.suppress and exc_type is ValueError


rec = Recorder()


@rec
def plus_one(value: int) -> int:
    rec.events.append(f"body:{value}")
    return value + 1


print(plus_one(4) == 5)
print(rec.events == ["enter", "body:4", "none"])


rec_suppress = Recorder(suppress=True)


@rec_suppress
def boom() -> None:
    rec_suppress.events.append("body")
    raise ValueError("expected")


print(boom() is None)
print(rec_suppress.events == ["enter", "body", "ValueError"])
