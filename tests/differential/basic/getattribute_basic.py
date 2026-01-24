"""Purpose: differential coverage for getattribute basic."""


class Foo:
    def __init__(self):
        self.x = 10

    def __getattribute__(self, name):
        if name == "x":
            return 42
        if name == "y":
            return "ok"
        return "miss"


foo = Foo()
print(foo.x)
print(foo.y)
print(foo.z)
