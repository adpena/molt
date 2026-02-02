"""Purpose: differential coverage for contextlib redirect stdout."""

import contextlib
import io


buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    print("hello")

print(buf.getvalue().strip())
