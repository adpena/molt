"""Purpose: readline size counts decoded characters for utf-8 text."""

import os
import tempfile

content = "éé\n"
tmp = tempfile.NamedTemporaryFile(delete=False)
path = tmp.name
tmp.close()

try:
    with open(path, "w", encoding="utf-8") as f:
        f.write(content)
    with open(path, "r", encoding="utf-8") as f:
        print(repr(f.readline(1)))
        f.seek(0)
        print(repr(f.readline(2)))
        f.seek(0)
        print(repr(f.readline(3)))
        f.seek(0)
        print(repr(f.readline(4)))
finally:
    os.unlink(path)
