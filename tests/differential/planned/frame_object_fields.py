"""Purpose: differential coverage for frame object fields."""

import inspect
import sys


def sample(value: int) -> tuple:
    frame = inspect.currentframe()
    if frame is None:
        return ("none",)
    return (
        frame.f_code.co_name,
        frame.f_lineno,
        "value" in frame.f_locals,
        "__name__" in frame.f_globals,
        getattr(frame, "f_lasti", None),
    )


print(sample(3))

try:
    frame = sys._getframe()
    print(frame.f_code.co_name, frame.f_lineno, getattr(frame, "f_lasti", None))
except Exception as exc:
    print(type(exc).__name__, exc)
