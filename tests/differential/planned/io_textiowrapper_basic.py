# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io textiowrapper basic."""

import io
import tempfile

with tempfile.NamedTemporaryFile(mode="w+", encoding="utf-8", delete=True) as handle:
    handle.write("hello
")
    handle.seek(0)
    print(handle.readline().strip())
