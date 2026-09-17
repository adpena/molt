"""Split-family conversion ordering, buffer custody and whitespace capsule."""

events = []


class Index:
    def __init__(self, value=1, action=None):
        self.value = value
        self.action = action

    def __index__(self):
        events.append("index")
        if self.action is not None:
            self.action()
        return self.value


def observe(label, operation):
    events.clear()
    try:
        result = operation()
    except Exception as error:
        result = (type(error).__name__, str(error))
    print(label, events, repr(result))


def split_value(receiver, separator, maximum, right):
    if right:
        return receiver.rsplit(separator, maximum)
    return receiver.split(separator, maximum)


for right in (False, True):
    method = "rsplit" if right else "split"
    for factory, data in (
        (str, "  a b  "),
        (bytes, b"  a b  "),
        (bytearray, b"  a b  "),
    ):
        receiver = factory(data)
        descriptor = getattr(factory, method)
        observe(
            (factory.__name__, method, "wrong-self"),
            lambda: descriptor(1, None, Index()),
        )
        for separator in (1, data[:0], None, data[0:1]):
            observe(
                (factory.__name__, method, "sep", separator),
                lambda: split_value(receiver, separator, Index(), right),
            )
        for separator in (None, data[0:1], data[:0], 1):
            for maximum in (0, 1, -1, True, 1.5, 10**100, -(10**100)):
                observe(
                    (factory.__name__, method, "limit", separator, maximum),
                    lambda: split_value(receiver, separator, maximum, right),
                )
        if factory is not str:
            for buffer_separator in (bytearray(b" "), memoryview(b" ")):
                observe(
                    (
                        factory.__name__,
                        method,
                        "buffer",
                        type(buffer_separator).__name__,
                    ),
                    lambda: split_value(receiver, buffer_separator, Index(), right),
                )
            released_separator = memoryview(b" ")
            released_separator.release()
            for buffer_label, buffer_separator in (
                ("noncontiguous", memoryview(b"|x|")[::2]),
                ("released", released_separator),
                ("typed", memoryview(b"||").cast("H")),
                ("empty-strided", memoryview(b"|x|")[0:0:2]),
                ("base-empty", memoryview(b"")),
                ("singleton-strided", memoryview(b"|x")[::2]),
                ("two-dimensional", memoryview(b"||").cast("B", (1, 2))),
                (
                    "empty-two-dimensional-strided",
                    memoryview(bytearray(6)).cast("B", (3, 2))[0:0:2],
                ),
            ):
                if buffer_label != "released":
                    print(
                        (factory.__name__, method, "buffer-layout", buffer_label),
                        buffer_separator.c_contiguous,
                        buffer_separator.nbytes,
                        buffer_separator.shape,
                        buffer_separator.strides,
                    )
                observe(
                    (
                        factory.__name__,
                        method,
                        "buffer-error",
                        buffer_label,
                    ),
                    lambda: split_value(receiver, buffer_separator, Index(), right),
                )
                observe(
                    (factory.__name__, method, "buffer-join", buffer_label),
                    lambda: factory(b"").join([buffer_separator]),
                )
            separator_owner = bytearray(b" ")
            observe(
                (factory.__name__, method, "resize-sep-in-index"),
                lambda: split_value(
                    receiver,
                    separator_owner,
                    Index(action=lambda: separator_owner.extend(b"!")),
                    right,
                ),
            )
            observe(
                (factory.__name__, method, "mutate-sep-in-index"),
                lambda: split_value(
                    receiver,
                    separator_owner,
                    Index(action=lambda: separator_owner.__setitem__(0, ord("b"))),
                    right,
                ),
            )
        if factory is bytearray:
            observe(
                (factory.__name__, method, "resize-receiver-in-index"),
                lambda: split_value(
                    receiver, None, Index(action=lambda: receiver.extend(b" c")), right
                ),
            )
    for whitespace in ("\v", "\x1c", "\u00a0", "\u2003"):
        text = whitespace + "\ud800a" + whitespace + "b" + whitespace
        for maximum in (0, 1, -1):
            observe(
                (method, "surrogate-space", repr(whitespace), maximum),
                lambda: split_value(text, None, maximum, right),
            )
    for data in (b"\va\vb\v", bytearray(b"\va\vb\v")):
        observe(
            (method, "vertical-tab", type(data).__name__),
            lambda: split_value(data, None, 1, right),
        )


# Text whitespace is shared by splitting, trimming and classification, and is
# different from byte whitespace for the ASCII information separators.
class TextSubclass(str):
    pass


for whitespace in ("\v", "\x1c", "\x1d", "\x1e", "\x1f", "\u00a0", "\u2003"):
    for factory in (str, TextSubclass):
        for text in ("", whitespace, whitespace + "\ud800x" + whitespace):
            value = factory(text)
            for method in ("strip", "lstrip", "rstrip"):
                result = getattr(value, method)()
                print(
                    "text-trim",
                    factory.__name__,
                    method,
                    repr(text),
                    repr(result),
                    type(result).__name__,
                )
            print("text-space", factory.__name__, repr(text), value.isspace())
for method in ("strip", "lstrip", "rstrip"):
    for chars in ("", "\ud800", "\ud800x", 1):
        observe(
            ("custom-trim", method, repr(chars)),
            lambda: getattr("\ud800x\ud800", method)(chars),
        )

for method in ("strip", "lstrip", "rstrip", "isspace"):
    for receiver in (1, None, b" ", bytearray(b" ")):
        observe(
            ("text-descriptor", method, type(receiver).__name__),
            lambda: getattr(str, method)(receiver),
        )
