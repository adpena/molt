"""Purpose: differential coverage for additional str case transformation methods."""

CASES = [
    "",
    "hello world",
    "HELLO WORLD",
    "hELLo WOrld",
    "they're bill's friends from the UK",
    "straße",
    "ǅungla",
    "A\u0301bc",
]


def main() -> None:
    for value in CASES:
        print(value.swapcase())


if __name__ == "__main__":
    main()
