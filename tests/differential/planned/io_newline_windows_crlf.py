# MOLT_META: platforms=windows
# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Behavior: newline=None translates LF writes to CRLF on disk and reads back LF.
Why: newline translation must mirror CPython's text IO on Windows.
Pitfalls: NamedTemporaryFile reopen behavior differs on Windows; use delete=False.
"""

import os
import tempfile

with tempfile.NamedTemporaryFile(mode="w", newline=None, delete=False) as handle:
    path = handle.name
    handle.write("a\n")

with open(path, "rb") as raw:
    print(raw.read())

with open(path, "r", newline=None) as handle:
    print(repr(handle.read()))

os.unlink(path)
