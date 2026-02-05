"""Purpose: differential coverage for with context."""

from molt.stdlib import contextlib


def main():
    value = 0
    with contextlib.nullcontext(42) as current:
        value = current
    print(value)


if __name__ == "__main__":
    main()
