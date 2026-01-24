"""Purpose: differential coverage for lambda sorting."""


def main():
    factor = 3

    def f(x, y=2):
        return x * y * factor

    print(f(4))

    data = [(2, "b"), (1, "a"), (3, "c")]
    print(sorted(data))
    print(sorted(data, key=lambda item: item[1]))
    items = list(data)
    items.sort(key=lambda item: item[0] * factor, reverse=True)
    print(items)


if __name__ == "__main__":
    main()
