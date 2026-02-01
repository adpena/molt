class Box:
    def __init__(self, x: int) -> None:
        self._x = x

    @property
    def x(self) -> int:
        return self._x


def main() -> None:
    container = Box(1)
    i = 0
    total = 0
    while i < 500_000:
        total += container.x
        i += 1

    print(total)


if __name__ == "__main__":
    main()
