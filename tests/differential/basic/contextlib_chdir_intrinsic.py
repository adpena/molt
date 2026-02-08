"""Purpose: verify contextlib.chdir intrinsic-backed enter/exit semantics."""

import contextlib
import os
import tempfile


start = os.getcwd()
print(isinstance(start, str))

with tempfile.TemporaryDirectory() as tmpdir:
    with contextlib.chdir(tmpdir) as token:
        print(token is None)
        print(os.getcwd() == tmpdir)
        with contextlib.chdir(start):
            print(os.getcwd() == start)
        print(os.getcwd() == tmpdir)

print(os.getcwd() == start)
