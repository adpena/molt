# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: buffered-writer physical temporary-file flush.

Raw-stream protocol qualification lives in io_bufferedwriter_protocol.py.
"""

import io
import tempfile

with tempfile.NamedTemporaryFile(mode="wb+", delete=True) as handle:
    writer = io.BufferedWriter(handle)
    writer.write(b"abc")
    writer.flush()
    handle.seek(0)
    print(handle.read())
