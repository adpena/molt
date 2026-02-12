# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io textiowrapper newlines."""

import tempfile

with tempfile.NamedTemporaryFile(mode="w+", newline="\n", delete=True) as handle:
    handle.write("a\n")
    handle.write("b\n")
    handle.seek(0)
    print(handle.read().splitlines())
