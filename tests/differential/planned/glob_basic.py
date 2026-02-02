# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for glob basics."""

import glob
import os
import tempfile


with tempfile.TemporaryDirectory() as root:
    paths = [
        os.path.join(root, "a.txt"),
        os.path.join(root, "b.txt"),
        os.path.join(root, "c.log"),
    ]
    for path in paths:
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("ok")

    matches = glob.glob(os.path.join(root, "*.txt"))
    print(sorted(os.path.basename(p) for p in matches))
