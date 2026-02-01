# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io textiowrapper newline none."""

import tempfile

with tempfile.NamedTemporaryFile(mode="w+", newline=None, delete=True) as handle:
    handle.write("a
b
")
    handle.seek(0)
    print(handle.read().splitlines())
