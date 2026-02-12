# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io textiowrapper newline variants."""

import tempfile

with tempfile.NamedTemporaryFile(mode="w+", newline="", delete=True) as handle:
    handle.write("a\r\nb\n")
    handle.seek(0)
    print(handle.read())
