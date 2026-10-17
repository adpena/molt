# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: stderr=exact
"""Purpose: buffered-reader physical temporary-file readinto.

This file keeps filesystem descriptor creation independent of raw-stream
protocol qualification in io_bufferedreader_protocol.py.
"""

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
