"""Purpose: differential coverage for traceback basic."""

import traceback


def boom():
    raise ValueError("bad")


try:
    boom()
except Exception as exc:
    lines = traceback.format_exception(type(exc), exc, exc.__traceback__)
    print(lines[-1].strip())
