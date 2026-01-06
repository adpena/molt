class Base:
    kind = "base"

    def __init__(self, x: int) -> None:
        self.x = x

    def inc(self, y: int) -> int:
        return self.x + y


class Mid(Base):
    pass


class Leaf(Mid):
    def dec(self, y: int) -> int:
        return self.x - y


obj = Leaf(10)
print(obj.x)
print(obj.inc(2))
print(obj.dec(3))
print(obj.kind)
print(Leaf.kind)
print(Leaf.inc(obj, 5))
