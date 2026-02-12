class Box:
    def __init__(self) -> None:
        self.a = 1
        self.b = 2


def main() -> None:
    call_dir = dir
    obj = Box()
    i = 0
    total = 0
    while i < 100_000:
        names = call_dir(obj)
        total += len(names)
        i += 1
    print(total)


if __name__ == "__main__":
    main()
