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
        location(frame),
    )


def location(frame):
    # f_lasti is a code-table coordinate, whose integer representation differs
    # between AOT source locations and CPython bytecode. Compare its meaning.
    offset = frame.f_lasti
    return list(frame.f_code.co_positions())[offset // 2]


print(sample(3))

try:
    frame = sys._getframe()
    print(frame.f_code.co_name, frame.f_lineno, location(frame))
except Exception as exc:
    print(type(exc).__name__, exc)
