"""Purpose: differential coverage for core string predicate surface."""

CASES = [
    "",
    "abc",
    "ABC",
    "AbC",
    "abc123",
    "123",
    "abc!",
    "HELLO!",
    "ß",
    "Σ",
    "σ",
    "ǅ",
    "ǅa",
    "aǅ",
    "A\u0301",
    "٣",
    "²",
    "⅕",
    " ",
    "\t",
    "\n",
    "\u2007",
    "\u2028",
    "Title Case",
    "title case",
    "A B",
    "A b",
]


def main() -> None:
    for value in CASES:
        codepoints = ",".join(str(ord(ch)) for ch in value) if value else "-"
        print(
            codepoints,
            value.isalpha(),
            value.isalnum(),
            value.isdecimal(),
            value.isdigit(),
            value.isnumeric(),
            value.islower(),
            value.isupper(),
            value.isspace(),
            value.istitle(),
            value.isprintable(),
            value.isascii(),
        )


if __name__ == "__main__":
    main()
