# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io textiowrapper write newline none."""

import tempfile

with tempfile.NamedTemporaryFile(mode="w+", newline=None, delete=True) as handle:
    handle.write("a
")
    handle.seek(0)
    print(repr(handle.read()))

with tempfile.NamedTemporaryFile(mode="w+", newline="", delete=True) as handle:
    handle.write("a
")
    handle.seek(0)
    print(repr(handle.read()))
