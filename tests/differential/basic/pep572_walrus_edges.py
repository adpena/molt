"""Purpose: differential coverage for PEP 572 walrus operator edges."""

def value_fn() -> int:
    return 5


def main() -> None:
    value = (x := 1)
    print(x, value)

    print((y := 3))  # noqa: F841

    total = 0
    it = iter([1, 2, 3])
    while (n := next(it, None)) is not None:
        total += n
    print(total)

    result = (z := value_fn())
    print(z, result)


main()
