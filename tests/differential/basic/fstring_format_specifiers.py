"""Purpose: differential coverage for f-string format specifiers + conversions."""


def main() -> None:
    value = 12.3456
    name = "molt"
    print(f"{value:.2f}")
    print(f"{value:8.1f}")
    print(f"{value:>8.2f}")
    print(f"{name!r:>10}")
    print(f"{name!s:^8}")
    print(f"{value=:.1f}")


main()
