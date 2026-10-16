"""Purpose: differential coverage for bytes ranges."""


def show(label, value):
    print(label, value)


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


b = b"  hello  world  "
show("find0", b.find(b"hello"))
show("find1", b.find(b"hello", 2))
show("find2", b.find(b"hello", 2, 6))
show("find3", b.find(b"", 3, 8))
show("find4", b.find(b"x", 0, 3))
show("find5", b.find(b"world", -10))
show("find6", b.find(b"world", -10, -1))
show("count0", b.count(b"l"))
show("count1", b.count(b"l", -8, -1))
show("starts0", b.startswith(b"hello", 2))
show("starts1", b.startswith((b"x", b"  he"), 0, 5))
show("starts2", b.startswith((), 0, 5))
show("ends0", b.endswith(b"  ", 0, len(b)))
show("ends1", b.endswith((b"x", b"ld  "), 0, len(b)))
show("ends2", b.endswith(b"world", -6, len(b)))

ba = bytearray(b"abcabc")
show("bafind0", ba.find(b"ab", 1))
show("bafind1", ba.find(b"", 2, 4))
show("bastarts0", ba.startswith((b"x", b"ab"), 0, 2))
show("baends0", ba.endswith(b"bc", 0, 3))
show("bacount0", ba.count(b"ab", -6, -1))

show_err("err_starts", lambda: b"abc".startswith((b"a", 1)))
show_err("err_find", lambda: b"abc".find("a"))
show_err("err_ba_find", lambda: bytearray(b"abc").find("a"))


class SearchBuffer:
    def __init__(self, events, value=b"b"):
        self.events = events
        self.value = value
        self.view = None

    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(self.value)
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))


class SearchBound:
    def __init__(self, events, label, value):
        self.events, self.label, self.value = events, label, value

    def __index__(self):
        self.events.append(self.label)
        return self.value


for factory in (bytes, bytearray):
    for method in ("find", "rfind", "index", "rindex", "count", "startswith", "endswith"):
        events = []
        needle = SearchBuffer(events)
        receiver = factory(b"abc")
        result = getattr(receiver, method)(needle, SearchBound(events, "start", 0), SearchBound(events, "end", 3))
        print("search-protocol", method, result, events)
    for method in ("split", "rsplit", "partition", "rpartition", "strip", "lstrip", "rstrip", "removeprefix", "removesuffix"):
        events = []
        needle = SearchBuffer(events)
        result = getattr(factory(b"babb"), method)(needle)
        # bytes.partition preserves Py_buffer.obj. A Python exporter returns
        # CPython's internal wrapper; its address-bearing repr is not an oracle.
        if method in ("partition", "rpartition"):
            result = (result[0], type(result[1]).__name__, result[2])
        print("byte-argument-protocol", method, result, events)


class PhysicalTail(bytes):
    def __buffer__(self, flags):
        raise AssertionError("tailmatch must use physical bytes")


for receiver in (b"abc", bytearray(b"abc")):
    print("tail-physical-subclass", receiver.startswith(PhysicalTail(b"a")), receiver.endswith(PhysicalTail(b"c")))
    try:
        receiver.startswith(1)
    except TypeError as error:
        print("tail-invalid-direct", str(error))
    try:
        receiver.startswith((b"x", 1))
    except TypeError as error:
        print("tail-invalid-tuple", str(error))


# Immutable partition preserves semantic identity; mutable partition copies.
class PartitionBytes(bytes):
    pass


class PartitionString(str):
    pass


class PartitionBytearray(bytearray):
    pass


for factory, text, separator in (
    (PartitionBytes, b"left<>right<>tail", b"<>"),
    (PartitionString, "left<>right<>tail", PartitionString("<>")),
    (PartitionBytearray, b"left<>right<>tail", bytearray(b"<>")),
):
    receiver = factory(text)
    for method in ("partition", "rpartition"):
        parts = getattr(receiver, method)(separator)
        absent = getattr(receiver, method)("missing" if isinstance(receiver, str) else b"missing")
        whole = absent[0 if method == "partition" else 2]
        print("partition-identity", factory.__name__, method, parts, parts[1] is separator, whole is receiver, type(whole).__name__)


for method in ("partition", "rpartition"):
    separator = bytearray(b"<>")
    view = memoryview(separator)
    parts = getattr(b"left<>right<>tail", method)(view)
    print("partition-view-owner", method, parts[0], parts[1] is view, parts[2])
    parts = getattr(b"left<>right<>tail", method)(separator)
    separator[:] = b"[]"
    print("partition-mutable-separator", method, parts, parts[1] is separator)
    view.release()


class PartitionExporter(bytes):
    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(b"<>")
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))


for method in ("partition", "rpartition"):
    for matched in (True, False):
        exporter = PartitionExporter(b"ignored-physical")
        exporter.events = []
        receiver = PartitionBytes(b"left<>right" if matched else b"absent")
        parts = getattr(receiver, method)(exporter)
        print("partition-overridden-export", method, matched, parts[0], type(parts[1]).__name__, parts[2], any(part is receiver for part in parts), exporter.events)
