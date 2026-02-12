# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io bufferedreader readinto."""

import io
import tempfile

with tempfile.NamedTemporaryFile(mode="wb+", delete=True) as handle:
    handle.write(b"abcd")
    handle.flush()
    handle.seek(0)
    reader = io.BufferedReader(handle)
    buf = bytearray(2)
    n = reader.readinto(buf)
    print(n)
    print(bytes(buf))
