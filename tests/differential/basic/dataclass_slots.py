from dataclasses import dataclass


@dataclass(slots=True)
class Point:
    x: int
    y: int = 2


p = Point(1)
print(p)
print(p.x, p.y)
print(p == Point(1, 2))
p.y = 5
print(p.y)
