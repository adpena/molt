"""Purpose: differential coverage for inspect frame object parity."""

import inspect
import sys


def sample(x: int) -> tuple:
    frame = inspect.currentframe()
    if frame is None:
        return ("none",)
    return (
        frame.f_code.co_name,
        frame.f_lineno,
        frame.f_back.f_code.co_name if frame.f_back else None,
        "x" in frame.f_locals,
        "__name__" in frame.f_globals,
    )


print(sample(3))

try:
    frame = sys._getframe()
    print(frame.f_code.co_name, frame.f_lineno)
except Exception as exc:
    print(type(exc).__name__, exc)
