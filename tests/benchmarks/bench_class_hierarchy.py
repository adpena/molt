"""Measures method dispatch through class hierarchies."""


class Base:
    def compute(self, x: int) -> int:
        return x


class Mid(Base):
    def compute(self, x: int) -> int:
        return super().compute(x) + 1


class Leaf(Mid):
    def compute(self, x: int) -> int:
        return super().compute(x) * 2


def main() -> None:
    obj = Leaf()
    total = 0
    for i in range(5_000_000):
        total += obj.compute(i)
    print(total)


if __name__ == "__main__":
    main()
