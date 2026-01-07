class Point:
    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y


p = Point(1, 2)
i = 0
total = 0
while i < 500_000:
    p.x = i
    total += p.x
    i += 1

print(total)
