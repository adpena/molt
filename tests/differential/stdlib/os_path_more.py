"""Purpose: differential coverage for os.path basics."""

import os

print(os.path.basename("/tmp/file.txt"))
print(os.path.dirname("/tmp/file.txt"))
print(os.path.join("/tmp", "file.txt"))

for parts in [
    ("root", "", "leaf"),
    (b"root", b"\xff", b""),
    ("root", "/absolute", "leaf"),
]:
    print("joined", repr(os.path.join(*parts)))

for value in ("base/", b"base/"):
    empty = value[:0]
    print(
        "empty-identity",
        os.path.join(empty, value) is value,
        os.path.join(value, empty) is value,
    )


class IdentityObserver(str):
    def __radd__(self, other):
        print("left-identity", other is original)
        return str.__add__(other, self)


original = "base/"
print("identity-callback", os.path.join(original, "", IdentityObserver("last")))

events = []


class Component(str):
    def startswith(self, prefix):
        events.append(("startswith", str(self)))
        return super().startswith(prefix)

    def endswith(self, suffix):
        events.append(("endswith", str(self)))
        return super().endswith(suffix)

    def __getitem__(self, key):
        events.append(("slice", str(self)))
        return super().__getitem__(key)

    def __add__(self, other):
        events.append(("add", str(self)))
        return super().__add__(other)

    def __radd__(self, other):
        events.append(("radd", str(self)))
        return str.__add__(other, self)

    def __del__(self):
        events.append(("retire", str(self)))


class PathValue:
    def __init__(self, value, subclass=False):
        self.value = value
        self.subclass = subclass

    def __fspath__(self):
        events.append(("fspath", self.value))
        return Component(self.value) if self.subclass else self.value


base = Component("base")
print("single-identity", os.path.join(base) is base, events)
events.clear()
absolute = Component("/absolute")
print("absolute-identity", os.path.join("base", absolute) is absolute, events)
events.clear()
print(
    "callbacks",
    os.path.join(PathValue("base"), PathValue("child", True), PathValue("last")),
)
print("events", events)
events.clear()
print("byte-pathlike", repr(os.path.join(PathValue(b"base"), PathValue(b"\xff"))))
print("events", events)
events.clear()
for parts in [
    ("a", b"b"),
    (PathValue("a"), PathValue(b"b"), PathValue("never")),
    ("a", bytearray(b"b")),
]:
    try:
        os.path.join(*parts)
    except TypeError as exc:
        print("error", str(exc), events)
    events.clear()
