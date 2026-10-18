"""Purpose: bytes-pattern admission through the public re subject family.

Oracle: CPython 3.12.13, Lib/re/__init__.py and Modules/_sre/sre.c
(https://github.com/python/cpython/blob/v3.12.13/Modules/_sre/sre.c).
This fixture exercises compiled semantics; the host-side facade unit tests
explicitly do not claim native parser coverage.
"""

import re
from array import array


pattern = re.compile(b"(?P<word>\\w+)")
print(pattern.search(b"\xffA9!").group(0))
print(pattern.findall(memoryview(b"A1,FF")))
print(re.search(b"[^0-9A-F]", array("B", b"AF1!")).group())

for byte_pattern in (b"\\u0041", b"\\U00000041", b"\\N{LATIN CAPITAL LETTER A}"):
    try:
        re.compile(byte_pattern)
    except Exception as exc:
        print(type(exc).__name__, str(exc))
print(re.search(b"[A-Z]", b"\xe4", re.IGNORECASE))
print(re.sub(b"(a)", b"\\141\\1", b"a"))
print(re.sub(b"(a)", b"\\n", b"a"))
for replacement in (b"\\z", b"\\g<missing>", b"\\9"):
    try:
        re.sub(b"(a)", replacement, b"a")
    except Exception as exc:
        print(type(exc).__name__, str(exc))

subject = bytearray(b"xA1!")
match = re.search(b"(?P<hex>[0-9A-F]+)", subject)
print(match.string is subject, match.group("hex"))
subject[1:3] = b"B2"
print(match.group(0), match.groups(), match.groupdict())


class TrappingBytes(bytes):
    def __len__(self):
        raise AssertionError("subclass __len__ was called")

    def __getitem__(self, key):
        raise AssertionError("subclass __getitem__ was called")


class TrappingStr(str):
    def __len__(self):
        raise AssertionError("subclass __len__ was called")

    def __getitem__(self, key):
        raise AssertionError("subclass __getitem__ was called")


print(re.search(b"a", TrappingBytes(b"ba")).span())
print(re.search("a", TrappingStr("ba")).span())
print([match.span() for match in re.finditer(b"|a", b"a")])
print([match.span() for match in re.finditer("|a", "a")])
print([match.span() for match in re.finditer(b"", b"a")])

live_subject = bytearray(b"ab")
live_matches = re.finditer(b"a", live_subject)
print(next(live_matches).span())
live_subject[1] = ord("a")
print(next(live_matches).span())
try:
    live_subject.append(0)
except Exception as exc:
    print(type(exc).__name__, str(exc))

for operation in (
    lambda: re.search(b"a", "a"),
    lambda: re.search("a", b"a"),
    lambda: re.compile(b"a", re.UNICODE),
):
    try:
        operation()
    except Exception as exc:
        print(type(exc).__name__, str(exc))

for value in ("a b", b"a b", bytearray(b"a b"), "a\x00b", b"a\x00b"):
    print("escape", re.escape(value))
for expression, value in (("(a)?b", "b"), (b"(a)?b", b"b")):
    print("unmatched findall", re.findall(expression, value))
for limit in (-1, 0, 1):
    for expression, replacement, value in (("a", "X", "aa"), (b"a", b"X", b"aa")):
        print("limits", limit, re.split(expression, value, maxsplit=limit))
        print(re.subn(expression, replacement, value, count=limit))
        print(re.subn(expression, lambda match: replacement, value, count=limit))
