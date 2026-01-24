"""Purpose: differential coverage for class attrs."""


class Foo:
    y = 5

    def __init__(self, x: int) -> None:
        self.x = x

    def bump(self) -> int:
        self.x = self.x + 1
        return self.x


f = Foo(2)
print(f.x)
print(f.y)
print(f.bump())
print(f.x)


class Bar:
    z = "ready"

    def __init__(self) -> None:
        pass


b = Bar()
print(b.z)
b.z = "set"
print(b.z)
b.extra = "extra"
print(b.extra)


class Baz:
    def __init__(self) -> None:
        pass

    def set(self) -> None:
        self.value = 7

    def get(self) -> int:
        return self.value


baz = Baz()
baz.set()
print(baz.get())
