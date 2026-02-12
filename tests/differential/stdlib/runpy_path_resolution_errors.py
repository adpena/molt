"""Purpose: runpy.run_path resolution/type errors stay deterministic."""

import os
import runpy
import tempfile


class PathLikeStr:
    def __init__(self, path: str) -> None:
        self._path = path

    def __fspath__(self) -> str:
        return self._path


with tempfile.TemporaryDirectory() as tmp:
    missing = os.path.join(tmp, "missing.py")
    try:
        runpy.run_path(PathLikeStr(missing))
    except FileNotFoundError:
        print("missing-ok")

try:
    runpy.run_path(123)
except TypeError:
    print("nonpath-typeerror")
