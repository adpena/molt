# MOLT_META: backends=native,wasm
"""Purpose: a runtime warning raised while no warnings module is loaded is
filtered by CPython's default filters and written to the sys.stderr object."""

import io
import sys


def invert(value):
    return ~value


def displayed(function, *args):
    """The header lines `function` writes to a replaced sys.stderr."""
    buffer = io.StringIO()
    saved = sys.stderr
    sys.stderr = buffer
    try:
        function(*args)
    finally:
        sys.stderr = saved
    return [
        line for line in buffer.getvalue().splitlines() if not line.startswith("  ")
    ]


lines = displayed(invert, True)
print(len(lines))
location, category, message = lines[0].split(": ", 2)
print(location.rsplit(":", 1)[1], category, message[:34])
print(displayed(invert, False))
