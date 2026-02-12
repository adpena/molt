"""Purpose: differential coverage for contextlib redirect stderr."""

import contextlib
import io
import sys


buf = io.StringIO()
with contextlib.redirect_stderr(buf):
    print("hello", file=sys.stderr)

print(buf.getvalue().strip())
