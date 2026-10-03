"""Differential coverage for eager sum(generator) inline lowering."""


def main() -> None:
    v = 10
    total = sum(v for v in range(5))
    print("name-scope", v, total)

    pairs = [(2, 3), (4, 5), (6, 7)]
    print("tuple-target", sum(a * b for a, b in pairs if a > 2))

    empty = sum(i for i in [])
    print("empty-default", empty, type(empty).__name__)

    # The outermost iterable is evaluated in this scope, where x is a cell.
    x = 4
    probe = lambda: x
    print("iterable-scope", sum(x for x in range(x)), probe())


if __name__ == "__main__":
    main()
