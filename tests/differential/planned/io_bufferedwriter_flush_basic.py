# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io bufferedwriter flush basic."""

import io
import tempfile

with tempfile.NamedTemporaryFile(mode="wb+", delete=True) as handle:
    writer = io.BufferedWriter(handle)
    writer.write(b"abc")
    writer.flush()
    handle.seek(0)
    print(handle.read())
