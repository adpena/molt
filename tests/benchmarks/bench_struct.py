class Point:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


def main() -> None:
    i = 0
    while i < 1_000_000:
        p = Point(0, 0)
        p.x = i
        p.y = i + 1
        i += 1
    print(i)


if __name__ == "__main__":
    main()
