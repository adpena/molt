"""Purpose: differential coverage for struct layout mutation."""


class Point:
    x: int
    y: int


p = Point()
p.x = 1
p.y = 2
Point.z = 3
p.z = 4
print(p.x, p.y, p.z)
