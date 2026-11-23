"""Exercise exact short ASCII admission, ordinary indexing and mutable calls."""


def parse_text(text: str) -> int:
    i = 0
    value = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


class Overridden(str):
    def __len__(self):
        print("length")
        return 2

    def __getitem__(self, i):
        print("index", i)
        if i == 1:
            raise LookupError("second character")
        return "7"


def dynamic(text, len, ord):
    i = 0
    value = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


for s in ("", "0", "42", "307", "9007", "12345", "\x00\x7f", "é", "𝟗", "\ud800"):
    print(parse_text(s))

try:
    parse_text(Overridden("09"))
except LookupError as error:
    print(type(error).__name__, str(error))

print(dynamic("12", lambda _: 1, lambda _: 57))
