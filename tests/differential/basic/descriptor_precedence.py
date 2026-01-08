class C:
    def x(self) -> int:
        return 1


c = C()
c.x = 7
print(c.x)


class D:
    @property
    def y(self) -> int:
        return 2


d = D()
try:
    d.y = 3
except Exception as exc:
    print(type(exc).__name__)
print(d.y)


class E:
    def __init__(self) -> None:
        self.z = 4


e = E()
print(e.z)


def get_z(self):
    return 9


E.z = property(get_z)
print(e.z)
