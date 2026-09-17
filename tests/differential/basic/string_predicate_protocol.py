"""String predicate descriptor admission and Python/WTF-8 scalar semantics."""

PREDICATES = (
    ("isidentifier", str.isidentifier),
    ("isdigit", str.isdigit),
    ("isdecimal", str.isdecimal),
    ("isnumeric", str.isnumeric),
    ("isspace", str.isspace),
    ("isalpha", str.isalpha),
    ("isalnum", str.isalnum),
    ("islower", str.islower),
    ("isupper", str.isupper),
    ("isascii", str.isascii),
    ("istitle", str.istitle),
    ("isprintable", str.isprintable),
)


def results(value):
    return tuple(method(value) for _, method in PREDICATES)


print("order", tuple(name for name, _ in PREDICATES))
for value in (
    "",
    "a",
    "A",
    "Abc",
    "ABC",
    "abc123",
    "_a0",
    "0a",
    "123",
    " ",
    "\t\n\v\f\r",
    "\x1c\x1d\x1e\x1f",
    "\x00",
    "\x7f",
    "\u00a0\u2003",
    "\u200b",
    "\u00e9",
    "\u00c9",
    "\u00b2",
    "\u00bc",
    "\u0660",
    "\u2160",
    "\u2170",
    "\u4e00",
    "\u01c5",
    "\u01c5a",
    "a\u01c5",
    "\u00aa",
    "A\u00aa",
    "A\u00aaB",
    "\u02b0",
    "\u0301",
    "\u0345",
    "a\u0345",
    "A\u0345",
    "\u05b0",
    "\u0903",
    "\U00010400",
    "\U00010428",
    "\U0001f600",
    "\ud800",
    "\udfff",
    "\ud800\udfff",
    "a\ud800",
    "\ud800a",
    "A\udfff",
    "\udfffA",
    "a\ud800b",
    "A\ud800B",
    "A\ud800b",
    "Ab\ud800Cd",
    "\u01c5\ud800A",
    "\u00aa\ud800",
    "1\ud800",
    " \ud800",
):
    print("scalar", ascii(value), results(value))

for length in (15, 16, 17, 31, 32, 33):
    for char in ("a", "A", "7", " ", "\x1c", "~", "\x7f"):
        print("ascii", length, ascii(char), results(char * length))


events = []


class Text(str):
    def __str__(self):
        events.append("str")
        raise AssertionError("predicate called __str__")

    def __len__(self):
        events.append("len")
        raise AssertionError("predicate called __len__")

    def __iter__(self):
        events.append("iter")
        raise AssertionError("predicate called __iter__")

    def __getitem__(self, index):
        events.append("getitem")
        raise AssertionError("predicate called __getitem__")

    def isalpha(self):
        events.append("isalpha override")
        return "override"


class Pretender:
    def __str__(self):
        events.append("str")
        raise AssertionError("descriptor converted receiver")

    def __index__(self):
        events.append("index")
        raise AssertionError("descriptor indexed receiver")

    def __iter__(self):
        events.append("iter")
        raise AssertionError("descriptor iterated receiver")


class WrongBytes(bytes):
    pass


class WrongInt(int):
    pass


def produce(value):
    events.append("receiver")
    return value


for source in ("", "abc", "Abc", "\u0345", "a\ud800", "A\ud800B"):
    value = Text(source)
    for name, method in PREDICATES:
        events.clear()
        result = method(produce(value))
        print("subclass", ascii(source), name, result, type(result).__name__, events)

for value in (
    None,
    7,
    1.5,
    True,
    b"abc",
    bytearray(b"abc"),
    [],
    (),
    {},
    object(),
    Pretender(),
    WrongBytes(b"abc"),
    WrongInt(7),
):
    for name, method in PREDICATES:
        events.clear()
        try:
            result = method(produce(value))
            print("invalid", type(value).__name__, name, "returned", result, events)
        except Exception as exc:
            print(
                "invalid",
                type(value).__name__,
                name,
                type(exc).__name__,
                str(exc),
                events,
            )

# The transformation's cased-boundary consumer shares the generated property;
# it must not retain the old lower() != upper() allocation heuristic.
for value in ("A\u00aaB", "a\u02b0B", "A\u0345B"):
    print("title-boundary", ascii(value), ascii(str.title(value)))
