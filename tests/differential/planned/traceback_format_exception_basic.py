"""Purpose: differential coverage for traceback format_exception."""

import traceback

try:
    1 / 0
except Exception as exc:
    lines = traceback.format_exception(type(exc), exc, exc.__traceback__)
    print(any("ZeroDivisionError" in line for line in lines))
