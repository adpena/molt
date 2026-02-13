"""Differential coverage for os.fsencode semantics."""

from __future__ import annotations

import os
import sys


class PathLikeStr:
    def __fspath__(self) -> str:
        return "pathlike.txt"


class PathLikeBytes:
    def __fspath__(self) -> bytes:
        return b"pathlike.bin"


class PathLikeBad:
    def __fspath__(self) -> int:
        return 123


raw = b"raw.bin"
print("fs_errors", sys.getfilesystemencodeerrors())
print("str", os.fsencode("alpha.txt"))
print("bytes", os.fsencode(raw))
print("bytes_identity", os.fsencode(raw) is raw)
print("pathlike_str", os.fsencode(PathLikeStr()))
print("pathlike_bytes", os.fsencode(PathLikeBytes()))

try:
    print("surrogate", os.fsencode("name\udcff.txt"))
except Exception as exc:
    print("surrogate_exc", type(exc).__name__, str(exc))

for label, value in (
    ("bytearray", bytearray(b"abc")),
    ("int", 123),
    ("none", None),
):
    try:
        os.fsencode(value)
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))

try:
    os.fsencode(PathLikeBad())
except Exception as exc:
    print("pathlike_bad", type(exc).__name__, str(exc))
