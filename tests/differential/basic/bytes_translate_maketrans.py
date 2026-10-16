"""Purpose: differential coverage for bytes/bytearray translate + maketrans."""


def show_exc(label, fn, *args):
    try:
        fn(*args)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "no error")


print("bytes.maketrans is bytes().maketrans", bytes.maketrans is b"".maketrans)
print(
    "bytearray.maketrans is bytearray().maketrans",
    bytearray.maketrans is bytearray().maketrans,
)

trans = bytes.maketrans(b"abc", b"xyz")
print("trans_len", len(trans))
print(b"abc".translate(trans))
print(b"abc".translate(trans, b"b"))
print(b"abc".translate(None))
print(b"abc".translate(None, b"b"))
print(bytearray(b"abc").translate(trans))

show_exc("maketrans_len", bytes.maketrans, b"ab", b"c")
show_exc("maketrans_type", bytes.maketrans, "ab", b"cd")
show_exc("maketrans_args", bytes.maketrans, b"ab")

show_exc("translate_table_len", b"abc".translate, b"abc")
show_exc("translate_delete_type", b"abc".translate, trans, "a")


class ProtocolArgument:
    def __init__(self, name, value, events, mutate=None, fail=False):
        self.name = name
        self.value = value
        self.events = events
        self.mutate = mutate
        self.fail = fail
        self.view = None

    def __buffer__(self, flags):
        self.events.append((self.name, "get", flags))
        if self.mutate is not None:
            self.mutate()
        if self.fail:
            raise RuntimeError(self.name)
        self.view = memoryview(self.value)
        return self.view

    def __release_buffer__(self, view):
        self.events.append((self.name, "release", view is self.view))


class ReplaceCount:
    def __init__(self, events, receiver):
        self.events = events
        self.receiver = receiver

    def __index__(self):
        self.events.append("count")
        if isinstance(self.receiver, bytearray):
            self.receiver.extend(b"a")
        return 1


for factory in (bytes, bytearray):
    for fail_second in (False, True):
        events = []
        receiver = factory(b"aba")
        old = ProtocolArgument("old", b"a", events)
        new = ProtocolArgument("new", b"z", events, fail=fail_second)
        try:
            print("replace-protocol", receiver.replace(old, new, ReplaceCount(events, receiver)))
        except RuntimeError as error:
            print("replace-protocol-error", str(error))
        print("replace-protocol-events", events, receiver)

    for deletion in (b"", b"b"):
        events = []
        table = ProtocolArgument("table", bytes(range(256)), events)
        delete = ProtocolArgument("delete", deletion, events)
        print("translate-protocol", factory(b"abc").translate(table, delete), events)

    events = []
    first = ProtocolArgument("first", b"ab", events)
    second = ProtocolArgument("second", b"xy", events)
    table = factory.maketrans(first, second)
    print("maketrans-protocol", table[97:99], events)


class PhysicalBytes(bytes):
    def __buffer__(self, flags):
        raise AssertionError("physical bytes shortcut dispatched")


print("translate-physical-subclass", b"abc".translate(PhysicalBytes(bytes(range(256))), PhysicalBytes(b"b")))
