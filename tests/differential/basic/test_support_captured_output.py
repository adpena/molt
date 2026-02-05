"""Purpose: validate captured output via contextlib redirectors."""

import contextlib
import io
import sys

stdout_buf = io.StringIO()
stderr_buf = io.StringIO()

with contextlib.redirect_stdout(stdout_buf), contextlib.redirect_stderr(stderr_buf):
    print("hello")
    print("oops", file=sys.stderr)

print(stdout_buf.getvalue().strip())
print(stderr_buf.getvalue().strip())
