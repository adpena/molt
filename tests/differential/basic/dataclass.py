"""Purpose: differential coverage for dataclass."""

from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int = 2


p = Point(1)
print(p.x)
print(p.y)
print(p)
print(p == Point(1, 2))
print(p == Point(2, 2))
p.y = 5
print(p.y)
p.z = 9
print(p.z)
