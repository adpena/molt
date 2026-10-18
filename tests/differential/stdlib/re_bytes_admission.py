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

# Saved spans remain valid after the mutable subject shrinks. Capture reads
# clamp both endpoints to the current storage, including replacement expansion.
shrinking = bytearray(b"ab")
shrunk_match = re.search(b"(?P<first>a)(?P<second>b)", shrinking)
shrinking.clear()
print("cleared captures", shrunk_match.span(), shrunk_match.group(), shrunk_match.groups(), shrunk_match.groupdict(), shrunk_match.expand(b"<\\1|\\2>"))
shrinking.extend(b"x")
print("short captures", shrunk_match.group(), shrunk_match.groups(), shrunk_match.groupdict(), shrunk_match.expand(b"<\\1|\\2>"))

surrogate_match = re.search("(?P<surrogate>.)", "\ud800A")
print("surrogate capture", repr(surrogate_match.group()), repr(surrogate_match.groups()), repr(surrogate_match.groupdict()), repr(surrogate_match.expand("<\\g<surrogate>>")))


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
text_match = re.search("(?P<letter>a)", TrappingStr("ba"))
print(text_match.group(), text_match.groups(), text_match.groupdict(), text_match.expand("[\\g<letter>]"))
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


# All SRE subjects enter the same PyBUF_SIMPLE role, including text-pattern
# mismatches. Exporter errors are replaced; the active handled error remains
# the resulting TypeError's context rather than the discarded exporter error.
def admission_result(label, operation):
    try:
        raise LookupError("outer admission handler")
    except LookupError as outer:
        try:
            operation()
        except Exception as exc:
            print(label, type(exc).__name__, str(exc), exc.__context__ is outer)


for expression in (b"a", "a"):
    compiled = re.compile(expression)
    for value in (None, 7, []):
        for name in ("search", "finditer", "split", "sub"):
            if name == "sub":
                replacement = b"X" if isinstance(expression, bytes) else "X"
                operation = lambda: compiled.sub(replacement, value)
            else:
                operation = lambda: getattr(compiled, name)(value)
            admission_result(("nonbuffer", expression, name, type(value).__name__), operation)


class SREBuffer:
    def __init__(self, failure=None, fail_on_call=None):
        self.data = bytearray(b"aa")
        self.failure = failure
        self.flags = []
        self.releases = 0
        self.fail_on_call = fail_on_call

    def __buffer__(self, flags):
        self.flags.append(flags)
        if self.failure is not None or len(self.flags) == self.fail_on_call:
            raise self.failure or TypeError("later capture exporter failure")
        return memoryview(self.data)

    def __release_buffer__(self, view):
        self.releases += 1
        view.release()


for failure_type in (TypeError, ValueError, BufferError, MemoryError):
    for expression in (b"a", "a"):
        compiled = re.compile(expression)
        for name in ("search", "finditer"):
            source = SREBuffer(failure_type("exporter failure"))
            admission_result(
                ("exporter", failure_type.__name__, expression, name),
                lambda: getattr(compiled, name)(source),
            )
            print("failed export custody", source.flags, source.releases)
    source = SREBuffer()
    match = re.search(b"a", source)
    source.failure = failure_type("capture exporter failure")
    admission_result(("capture", failure_type.__name__), lambda: match.group())
    print("capture export custody", source.flags, source.releases)

source = SREBuffer()
match = re.search(b"(?P<first>a)(?P<second>a)", source)
source.fail_on_call = len(source.flags) + 2
admission_result("later capture exporter", lambda: match.groups())
print("later capture export custody", source.flags, source.releases)

for expression in (b"a", "a"):
    for name in ("search", "finditer"):
        strided = memoryview(b"axax")[::2]
        admission_result(
            ("strided", expression, name),
            lambda: getattr(re.compile(expression), name)(strided),
        )
        strided.release()

source = SREBuffer()
iterator = re.finditer(b"a", source)
print("scanner admission flags", source.flags, source.releases)
try:
    source.data.append(0)
except BufferError:
    print("scanner holds export", True)
print("scanner spans", [match.span() for match in iterator])
print("scanner exhaustion custody", source.flags, source.releases)
source.data.append(0)
print("exhausted iterator stays live without export", bytes(source.data))
