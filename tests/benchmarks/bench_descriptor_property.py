class Box:
    def __init__(self, x: int) -> None:
        self._x = x

    @property
    def x(self) -> int:
        return self._x


box = Box(1)
i = 0
total = 0
while i < 500_000:
    total += box.x
    i += 1

print(total)
