# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for tempfile namedtempfile basic."""

import tempfile

with tempfile.NamedTemporaryFile(delete=True) as handle:
    handle.write(b"hi")
    handle.flush()
    print(handle.name is not None)
