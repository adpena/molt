"""Differential coverage for the filesystem encode/decode protocol."""

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


text = "raw.txt"
print("decode_str", os.fsdecode(text), os.fsdecode(text) is text)
print("decode_bytes", os.fsdecode(raw))
print("decode_pathlike_str", os.fsdecode(PathLikeStr()))
print("decode_pathlike_bytes", os.fsdecode(PathLikeBytes()))
for value in (b"name\xff.txt", b"\xed\xb3\xbf", b"\xf0\x9f\x8c\x8d"):
    try:
        decoded = os.fsdecode(value)
        print("decode_roundtrip", ascii(decoded), os.fsencode(decoded) == value)
    except UnicodeError as exc:
        print("decode_roundtrip_error", type(exc).__name__, str(exc))
for label, value in (
    ("bytearray", bytearray(b"abc")),
    ("int", 123),
    ("none", None),
    ("pathlike_bad", PathLikeBad()),
):
    try:
        os.fsdecode(value)
    except Exception as exc:
        print("decode_invalid", label, type(exc).__name__, str(exc))

failure = RuntimeError("filesystem path failure")


class FailingPath:
    def __fspath__(self):
        raise failure


for operation in (os.fsencode, os.fsdecode):
    try:
        operation(FailingPath())
    except RuntimeError as exc:
        print("path_callback_identity", exc is failure)

original_encoding = sys.getfilesystemencoding
original_errors = sys.getfilesystemencodeerrors
original_fspath = os.fspath
events = []


def replaced_encoding():
    events.append("encoding lookup")
    return "ascii"


def replaced_errors():
    events.append("errors lookup")
    return "strict"


class PolicyChangingPath:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        events.append("path callback")
        sys.getfilesystemencoding = replaced_encoding
        sys.getfilesystemencodeerrors = replaced_errors
        return self.value


try:
    for operation, value in ((os.fsencode, "caf\u00e9"), (os.fsdecode, b"caf\xc3\xa9")):
        sys.getfilesystemencoding = original_encoding
        sys.getfilesystemencodeerrors = original_errors
        events.clear()
        print("captured_policy", ascii(operation(PolicyChangingPath(value))), events)
    os.fspath = lambda value: value.value
    print("rebound_fspath", os.fsencode(PolicyChangingPath("changed")))
    print("rebound_fspath", os.fsdecode(PolicyChangingPath(b"changed")))
finally:
    os.fspath = original_fspath
    sys.getfilesystemencoding = original_encoding
    sys.getfilesystemencodeerrors = original_errors
