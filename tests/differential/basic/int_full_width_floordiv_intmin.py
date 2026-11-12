def wide(n: int) -> int:
    x = -(2**62)
    for _ in range(n):
        x = x + x
    return x


def main() -> None:
    m = wide(1)
    print(m, m // -1, m % -1)
    d = -1
    for _ in range(2):
        print(m // d, m % d, divmod(m, d))
    y = wide(1) + 0
    print(y // -1)


main()
