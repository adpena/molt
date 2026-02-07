"""Purpose: ensure callee return-time context unwind never pops caller-owned contexts."""

import os
import tempfile


def _read_marker(path: str) -> str:
    with open(path, "r", encoding="utf-8") as handle:
        return handle.read().strip()


with tempfile.TemporaryDirectory() as tmp:
    src = os.path.join(tmp, "src.txt")
    with open(src, "w", encoding="utf-8") as handle:
        handle.write("ok\\n")

    print(_read_marker(src))
    print(os.path.exists(tmp))

    dst = os.path.join(tmp, "dst.txt")
    with open(dst, "w", encoding="utf-8") as handle:
        handle.write("done\\n")
    print(os.path.exists(dst))
