"""Purpose: differential coverage for os.path.realpath, os.path.commonpath,
os.path.commonprefix, os.path.samefile, os.path.getsize, os.path.getatime,
os.path.getmtime, os.path.getctime."""

import os
import tempfile

# realpath
print("realpath /tmp:", type(os.path.realpath("/tmp")).__name__)
# commonpath
print("commonpath:", os.path.commonpath(["/usr/bin", "/usr/lib"]))
# commonprefix
print("commonprefix:", os.path.commonprefix(["/usr/bin", "/usr/lib"]))
# Create a temp file for stat tests
with tempfile.NamedTemporaryFile(delete=False) as f:
    f.write(b"hello")
    path = f.name
print("getsize:", os.path.getsize(path))
print("getatime type:", type(os.path.getatime(path)).__name__)
print("getmtime type:", type(os.path.getmtime(path)).__name__)
print("getctime type:", type(os.path.getctime(path)).__name__)
print("samefile:", os.path.samefile(path, path))
os.unlink(path)
