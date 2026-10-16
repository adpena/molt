"""Purpose: differential coverage for codecs basic API surface."""

import codecs

print(codecs.lookup("utf-8").name)
print(codecs.encode("hi", "utf-8"))
print(codecs.decode(b"hi", "utf-8"))

replaced = codecs.decode(b"\xff", "utf-8", "replace")
print(replaced.encode("unicode_escape").decode("ascii"))


class ProtocolBuffer:
    def __init__(self, data):
        self.data = bytearray(data)
        self.events = []
        self.view = None

    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(self.data)
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None

for operation in (
    lambda value: codecs.decode(value, "utf-8"),
    lambda value: codecs.utf_8_decode(value),
    lambda value: codecs.getincrementaldecoder("utf-8")().decode(value, True),
    lambda value: codecs.charmap_decode(value),
):
    source = ProtocolBuffer(b"abc")
    print("buffer-codec", operation(source), source.events)
    source.data.extend(b"!")
    print("buffer-codec-unpinned", len(source.data))


class CountPoisonBuffer(ProtocolBuffer):
    def __len__(self):
        raise AssertionError("codec must count the acquired buffer, not its exporter")


# All complete low-level builtin decode results share the acquired byte count.
for encoding, data in (
    ("ascii", b"abc"),
    ("latin-1", b"a\xe9"),
    ("utf-8", b"a\xc3\xa9"),
    ("utf-16-le", b"a\x00\xe9\x00"),
    ("utf-16-be", b"\x00a\x00\xe9"),
    ("utf-32-le", b"a\x00\x00\x00\xe9\x00\x00\x00"),
    ("utf-32-be", b"\x00\x00\x00a\x00\x00\x00\xe9"),
    ("unicode-escape", b"a\\u00e9"),
):
    source = CountPoisonBuffer(data)
    print("native-codec-count", encoding, codecs.getdecoder(encoding)(source), source.events)
    source.data.extend(b"!")
    print("native-codec-count-unpinned", len(source.data))


class CountPoisonText(str):
    def __len__(self):
        raise AssertionError("codec must count native characters, not invoke a hook")


for encoding in ("utf-8", "utf-16-le", "utf-16-be", "utf-32-le", "utf-32-be"):
    source = CountPoisonText("aé\U0001f600")
    print("native-encode-count", encoding, codecs.getencoder(encoding)(source))
