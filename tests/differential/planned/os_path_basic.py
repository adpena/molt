# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for os path basic."""

import os
import tempfile


tmpdir = tempfile.gettempdir()
path = os.path.join(tmpdir, "molt_os_path.txt")
with open(path, "w") as handle:
    handle.write("x")

print(os.path.exists(path))
print(os.path.basename(path))
print(os.path.dirname(path) == tmpdir)

os.remove(path)
